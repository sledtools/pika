use std::collections::BTreeSet;
use std::thread;

use anyhow::Context;

use crate::auth::normalize_npub;
use crate::branch_store::BranchGenerationJob;
use crate::config::Config;
use crate::forge;
use crate::model::{self, GenerationError, PromptInput, PromptPrMetadata};
use crate::render;
use crate::storage::Store;
use crate::tutorial;

#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct WorkerPassResult {
    pub claimed: usize,
    pub ready: usize,
    pub failed: usize,
    pub retry_scheduled: usize,
}

pub fn run_generation_pass(store: &Store, config: &Config) -> anyhow::Result<WorkerPassResult> {
    let Some(forge_repo) = config.effective_forge_repo() else {
        return Ok(WorkerPassResult::default());
    };

    let jobs = store
        .claim_pending_branch_generation_jobs(config.worker_concurrency)
        .context("claim pending branch generation jobs")?;
    if jobs.is_empty() {
        return Ok(WorkerPassResult::default());
    }

    let mut handles = Vec::with_capacity(jobs.len());
    for job in jobs {
        let store = store.clone();
        let forge_repo = forge_repo.clone();
        let config = config.clone();
        let model = config.model.clone();
        let api_key_env = config.api_key_env.clone();
        let retry_backoff_secs = config.retry_backoff_secs;

        handles.push(thread::spawn(move || {
            process_job(
                &store,
                &config,
                &forge_repo,
                &job,
                &model,
                &api_key_env,
                retry_backoff_secs,
            )
        }));
    }

    let mut result = WorkerPassResult {
        claimed: handles.len(),
        ..WorkerPassResult::default()
    };
    for handle in handles {
        match handle.join() {
            Ok(Ok(JobOutcome::Ready)) => result.ready += 1,
            Ok(Ok(JobOutcome::RetryScheduled)) => result.retry_scheduled += 1,
            Ok(Ok(JobOutcome::Failed)) => result.failed += 1,
            Ok(Err(err)) => {
                result.failed += 1;
                eprintln!("worker thread returned error: {}", err);
            }
            Err(_) => {
                result.failed += 1;
                eprintln!("worker thread panicked");
            }
        }
    }

    Ok(result)
}

enum JobOutcome {
    Ready,
    RetryScheduled,
    Failed,
}

fn process_job(
    store: &Store,
    config: &Config,
    forge_repo: &crate::config::ForgeRepoConfig,
    job: &BranchGenerationJob,
    model_name: &str,
    api_key_env: &str,
    retry_backoff_secs: u64,
) -> anyhow::Result<JobOutcome> {
    let diff = forge::branch_diff(forge_repo, &job.merge_base_sha, &job.head_sha)
        .with_context(|| format!("collect branch diff for {}", job.branch_name));
    let diff = match diff {
        Ok(diff) => diff,
        Err(err) => {
            store
                .mark_branch_generation_failed(
                    job.artifact_id,
                    &format!("diff fetch failed: {}", err),
                    true,
                    retry_backoff_secs,
                )
                .context("persist diff-fetch failure")?;
            return Ok(JobOutcome::RetryScheduled);
        }
    };

    let prompt_input = PromptInput {
        pr: PromptPrMetadata {
            repo: job.repo.clone(),
            number: Some(job.branch_id as u64),
            title: format!("branch {}: {}", job.branch_name, job.title),
            head_sha: Some(job.head_sha.clone()),
            base_ref: forge_repo.default_branch.clone(),
        },
        files: tutorial::extract_files(&diff),
        unified_diff: model::bounded_diff(&diff, 60_000),
    };

    let generated = model::generate_with_anthropic(model_name, api_key_env, &prompt_input);
    let gen_output = match generated {
        Ok(output) => output,
        Err(GenerationError::RetrySafe(message)) => {
            store
                .mark_branch_generation_failed(job.artifact_id, &message, true, retry_backoff_secs)
                .with_context(|| {
                    format!(
                        "persist retry-safe failure for artifact {}",
                        job.artifact_id
                    )
                })?;
            return Ok(JobOutcome::RetryScheduled);
        }
        Err(GenerationError::MissingApiKey { env_var }) => {
            store
                .mark_branch_generation_failed(
                    job.artifact_id,
                    &format!("missing API key env var {}", env_var),
                    false,
                    retry_backoff_secs,
                )
                .with_context(|| {
                    format!(
                        "persist missing-key failure for artifact {}",
                        job.artifact_id
                    )
                })?;
            return Ok(JobOutcome::Failed);
        }
        Err(GenerationError::Fatal(message)) => {
            store
                .mark_branch_generation_failed(job.artifact_id, &message, false, retry_backoff_secs)
                .with_context(|| {
                    format!("persist fatal failure for artifact {}", job.artifact_id)
                })?;
            return Ok(JobOutcome::Failed);
        }
    };

    let html = render::render_tutorial_html(
        &format!("branch #{} {}", job.branch_id, job.branch_name),
        &forge_repo.default_branch,
        &diff,
        &gen_output.tutorial,
    );
    let tutorial_json =
        serde_json::to_string(&gen_output.tutorial).context("serialize tutorial JSON")?;
    store
        .mark_branch_generation_ready(job.artifact_id, &tutorial_json, &html, &job.head_sha, &diff)
        .with_context(|| format!("mark branch artifact {} ready", job.artifact_id))?;
    populate_ready_branch_inbox(store, config, job.artifact_id)
        .with_context(|| format!("populate inbox for branch artifact {}", job.artifact_id))?;

    Ok(JobOutcome::Ready)
}

