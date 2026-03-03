use std::env;
use std::sync::Arc;
use std::thread;

use anyhow::Context;

use crate::config::Config;
use crate::github::GitHubClient;
use crate::model::{self, GenerationError, PromptInput, PromptPrMetadata};
use crate::render;
use crate::storage::{GenerationJob, Store};

#[derive(Debug, Default)]
pub struct WorkerPassResult {
    pub claimed: usize,
    pub ready: usize,
    pub failed: usize,
    pub retry_scheduled: usize,
}

pub fn run_generation_pass(store: &Store, config: &Config) -> anyhow::Result<WorkerPassResult> {
    let jobs = store
        .claim_pending_generation_jobs(config.worker_concurrency)
        .context("claim pending generation jobs")?;
    if jobs.is_empty() {
        return Ok(WorkerPassResult::default());
    }

    let github = Arc::new(
        GitHubClient::new(env::var(&config.github_token_env).ok())
            .context("initialize GitHub client for worker")?,
    );
    let shared_store = Arc::new(store.clone());

    let mut handles = Vec::with_capacity(jobs.len());
    for job in jobs {
        let github = Arc::clone(&github);
        let store = Arc::clone(&shared_store);
        let model = config.model.clone();
        let api_key_env = config.api_key_env.clone();
        let retry_backoff_secs = config.retry_backoff_secs;

        handles.push(thread::spawn(move || {
            process_job(
                &store,
                &github,
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
    github: &GitHubClient,
    job: &GenerationJob,
    model_name: &str,
    api_key_env: &str,
    retry_backoff_secs: u64,
) -> anyhow::Result<JobOutcome> {
    let diff = github
        .fetch_pull_diff(&job.repo, job.pr_number)
        .with_context(|| format!("fetch PR diff for {}/{}", job.repo, job.pr_number));

    let diff = match diff {
        Ok(diff) => diff,
        Err(err) => {
            store
                .mark_generation_failed(
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
            number: Some(job.pr_number as u64),
            title: job.title.clone(),
            head_sha: Some(job.head_sha.clone()),
            base_ref: job.base_ref.clone(),
        },
        files: diff.files,
        unified_diff: model::bounded_diff(&diff.unified_diff, 60_000),
    };

    let generated = model::generate_with_anthropic(model_name, api_key_env, &prompt_input);

    let doc = match generated {
        Ok(doc) => doc,
        Err(GenerationError::RetrySafe(message)) => {
            store
                .mark_generation_failed(job.artifact_id, &message, true, retry_backoff_secs)
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
                .mark_generation_failed(
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
                .mark_generation_failed(job.artifact_id, &message, false, retry_backoff_secs)
                .with_context(|| {
                    format!("persist fatal failure for artifact {}", job.artifact_id)
                })?;
            return Ok(JobOutcome::Failed);
        }
    };

    let html = render::render_tutorial_html(
        &format!("{}#{} tutorial", job.repo, job.pr_number),
        &job.base_ref,
        &diff.unified_diff,
        &doc,
    );

    let tutorial_json = serde_json::to_string(&doc).context("serialize tutorial JSON")?;
    store
        .mark_generation_ready(
            job.artifact_id,
            &tutorial_json,
            &html,
            &job.head_sha,
            &diff.unified_diff,
        )
        .with_context(|| format!("mark artifact {} ready", job.artifact_id))?;

    Ok(JobOutcome::Ready)
}

#[cfg(test)]
mod tests {
    use super::WorkerPassResult;

    #[test]
    fn result_defaults_are_zeroed() {
        let result = WorkerPassResult::default();
        assert_eq!(result.claimed, 0);
        assert_eq!(result.ready, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(result.retry_scheduled, 0);
    }
}