fn branch_inbox_recipients(store: &Store, config: &Config) -> anyhow::Result<Vec<String>> {
    let mut recipients = BTreeSet::new();
    for npub in config.effective_bootstrap_admin_npubs() {
        if let Ok(normalized) = normalize_npub(&npub) {
            recipients.insert(normalized);
        }
    }
    for npub in &config.allowed_npubs {
        if let Ok(normalized) = normalize_npub(npub) {
            recipients.insert(normalized);
        }
    }
    for npub in store.list_active_chat_allowlist_npubs()? {
        if let Ok(normalized) = normalize_npub(&npub) {
            recipients.insert(normalized);
        }
    }
    Ok(recipients.into_iter().collect())
}

fn populate_ready_branch_inbox(
    store: &Store,
    config: &Config,
    artifact_id: i64,
) -> anyhow::Result<usize> {
    let recipients = branch_inbox_recipients(store, config)?;
    store.populate_branch_inbox(artifact_id, &recipients)
}

#[cfg(test)]
mod tests {
    use nostr::nips::nip19::ToBech32;
    use nostr::Keys;

    use super::populate_ready_branch_inbox;
    use crate::branch_store::BranchUpsertInput;
    use crate::config::Config;
    use crate::storage::Store;

    fn branch_upsert_input(branch_name: &str, head_sha: &str) -> BranchUpsertInput {
        BranchUpsertInput {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: "/tmp/pika.git".to_string(),
            default_branch: "master".to_string(),
            ci_entrypoint: "just pre-merge".to_string(),
            branch_name: branch_name.to_string(),
            title: format!("{branch_name} title"),
            head_sha: head_sha.to_string(),
            merge_base_sha: "base123".to_string(),
            author_name: Some("alice".to_string()),
            author_email: Some("alice@example.com".to_string()),
            updated_at: "2026-03-18T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn ready_branch_artifacts_feed_branch_inbox() {
        let admin_npub = Keys::generate()
            .public_key()
            .to_bech32()
            .expect("encode admin npub");
        let legacy_npub = Keys::generate()
            .public_key()
            .to_bech32()
            .expect("encode legacy npub");
        let allowlisted_npub = Keys::generate()
            .public_key()
            .to_bech32()
            .expect("encode allowlisted npub");
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        store
            .upsert_chat_allowlist_entry(
                &allowlisted_npub,
                true,
                false,
                Some("reviewer"),
                &admin_npub,
            )
            .expect("upsert allowlist");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/inbox", "head-1"))
            .expect("insert branch");
        let artifact_id = store
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT id
                     FROM branch_artifact_versions
                     WHERE branch_id = ?1
                     ORDER BY version DESC
                     LIMIT 1",
                    rusqlite::params![branch.branch_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("branch artifact id");
        store
            .mark_branch_generation_ready(
                artifact_id,
                r#"{"executive_summary":"ok","media_links":[],"steps":[]}"#,
                "<p>ok</p>",
                "head-1",
                "diff",
            )
            .expect("mark ready");

        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: None,
            poll_interval_secs: 60,
            model: "test-model".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            github_token_env: "GITHUB_TOKEN".to_string(),
            merged_lookback_hours: 72,
            worker_concurrency: 1,
            retry_backoff_secs: 120,
            webhook_secret_env: "PIKA_NEWS_WEBHOOK_SECRET".to_string(),
            bind_address: "127.0.0.1".to_string(),
            bind_port: 8787,
            allowed_npubs: vec![legacy_npub.clone()],
            bootstrap_admin_npubs: vec![admin_npub.clone()],
        };

        assert_eq!(
            populate_ready_branch_inbox(&store, &config, artifact_id).unwrap(),
            3
        );
        assert_eq!(store.branch_inbox_count(&admin_npub).unwrap(), 1);
        assert_eq!(store.branch_inbox_count(&legacy_npub).unwrap(), 1);
        assert_eq!(store.branch_inbox_count(&allowlisted_npub).unwrap(), 1);
    }
}
