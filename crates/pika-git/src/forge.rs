use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::io::{BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use chrono::Utc;
use fs2::FileExt;
use jerichoci::{RunLifecycleEvent, RunStatus};
use tempfile::TempDir;

use crate::config::{ForgeRepoConfig, DEFAULT_MIRROR_TIMEOUT_SECS};
use crate::jerichoci_store::JerichociRunStore;

#[derive(Debug, Clone)]
pub struct CanonicalBranch {
    pub branch_name: String,
    pub head_sha: String,
    pub merge_base_sha: String,
    pub title: String,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MergeOutcome {
    pub head_sha: String,
    pub merge_base_sha: String,
    pub merge_commit_sha: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CloseOutcome {
    pub head_sha: Option<String>,
    pub deleted: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CiExecutionResult {
    pub success: bool,
    pub log: String,
    pub ci_run_id: Option<String>,
    pub ci_target_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CiExecutionEvent {
    CiRunStarted {
        run_id: String,
        target_id: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct MirrorSyncOutcome {
    pub remote_name: String,
    pub local_default_head: Option<String>,
    pub remote_default_head: Option<String>,
    pub lagging_ref_count: i64,
    pub synced_ref_count: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MirrorLockStatus {
    pub state: String,
    pub pid: Option<u32>,
    pub trigger_source: Option<String>,
    pub operation: Option<String>,
    pub started_at: Option<String>,
    pub age_secs: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct MirrorLockMetadata {
    pid: u32,
    trigger_source: String,
    operation: String,
    started_at: String,
    started_at_unix_secs: i64,
}

struct MirrorLockGuard {
    file: std::fs::File,
}

impl Drop for MirrorLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub fn ensure_canonical_repo(repo: &ForgeRepoConfig) -> anyhow::Result<()> {
    let git_dir = canonical_git_dir(repo);
    if git_dir.join("HEAD").exists() {
        return Ok(());
    }
    if let Some(parent) = git_dir.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for {}", git_dir.display()))?;
    }
    let output = Command::new("git")
        .args(["init", "--bare", git_dir.to_str().unwrap_or("")])
        .output()
        .with_context(|| format!("initialize bare repo {}", git_dir.display()))?;
    decode_git_output(output).map(|_| ())
}

pub fn install_hooks(repo: &ForgeRepoConfig, secret: &str) -> anyhow::Result<()> {
    let hooks_dir = effective_hooks_dir(repo)?;
    fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("create hooks dir in {}", hooks_dir.display()))?;

    let update_hook = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nexport PATH=\"/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:/usr/local/bin:/usr/bin:/bin:${{PATH:-}}\"\nref_name=\"$1\"\nif [[ \"$ref_name\" == \"refs/heads/{default_branch}\" ]]; then\n  echo \"direct pushes to {default_branch} are rejected; merge through the forge\" >&2\n  exit 1\nfi\n",
        default_branch = repo.default_branch
    );
    write_hook(&hooks_dir.join("update"), &update_hook)?;

    let hook_url = repo
        .hook_url
        .as_deref()
        .ok_or_else(|| anyhow!("forge hook_url missing for {}", repo.repo))?;
    let openssl = shell_quote(&resolve_hook_tool("openssl")?);
    let curl = shell_quote(&resolve_hook_tool("curl")?);
    let post_receive_hook = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nexport PATH=\"/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:/usr/local/bin:/usr/bin:/bin:${{PATH:-}}\"\npayload=\"$(mktemp)\"\ntrap 'rm -f \"$payload\"' EXIT\ncat >\"$payload\"\nif [[ ! -s \"$payload\" ]]; then\n  exit 0\nfi\nsignature=\"sha256=$({openssl} dgst -sha256 -hmac {secret} -hex \"$payload\" | sed 's/^.*= //')\"\nif ! {curl} --silent --show-error --fail -X POST -H \"content-type: text/plain\" -H \"x-pika-signature-256: ${{signature}}\" --data-binary @\"$payload\" {url} >/dev/null; then\n  echo \"warning: failed to notify pika forge post-receive hook\" >&2\nfi\n",
        openssl = openssl,
        curl = curl,
        secret = shell_quote(secret),
        url = shell_quote(hook_url),
    );
    write_hook(&hooks_dir.join("post-receive"), &post_receive_hook)?;

    Ok(())
}

pub fn list_branches(repo: &ForgeRepoConfig) -> anyhow::Result<Vec<CanonicalBranch>> {
    let output = git_bare(
        repo,
        [
            "for-each-ref",
            "--format=%(refname:short)%00%(objectname)",
            "refs/heads",
        ],
    )?;
    let mut branches = Vec::new();
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((branch_name, head_sha)) = line.split_once('\0') else {
            continue;
        };
        if branch_name == repo.default_branch {
            continue;
        }
        branches.push(describe_branch(repo, branch_name, head_sha)?);
    }
    branches.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(branches)
}

pub fn current_branch_head(
    repo: &ForgeRepoConfig,
    branch_name: &str,
) -> anyhow::Result<Option<String>> {
    resolve_ref(repo, &format!("refs/heads/{branch_name}"))
}

pub fn branch_diff(
    repo: &ForgeRepoConfig,
    merge_base_sha: &str,
    head_sha: &str,
) -> anyhow::Result<String> {
    if merge_base_sha.trim().is_empty() {
        return Ok(String::new());
    }
    git_bare(
        repo,
        [
            "diff",
            "--unified=3",
            &format!("{merge_base_sha}..{head_sha}"),
        ],
    )
}

pub fn changed_paths(
    repo: &ForgeRepoConfig,
    merge_base_sha: &str,
    head_sha: &str,
) -> anyhow::Result<Vec<String>> {
    let output = if merge_base_sha.trim().is_empty() {
        git_bare(repo, ["ls-tree", "-r", "--name-only", head_sha])?
    } else {
        git_bare(
            repo,
            [
                "diff",
                "--name-only",
                &format!("{merge_base_sha}..{head_sha}"),
            ],
        )?
    };
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn merge_branch(
    repo: &ForgeRepoConfig,
    branch_name: &str,
    expected_head_sha: &str,
) -> anyhow::Result<MergeOutcome> {
    let default_branch_ref = format!("refs/heads/{}", repo.default_branch);
    let current_head = resolve_ref(repo, &format!("refs/heads/{branch_name}"))?
        .ok_or_else(|| anyhow!("branch `{branch_name}` no longer exists"))?;
    if current_head != expected_head_sha {
        bail!(
            "branch head changed from {} to {}",
            expected_head_sha,
            current_head
        );
    }

    let default_head_sha = resolve_ref(repo, &default_branch_ref)?
        .ok_or_else(|| anyhow!("default branch `{}` no longer exists", repo.default_branch))?;
    let merge_base_sha = git_bare(repo, ["merge-base", &default_branch_ref, expected_head_sha])?
        .trim()
        .to_string();
    let merge_tree_sha = write_merge_tree(repo, &repo.default_branch, expected_head_sha)
        .with_context(|| format!("merge branch `{branch_name}` into {}", repo.default_branch))?;
    let merge_commit_sha = create_merge_commit(
        repo,
        &merge_tree_sha,
        &default_head_sha,
        expected_head_sha,
        branch_name,
        &repo.default_branch,
    )?;
    publish_merge_refs(
        repo,
        branch_name,
        expected_head_sha,
        &default_head_sha,
        &merge_commit_sha,
    )
    .with_context(|| format!("publish merge branch `{branch_name}`"))?;

    Ok(MergeOutcome {
        head_sha: current_head,
        merge_base_sha,
        merge_commit_sha,
    })
}

pub fn close_branch(
    repo: &ForgeRepoConfig,
    branch_name: &str,
    expected_head_sha: &str,
) -> anyhow::Result<CloseOutcome> {
    let head_sha = resolve_ref(repo, &format!("refs/heads/{branch_name}"))?
        .ok_or_else(|| anyhow!("branch `{branch_name}` no longer exists"))?;
    if head_sha != expected_head_sha {
        bail!(
            "branch head changed from {} to {}",
            expected_head_sha,
            head_sha
        );
    }
    git_bare(
        repo,
        [
            "update-ref",
            "-d",
            &format!("refs/heads/{branch_name}"),
            expected_head_sha,
        ],
    )
    .with_context(|| format!("delete branch `{branch_name}`"))?;

    Ok(CloseOutcome {
        deleted: true,
        head_sha: Some(head_sha),
    })
}

pub fn run_ci_command_for_head_with_heartbeat<F, G>(
    repo: &ForgeRepoConfig,
    head_sha: &str,
    command: &[String],
    structured_ci_target_id: Option<&str>,
    heartbeat_interval: Duration,
    mut heartbeat: F,
    mut on_event: G,
) -> anyhow::Result<CiExecutionResult>
where
    F: FnMut() -> anyhow::Result<()>,
    G: FnMut(CiExecutionEvent) -> anyhow::Result<()>,
{
    if command.is_empty() {
        bail!("ci command is empty");
    }
    let structured_ci_target = structured_ci_target_id
        .map(ToOwned::to_owned)
        .or_else(|| staged_ci_target_from_command(command));
    let structured_ci_state_root = structured_ci_target.as_ref().map(|_| {
        JerichociRunStore::from_forge_repo(repo)
            .state_root()
            .to_path_buf()
    });
    let effective_command = if structured_ci_target.is_some() {
        ensure_structured_ci_output(command)?
    } else {
        command.to_vec()
    };
    let worktree = temp_worktree(repo, head_sha)?;
    let ci_result = (|| {
        let mut cmd = Command::new(&effective_command[0]);
        cmd.args(&effective_command[1..])
            .current_dir(worktree.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(state_root) = structured_ci_state_root.as_ref() {
            fs::create_dir_all(state_root)
                .with_context(|| format!("create ci state root {}", state_root.display()))?;
            cmd.env("JERICHOCI_STATE_ROOT", state_root);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("run ci command in {}", worktree.path().display()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing ci stdout pipe"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("missing ci stderr pipe"))?;
        let stdout_log = Arc::new(Mutex::new(String::new()));
        let stderr_log = Arc::new(Mutex::new(String::new()));
        let (event_tx, event_rx) = mpsc::channel::<CiExecutionEvent>();
        let stdout_log_for_thread = Arc::clone(&stdout_log);
        let structured_ci_target_for_stdout = structured_ci_target.clone();
        let stdout_reader = thread::spawn(move || -> anyhow::Result<()> {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let line = line.context("read ci stdout line")?;
                let rendered =
                    render_ci_stdout_line(&line, structured_ci_target_for_stdout.as_deref());
                if let Some(event) = parse_ci_execution_event(&line) {
                    let _ = event_tx.send(event);
                }
                let mut log = stdout_log_for_thread
                    .lock()
                    .map_err(|_| anyhow!("lock ci stdout log"))?;
                log.push_str(&rendered);
                log.push('\n');
            }
            Ok(())
        });
        let stderr_log_for_thread = Arc::clone(&stderr_log);
        let stderr_reader = thread::spawn(move || -> anyhow::Result<()> {
            let mut raw = String::new();
            BufReader::new(stderr)
                .read_to_string(&mut raw)
                .context("read ci stderr")?;
            let mut log = stderr_log_for_thread
                .lock()
                .map_err(|_| anyhow!("lock ci stderr log"))?;
            log.push_str(&raw);
            Ok(())
        });

        let mut ci_run_id = None;
        let mut ci_target_id = structured_ci_target;
        heartbeat()?;
        let poll_interval = Duration::from_millis(250);
        let mut next_heartbeat = Instant::now() + heartbeat_interval;
        let status = loop {
            while let Ok(event) = event_rx.try_recv() {
                match event {
                    CiExecutionEvent::CiRunStarted { run_id, target_id } => {
                        ci_run_id = Some(run_id.clone());
                        if target_id.is_some() {
                            ci_target_id = target_id.clone();
                        }
                        if let Err(err) =
                            on_event(CiExecutionEvent::CiRunStarted { run_id, target_id })
                        {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(err);
                        }
                    }
                }
            }
            let polled = child
                .try_wait()
                .with_context(|| format!("poll ci command in {}", worktree.path().display()));
            match polled {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(err) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err);
                }
            }
            if Instant::now() >= next_heartbeat {
                if let Err(err) = heartbeat() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err);
                }
                next_heartbeat = Instant::now() + heartbeat_interval;
            }
            thread::sleep(poll_interval);
        };
        while let Ok(event) = event_rx.try_recv() {
            match event {
                CiExecutionEvent::CiRunStarted { run_id, target_id } => {
                    ci_run_id = Some(run_id.clone());
                    if target_id.is_some() {
                        ci_target_id = target_id.clone();
                    }
                    on_event(CiExecutionEvent::CiRunStarted { run_id, target_id })?;
                }
            }
        }
        stdout_reader
            .join()
            .map_err(|_| anyhow!("join ci stdout reader"))??;
        stderr_reader
            .join()
            .map_err(|_| anyhow!("join ci stderr reader"))??;
        let mut log = stdout_log
            .lock()
            .map_err(|_| anyhow!("lock ci stdout log"))?
            .clone();
        let stderr_log = stderr_log
            .lock()
            .map_err(|_| anyhow!("lock ci stderr log"))?
            .clone();
        if !stderr_log.is_empty() {
            if !log.is_empty() && !log.ends_with('\n') {
                log.push('\n');
            }
            log.push_str(&stderr_log);
        }
        Ok(CiExecutionResult {
            success: status.success(),
            log,
            ci_run_id,
            ci_target_id,
        })
    })();
    let cleanup_result = remove_temp_worktree(repo, worktree);
    match (ci_result, cleanup_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(_), Err(cleanup_err)) => Err(cleanup_err),
        (Err(err), Ok(())) => Err(err),
        (Err(err), Err(cleanup_err)) => Err(err.context(format!(
            "cleanup after failed ci run also failed: {}",
            cleanup_err
        ))),
    }
}

pub fn sync_mirror(
    repo: &ForgeRepoConfig,
    remote_name: &str,
    github_token: Option<&str>,
    trigger_source: &str,
) -> anyhow::Result<MirrorSyncOutcome> {
    let _mirror_lock = match acquire_mirror_lock(repo, trigger_source) {
        Ok(lock) => lock,
        Err(lock_err) => {
            if let Ok(outcome) = inspect_mirror(repo, remote_name) {
                if outcome.lagging_ref_count == 0 {
                    return Err(lock_err
                        .context("mirror is already in sync; treating this trigger as obsolete"));
                }
            }
            return Err(lock_err);
        }
    };
    push_mirror(repo, remote_name, github_token)?;
    inspect_mirror(repo, remote_name)
}

pub fn inspect_mirror(
    repo: &ForgeRepoConfig,
    remote_name: &str,
) -> anyhow::Result<MirrorSyncOutcome> {
    let local_heads = local_head_refs(repo)?;
    let remote_heads = remote_head_refs(repo, remote_name)?;
    let default_ref = format!("refs/heads/{}", repo.default_branch);
    let local_default_head = local_heads.get(&default_ref).cloned();
    let remote_default_head = remote_heads.get(&default_ref).cloned();

    let mut lagging = 0_i64;
    let mut synced = 0_i64;
    let ref_names = local_heads
        .keys()
        .chain(remote_heads.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for ref_name in ref_names {
        let local = local_heads.get(&ref_name);
        let remote = remote_heads.get(&ref_name);
        if local == remote {
            synced += 1;
        } else {
            lagging += 1;
        }
    }

    Ok(MirrorSyncOutcome {
        remote_name: remote_name.to_string(),
        local_default_head,
        remote_default_head,
        lagging_ref_count: lagging,
        synced_ref_count: synced,
    })
}

fn describe_branch(
    repo: &ForgeRepoConfig,
    branch_name: &str,
    head_sha: &str,
) -> anyhow::Result<CanonicalBranch> {
    let metadata = git_bare(
        repo,
        [
            "show",
            "-s",
            "--format=%H%x00%s%x00%an%x00%ae%x00%aI",
            head_sha,
        ],
    )?;
    let parts: Vec<&str> = metadata.trim_end().split('\0').collect();
    if parts.len() != 5 {
        bail!("unexpected commit metadata format for branch `{branch_name}`");
    }
    let merge_base_sha = git_bare(
        repo,
        [
            "merge-base",
            &format!("refs/heads/{}", repo.default_branch),
            head_sha,
        ],
    )?
    .trim()
    .to_string();

    Ok(CanonicalBranch {
        branch_name: branch_name.to_string(),
        head_sha: parts[0].to_string(),
        title: parts[1].to_string(),
        author_name: some_if_non_empty(parts[2]),
        author_email: some_if_non_empty(parts[3]),
        updated_at: parts[4].to_string(),
        merge_base_sha,
    })
}

fn canonical_git_dir(repo: &ForgeRepoConfig) -> PathBuf {
    PathBuf::from(&repo.canonical_git_dir)
}

fn mirror_timeout(repo: &ForgeRepoConfig) -> Duration {
    Duration::from_secs(
        repo.mirror_timeout_secs
            .unwrap_or(DEFAULT_MIRROR_TIMEOUT_SECS)
            .max(1),
    )
}

fn mirror_lock_path(repo: &ForgeRepoConfig) -> PathBuf {
    canonical_git_dir(repo).join("pika-git-mirror.lock")
}

pub fn current_mirror_lock_status(repo: &ForgeRepoConfig) -> Option<MirrorLockStatus> {
    let path = mirror_lock_path(repo);
    let file = OpenOptions::new().read(true).write(true).open(&path).ok()?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = file.unlock();
            None
        }
        Err(err) if err.kind() == ErrorKind::WouldBlock => {
            let timeout = mirror_timeout(repo);
            let metadata = read_mirror_lock_metadata(&file).ok().flatten();
            Some(mirror_lock_status_from_metadata(metadata, timeout))
        }
        Err(_) => None,
    }
}

fn acquire_mirror_lock(
    repo: &ForgeRepoConfig,
    trigger_source: &str,
) -> anyhow::Result<MirrorLockGuard> {
    let path = mirror_lock_path(repo);
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open mirror lock file {}", path.display()))?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            let metadata = MirrorLockMetadata {
                pid: std::process::id(),
                trigger_source: trigger_source.to_string(),
                operation: "git push --prune mirror".to_string(),
                started_at: Utc::now().to_rfc3339(),
                started_at_unix_secs: Utc::now().timestamp(),
            };
            write_mirror_lock_metadata(&mut file, &metadata)?;
            Ok(MirrorLockGuard { file })
        }
        Err(err) if err.kind() == ErrorKind::WouldBlock => {
            let status = mirror_lock_status_from_metadata(
                read_mirror_lock_metadata(&file).ok().flatten(),
                mirror_timeout(repo),
            );
            let trigger = status
                .trigger_source
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let age = status
                .age_secs
                .map(|secs| format!("{secs}s"))
                .unwrap_or_else(|| "unknown age".to_string());
            let pid = status
                .pid
                .map(|value| format!("pid {value}"))
                .unwrap_or_else(|| "unknown pid".to_string());
            if status.state == "stale" {
                bail!(
                    "stale mirror sync still holds repo lock ({pid}, trigger {trigger}, elapsed {age})"
                );
            }
            bail!("mirror sync already running ({pid}, trigger {trigger}, elapsed {age})");
        }
        Err(err) => Err(err).with_context(|| format!("lock mirror sync file {}", path.display())),
    }
}

fn write_mirror_lock_metadata(
    file: &mut std::fs::File,
    metadata: &MirrorLockMetadata,
) -> anyhow::Result<()> {
    file.set_len(0).context("truncate mirror lock file")?;
    file.seek(SeekFrom::Start(0))
        .context("rewind mirror lock file")?;
    serde_json::to_writer(&mut *file, metadata).context("write mirror lock metadata")?;
    file.write_all(b"\n")
        .context("terminate mirror lock metadata")?;
    file.sync_data().context("flush mirror lock metadata")?;
    Ok(())
}

fn read_mirror_lock_metadata(file: &std::fs::File) -> anyhow::Result<Option<MirrorLockMetadata>> {
    let mut reader = file
        .try_clone()
        .context("clone mirror lock file for metadata read")?;
    reader
        .seek(SeekFrom::Start(0))
        .context("rewind mirror lock file for metadata read")?;
    let mut raw = String::new();
    reader
        .read_to_string(&mut raw)
        .context("read mirror lock metadata")?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let metadata = serde_json::from_str(raw.trim()).context("parse mirror lock metadata")?;
    Ok(Some(metadata))
}

fn mirror_lock_status_from_metadata(
    metadata: Option<MirrorLockMetadata>,
    timeout: Duration,
) -> MirrorLockStatus {
    let age_secs = metadata.as_ref().and_then(|meta| {
        let elapsed = Utc::now().timestamp() - meta.started_at_unix_secs;
        (elapsed >= 0).then_some(elapsed as u64)
    });
    let is_stale = age_secs.map(|age| age > timeout.as_secs()).unwrap_or(false);
    MirrorLockStatus {
        state: if is_stale {
            "stale".to_string()
        } else {
            "active".to_string()
        },
        pid: metadata.as_ref().map(|meta| meta.pid),
        trigger_source: metadata.as_ref().map(|meta| meta.trigger_source.clone()),
        operation: metadata.as_ref().map(|meta| meta.operation.clone()),
        started_at: metadata.as_ref().map(|meta| meta.started_at.clone()),
        age_secs,
    }
}

fn effective_hooks_dir(repo: &ForgeRepoConfig) -> anyhow::Result<PathBuf> {
    let git_dir = canonical_git_dir(repo);
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(&git_dir)
        .args(["config", "--get", "core.hooksPath"])
        .output()
        .context("resolve git hooksPath for canonical bare repo")?;
    if !output.status.success() {
        return Ok(git_dir.join("hooks"));
    }
    let configured = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if configured.is_empty() {
        return Ok(git_dir.join("hooks"));
    }
    let path = PathBuf::from(configured);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(git_dir.join(path))
    }
}

fn resolve_hook_tool(name: &str) -> anyhow::Result<String> {
    let path = env::var_os("PATH").ok_or_else(|| anyhow!("PATH is not set"))?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    bail!("required hook tool `{name}` was not found on PATH");
}

fn push_mirror(
    repo: &ForgeRepoConfig,
    remote_name: &str,
    github_token: Option<&str>,
) -> anyhow::Result<()> {
    let mut cmd = Command::new("git");
    cmd.arg("--git-dir").arg(canonical_git_dir(repo));
    cmd.args(["-c", "gc.auto=0", "-c", "maintenance.auto=0"]);
    let remote_url = remote_url(repo, remote_name)?;
    if remote_url.contains("github.com") {
        if let Some(token) = github_token {
            cmd.args([
                "-c",
                &format!("http.extraheader={}", github_http_auth_header(token)),
            ]);
        }
    }
    cmd.args(["push", "--prune", remote_name, "refs/heads/*:refs/heads/*"]);
    let output = run_command_output_bounded(
        cmd,
        mirror_timeout(repo),
        &format!("push canonical refs to mirror remote `{remote_name}`"),
    )?;
    decode_git_output(output).map(|_| ())
}

pub fn mirror_remote_url(repo: &ForgeRepoConfig, remote_name: &str) -> anyhow::Result<String> {
    remote_url(repo, remote_name)
}

fn remote_url(repo: &ForgeRepoConfig, remote_name: &str) -> anyhow::Result<String> {
    git_bare(repo, ["remote", "get-url", remote_name])
        .map(|url| url.trim().to_string())
        .with_context(|| format!("resolve mirror remote `{remote_name}`"))
}

fn github_http_auth_header(token: &str) -> String {
    let credentials = format!("x-access-token:{token}");
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, credentials);
    format!("AUTHORIZATION: basic {encoded}")
}

fn local_head_refs(
    repo: &ForgeRepoConfig,
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let output = git_bare(
        repo,
        [
            "for-each-ref",
            "--format=%(refname)%00%(objectname)",
            "refs/heads",
        ],
    )?;
    parse_ref_map(&output)
}

fn remote_head_refs(
    repo: &ForgeRepoConfig,
    remote_name: &str,
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let output = git_bare_bounded(
        repo,
        mirror_timeout(repo),
        ["ls-remote", "--heads", remote_name],
        &format!("list remote heads for mirror `{remote_name}`"),
    )?;
    let mut refs = std::collections::BTreeMap::new();
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        let Some(sha) = parts.next() else { continue };
        let Some(ref_name) = parts.next() else {
            continue;
        };
        refs.insert(ref_name.to_string(), sha.to_string());
    }
    Ok(refs)
}

fn parse_ref_map(raw: &str) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let mut refs = std::collections::BTreeMap::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((ref_name, sha)) = line.split_once('\0') else {
            bail!("unexpected ref map line `{line}`");
        };
        refs.insert(ref_name.to_string(), sha.to_string());
    }
    Ok(refs)
}

fn resolve_ref(repo: &ForgeRepoConfig, git_ref: &str) -> anyhow::Result<Option<String>> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(canonical_git_dir(repo))
        .arg("rev-parse")
        .arg("--verify")
        .arg(git_ref)
        .output()
        .with_context(|| format!("resolve git ref `{git_ref}`"))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

fn temp_worktree(repo: &ForgeRepoConfig, start_point: &str) -> anyhow::Result<TempDir> {
    let worktree = TempDir::new().context("create temp worktree dir")?;
    git_bare(
        repo,
        [
            "worktree",
            "add",
            "--detach",
            worktree.path().to_str().unwrap_or(""),
            start_point,
        ],
    )
    .with_context(|| format!("create temporary worktree at {}", worktree.path().display()))?;
    Ok(worktree)
}

fn remove_temp_worktree(repo: &ForgeRepoConfig, worktree: TempDir) -> anyhow::Result<()> {
    let worktree_path = worktree.path().to_path_buf();
    git_bare(
        repo,
        [
            "worktree",
            "remove",
            "--force",
            worktree_path.to_str().unwrap_or(""),
        ],
    )
    .with_context(|| format!("remove temporary worktree {}", worktree_path.display()))?;
    Ok(())
}

fn git_bare<const N: usize>(repo: &ForgeRepoConfig, args: [&str; N]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(canonical_git_dir(repo))
        .args(args)
        .output()
        .context("run git command against canonical bare repo")?;
    decode_git_output(output)
}

fn git_bare_bounded<const N: usize>(
    repo: &ForgeRepoConfig,
    timeout: Duration,
    args: [&str; N],
    context: &str,
) -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("--git-dir")
        .arg(canonical_git_dir(repo))
        .args(["-c", "gc.auto=0", "-c", "maintenance.auto=0"])
        .args(args);
    let output = run_command_output_bounded(cmd, timeout, context)?;
    decode_git_output(output)
}

fn run_command_output_bounded(
    mut cmd: Command,
    timeout: Duration,
    context: &str,
) -> anyhow::Result<Output> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().with_context(|| context.to_string())?;
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .with_context(|| format!("poll {context}"))?
            .is_some()
        {
            return child
                .wait_with_output()
                .with_context(|| format!("collect output for {context}"));
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .with_context(|| format!("collect timed-out output for {context}"))?;
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                bail!("{context} timed out after {}s", timeout.as_secs());
            }
            bail!(
                "{context} timed out after {}s: {}",
                timeout.as_secs(),
                stderr
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn write_merge_tree(
    repo: &ForgeRepoConfig,
    default_branch: &str,
    expected_head_sha: &str,
) -> anyhow::Result<String> {
    let default_branch_ref = format!("refs/heads/{default_branch}");
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(canonical_git_dir(repo))
        .args([
            "merge-tree",
            "--write-tree",
            &default_branch_ref,
            expected_head_sha,
        ])
        .output()
        .context("compute merge tree in canonical bare repo")?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let details = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else if stdout.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            format!("{}\n{}", stdout.trim(), stderr.trim())
        };
        bail!("git merge-tree failed: {}", details);
    }
    stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .ok_or_else(|| anyhow!("git merge-tree returned no tree id"))
}

fn create_merge_commit(
    repo: &ForgeRepoConfig,
    merge_tree_sha: &str,
    default_head_sha: &str,
    expected_head_sha: &str,
    branch_name: &str,
    default_branch: &str,
) -> anyhow::Result<String> {
    let message = format!("Merge branch '{branch_name}' into {default_branch}");
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(canonical_git_dir(repo))
        .env("GIT_AUTHOR_NAME", "Pika Forge")
        .env("GIT_AUTHOR_EMAIL", "forge@git.pikachat.org")
        .env("GIT_COMMITTER_NAME", "Pika Forge")
        .env("GIT_COMMITTER_EMAIL", "forge@git.pikachat.org")
        .args([
            "commit-tree",
            merge_tree_sha,
            "-p",
            default_head_sha,
            "-p",
            expected_head_sha,
            "-m",
            &message,
        ])
        .output()
        .context("create merge commit in canonical bare repo")?;
    let commit_sha = decode_git_output(output)?;
    let trimmed = commit_sha.trim();
    if trimmed.is_empty() {
        bail!("git commit-tree returned no commit id");
    }
    Ok(trimmed.to_string())
}

fn publish_merge_refs(
    repo: &ForgeRepoConfig,
    branch_name: &str,
    expected_head_sha: &str,
    default_head_sha: &str,
    merge_commit_sha: &str,
) -> anyhow::Result<()> {
    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(canonical_git_dir(repo))
        .args(["update-ref", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start atomic merge ref update")?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("failed to open git update-ref stdin"))?;
        writeln!(stdin, "start")?;
        writeln!(
            stdin,
            "update refs/heads/{} {} {}",
            repo.default_branch, merge_commit_sha, default_head_sha
        )?;
        writeln!(
            stdin,
            "delete refs/heads/{} {}",
            branch_name, expected_head_sha
        )?;
        writeln!(stdin, "prepare")?;
        writeln!(stdin, "commit")?;
    }
    let output = child
        .wait_with_output()
        .context("finish atomic merge ref update")?;
    decode_git_output(output).map(|_| ())
}

fn decode_git_output(output: Output) -> anyhow::Result<String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git command failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write_hook(path: &Path, contents: &str) -> anyhow::Result<()> {
    fs::write(path, contents).with_context(|| format!("write hook {}", path.display()))?;
    let mut perms = fs::metadata(path)
        .with_context(|| format!("read hook metadata for {}", path.display()))?
        .permissions();
    perms.set_mode(0o750);
    fs::set_permissions(path, perms)
        .with_context(|| format!("set executable permissions on {}", path.display()))?;
    Ok(())
}

fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\"'\"'"))
}

fn some_if_non_empty(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn staged_ci_target_from_command(command: &[String]) -> Option<String> {
    let executable = command.first()?;
    let file_name = Path::new(executable).file_name()?.to_str()?;
    if file_name != "pikaci-staged-linux-remote.sh" {
        return None;
    }
    if command.get(1).map(String::as_str) != Some("run") {
        return None;
    }
    command.get(2).cloned()
}

fn ensure_structured_ci_output(command: &[String]) -> anyhow::Result<Vec<String>> {
    if let Some(output_index) = command.iter().position(|arg| arg == "--output") {
        let Some(output_value) = command.get(output_index + 1) else {
            bail!("structured ci command passed `--output` without a value");
        };
        if output_value != "jsonl" {
            bail!("structured ci command must use `--output jsonl`, got `--output {output_value}`");
        }
        return Ok(command.to_vec());
    }
    let mut structured = command.to_vec();
    structured.push("--output".to_string());
    structured.push("jsonl".to_string());
    Ok(structured)
}

fn parse_ci_execution_event(line: &str) -> Option<CiExecutionEvent> {
    match serde_json::from_str::<RunLifecycleEvent>(line).ok()? {
        RunLifecycleEvent::RunStarted {
            run_id, target_id, ..
        } => Some(CiExecutionEvent::CiRunStarted { run_id, target_id }),
        _ => None,
    }
}

fn render_ci_stdout_line(line: &str, default_target_id: Option<&str>) -> String {
    let Ok(event) = serde_json::from_str::<RunLifecycleEvent>(line) else {
        return line.to_string();
    };
    match event {
        RunLifecycleEvent::RunStarted {
            run_id,
            target_id,
            target_description,
            ..
        } => {
            let target = target_id
                .or_else(|| default_target_id.map(str::to_string))
                .unwrap_or_else(|| "unknown-target".to_string());
            match target_description {
                Some(description) => {
                    format!("[ci] run started: {run_id} · {target} · {description}")
                }
                None => format!("[ci] run started: {run_id} · {target}"),
            }
        }
        RunLifecycleEvent::JobStarted { job, .. } => {
            format!("[ci] job started: {} · {}", job.id, job.description)
        }
        RunLifecycleEvent::JobFinished { job, .. } => {
            let mut line = format!(
                "[ci] job finished: {} · status={}",
                job.id,
                render_run_status(job.status)
            );
            if let Some(message) = job
                .message
                .as_deref()
                .filter(|message| !message.trim().is_empty())
            {
                line.push_str(" · ");
                line.push_str(message.trim());
            }
            line
        }
        RunLifecycleEvent::RunFinished { run } => {
            let mut line = format!(
                "[ci] run finished: {} · status={}",
                run.run_id,
                render_run_status(run.status)
            );
            if let Some(message) = run
                .message
                .as_deref()
                .filter(|message| !message.trim().is_empty())
            {
                line.push_str(" · ");
                line.push_str(message.trim());
            }
            line
        }
    }
}

fn render_run_status(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Passed => "passed",
        RunStatus::Failed => "failed",
        RunStatus::Skipped => "skipped",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    use chrono::Utc;
    use jerichoci::RunStatus;

    use super::{
        acquire_mirror_lock, close_branch, create_merge_commit, current_branch_head,
        current_mirror_lock_status, github_http_auth_header, inspect_mirror, install_hooks,
        merge_branch, mirror_lock_path, publish_merge_refs, render_ci_stdout_line,
        resolve_hook_tool, run_ci_command_for_head_with_heartbeat, run_command_output_bounded,
        write_merge_tree, write_mirror_lock_metadata, MirrorLockMetadata,
    };
    use crate::config::ForgeRepoConfig;
    use crate::jerichoci_store::{
        JerichociRunStore, TestJerichociJobFixture, TestJerichociRunFixture,
    };

    fn git<P: AsRef<Path>>(cwd: P, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd.as_ref())
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn event_line(event: jerichoci::RunLifecycleEvent) -> String {
        serde_json::to_string(&event).expect("serialize test event")
    }

    fn setup_repo() -> (tempfile::TempDir, ForgeRepoConfig, std::path::PathBuf) {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        git(root.path(), &["init", seed.to_str().expect("seed path")]);
        git(&seed, &["config", "user.name", "Test User"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        std::fs::write(seed.join("README.md"), "hello\n").expect("write readme");
        git(&seed, &["add", "README.md"]);
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);

        let forge_repo = ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: bare.to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: Some(2),
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["./ci.sh".to_string()],
            hook_url: Some("http://127.0.0.1:9/git/webhook".to_string()),
        };
        (root, forge_repo, seed)
    }

    fn worktree_count(repo: &ForgeRepoConfig) -> usize {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(&repo.canonical_git_dir)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .expect("list worktrees");
        assert!(
            output.status.success(),
            "worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count()
    }

    #[test]
    fn merge_branch_deletes_source_ref() {
        let (_root, forge_repo, seed) = setup_repo();
        install_hooks(&forge_repo, "secret").expect("install hooks");

        git(&seed, &["checkout", "-b", "feature/test"]);
        std::fs::write(seed.join("feature.txt"), "hello from feature\n").expect("write feature");
        git(&seed, &["add", "feature.txt"]);
        git(&seed, &["commit", "-m", "feature work"]);
        git(&seed, &["push", "origin", "feature/test"]);

        let head_sha = current_branch_head(&forge_repo, "feature/test")
            .expect("resolve head")
            .expect("head");
        let outcome = merge_branch(&forge_repo, "feature/test", &head_sha).expect("merge branch");

        assert_eq!(outcome.head_sha, head_sha);
        assert!(current_branch_head(&forge_repo, "feature/test")
            .expect("resolve branch")
            .is_none());
        let master_head = current_branch_head(&forge_repo, "master")
            .expect("resolve master")
            .expect("master head");
        assert_eq!(master_head, outcome.merge_commit_sha);
    }

    #[test]
    fn update_hook_rejects_direct_master_pushes() {
        let (root, forge_repo, seed) = setup_repo();
        git(
            root.path(),
            &[
                "--git-dir",
                forge_repo.canonical_git_dir.as_str(),
                "config",
                "core.hooksPath",
                ".githooks",
            ],
        );
        install_hooks(&forge_repo, "secret").expect("install hooks");
        assert!(root.path().join("pika.git/.githooks/update").exists());
        assert!(root.path().join("pika.git/.githooks/post-receive").exists());

        std::fs::write(seed.join("README.md"), "updated\n").expect("update readme");
        git(&seed, &["add", "README.md"]);
        git(&seed, &["commit", "-m", "master change"]);
        let output = Command::new("git")
            .args(["push", "origin", "master"])
            .current_dir(&seed)
            .output()
            .expect("push master");
        assert!(
            !output.status.success(),
            "master push unexpectedly succeeded"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("direct pushes to master are rejected"),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn post_receive_hook_exports_nixos_safe_path() {
        let (_root, forge_repo, _seed) = setup_repo();
        install_hooks(&forge_repo, "secret").expect("install hooks");

        let hook = std::fs::read_to_string(
            Path::new(&forge_repo.canonical_git_dir).join("hooks/post-receive"),
        )
        .expect("read post-receive hook");
        assert!(hook.contains("/run/current-system/sw/bin"));
        assert!(hook.contains(&resolve_hook_tool("openssl").expect("resolve openssl")));
        assert!(hook.contains(&resolve_hook_tool("curl").expect("resolve curl")));
        assert!(hook.contains("x-pika-signature-256"));
    }

    #[test]
    fn close_branch_rejects_stale_head() {
        let (_root, forge_repo, seed) = setup_repo();
        install_hooks(&forge_repo, "secret").expect("install hooks");

        git(&seed, &["checkout", "-b", "feature/close"]);
        std::fs::write(seed.join("close.txt"), "v1\n").expect("write close v1");
        git(&seed, &["add", "close.txt"]);
        git(&seed, &["commit", "-m", "close v1"]);
        git(&seed, &["push", "origin", "feature/close"]);
        let old_head = current_branch_head(&forge_repo, "feature/close")
            .expect("resolve feature head")
            .expect("feature head");

        std::fs::write(seed.join("close.txt"), "v2\n").expect("write close v2");
        git(&seed, &["add", "close.txt"]);
        git(&seed, &["commit", "-m", "close v2"]);
        let output = Command::new("git")
            .args(["push", "origin", "HEAD:feature/close"])
            .current_dir(&seed)
            .output()
            .expect("push v2");
        assert!(
            output.status.success(),
            "push v2 failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let err = close_branch(&forge_repo, "feature/close", &old_head).expect_err("stale close");
        assert!(err.to_string().contains("branch head changed"));
        assert!(current_branch_head(&forge_repo, "feature/close")
            .expect("resolve current head")
            .is_some());
    }

    #[test]
    fn failed_merge_cleans_up_temp_worktree() {
        let (_root, forge_repo, seed) = setup_repo();

        std::fs::write(seed.join("README.md"), "master v2\n").expect("write master change");
        git(&seed, &["add", "README.md"]);
        git(&seed, &["commit", "-m", "master change"]);
        git(&seed, &["push", "origin", "master"]);
        install_hooks(&forge_repo, "secret").expect("install hooks");

        git(&seed, &["checkout", "HEAD~1"]);
        git(&seed, &["checkout", "-b", "feature/conflict"]);
        std::fs::write(seed.join("README.md"), "feature v2\n").expect("write feature change");
        git(&seed, &["add", "README.md"]);
        git(&seed, &["commit", "-m", "feature conflict"]);
        git(&seed, &["push", "origin", "feature/conflict"]);

        let before = worktree_count(&forge_repo);
        let head_sha = current_branch_head(&forge_repo, "feature/conflict")
            .expect("resolve feature head")
            .expect("feature head");
        let err = merge_branch(&forge_repo, "feature/conflict", &head_sha)
            .expect_err("merge should conflict");
        assert!(err.to_string().contains("merge branch `feature/conflict`"));
        assert_eq!(worktree_count(&forge_repo), before);
        assert!(current_branch_head(&forge_repo, "feature/conflict")
            .expect("resolve feature branch")
            .is_some());
    }

    #[test]
    fn stale_branch_after_prepared_merge_does_not_update_master() {
        let (_root, forge_repo, seed) = setup_repo();
        install_hooks(&forge_repo, "secret").expect("install hooks");

        git(&seed, &["checkout", "-b", "feature/race"]);
        std::fs::write(seed.join("feature.txt"), "v1\n").expect("write feature v1");
        git(&seed, &["add", "feature.txt"]);
        git(&seed, &["commit", "-m", "feature v1"]);
        git(&seed, &["push", "origin", "feature/race"]);

        let expected_head = current_branch_head(&forge_repo, "feature/race")
            .expect("resolve feature head")
            .expect("feature head");
        let default_head = current_branch_head(&forge_repo, "master")
            .expect("resolve master head")
            .expect("master head");
        let merge_tree =
            write_merge_tree(&forge_repo, "master", &expected_head).expect("write merge tree");
        let merge_commit = create_merge_commit(
            &forge_repo,
            &merge_tree,
            &default_head,
            &expected_head,
            "feature/race",
            "master",
        )
        .expect("create merge commit");

        std::fs::write(seed.join("feature.txt"), "v2\n").expect("write feature v2");
        git(&seed, &["add", "feature.txt"]);
        git(&seed, &["commit", "-m", "feature v2"]);
        git(&seed, &["push", "origin", "HEAD:feature/race"]);

        let err = publish_merge_refs(
            &forge_repo,
            "feature/race",
            &expected_head,
            &default_head,
            &merge_commit,
        )
        .expect_err("atomic publish should fail on stale branch ref");
        assert!(err.to_string().contains("cannot lock ref"));
        let master_after = current_branch_head(&forge_repo, "master")
            .expect("resolve master after failed publish")
            .expect("master after failed publish");
        assert_eq!(master_after, default_head);
    }

    #[test]
    fn failed_ci_spawn_cleans_up_temp_worktree() {
        let (_root, forge_repo, _seed) = setup_repo();
        let before = worktree_count(&forge_repo);
        let head_sha = current_branch_head(&forge_repo, "master")
            .expect("resolve master head")
            .expect("master head");

        let err = run_ci_command_for_head_with_heartbeat(
            &forge_repo,
            &head_sha,
            &["/definitely/missing/pika-ci-command".to_string()],
            None,
            Duration::from_secs(1),
            || Ok(()),
            |_| Ok(()),
        )
        .expect_err("ci command spawn should fail");

        assert!(err.to_string().contains("run ci command"));
        assert_eq!(worktree_count(&forge_repo), before);
    }

    #[test]
    fn staged_pikaci_lane_reports_run_id_and_human_log_summary() {
        let (_root, forge_repo, seed) = setup_repo();
        let run_store = JerichociRunStore::from_forge_repo(&forge_repo);
        let mut fixture = TestJerichociRunFixture::passed(
            "pikaci-run-123",
            Some("pre-merge-pika-rust"),
            Some("Run staged pika rust"),
        );
        fixture
            .jobs
            .push(TestJerichociJobFixture::passed_remote_linux(
                "job-one", "job one",
            ));
        run_store
            .write_fixture(&fixture)
            .expect("write persisted run fixture");
        let scripts_dir = seed.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("create scripts dir");
        let wrapper_path = scripts_dir.join("pikaci-staged-linux-remote.sh");
        let run_started = event_line(fixture.clone().run_started_event());
        let job_started = event_line(
            TestJerichociJobFixture::passed_remote_linux("job-one", "job one")
                .job_started_event("pikaci-run-123"),
        );
        let job_finished = event_line(
            TestJerichociJobFixture::passed_remote_linux("job-one", "job one")
                .job_finished_event("pikaci-run-123"),
        );
        let run_finished = event_line(fixture.clone().run_finished_event());
        fs::write(
            &wrapper_path,
            format!(
                "#!/usr/bin/env bash\n\
set -euo pipefail\n\
if [[ \"$1\" != \"run\" || \"$2\" != \"pre-merge-pika-rust\" || \"$3\" != \"--output\" || \"$4\" != \"jsonl\" ]]; then\n\
  echo \"unexpected args: $*\" >&2\n\
  exit 99\n\
fi\n\
cat <<'EOF'\n\
{run_started}\n\
{job_started}\n\
{job_finished}\n\
{run_finished}\n\
EOF\n"
            ),
        )
        .expect("write fake wrapper");
        let mut perms = fs::metadata(&wrapper_path)
            .expect("wrapper metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&wrapper_path, perms).expect("set wrapper perms");
        git(&seed, &["add", "scripts/pikaci-staged-linux-remote.sh"]);
        git(&seed, &["commit", "-m", "add fake staged wrapper"]);
        git(&seed, &["push", "origin", "master"]);

        let head_sha = current_branch_head(&forge_repo, "master")
            .expect("resolve master head")
            .expect("master head");
        let mut started_run_ids = Vec::new();
        let result = run_ci_command_for_head_with_heartbeat(
            &forge_repo,
            &head_sha,
            &[
                "./scripts/pikaci-staged-linux-remote.sh".to_string(),
                "run".to_string(),
                "pre-merge-pika-rust".to_string(),
            ],
            Some("pre-merge-pika-rust"),
            Duration::from_secs(1),
            || Ok(()),
            |event| {
                match event {
                    super::CiExecutionEvent::CiRunStarted { run_id, .. } => {
                        started_run_ids.push(run_id);
                    }
                }
                Ok(())
            },
        )
        .expect("run structured staged lane");

        assert!(result.success);
        assert_eq!(result.ci_run_id.as_deref(), Some("pikaci-run-123"));
        assert_eq!(result.ci_target_id.as_deref(), Some("pre-merge-pika-rust"));
        assert_eq!(started_run_ids, vec!["pikaci-run-123".to_string()]);
        assert!(result.log.contains("[ci] run started: pikaci-run-123"));
        assert!(result
            .log
            .contains("[ci] job finished: job-one · status=passed"));
        assert!(run_store.run_record_path("pikaci-run-123").is_file());
        assert!(run_store
            .host_log_path("pikaci-run-123", "job-one")
            .is_file());
    }

    #[test]
    fn explicit_structured_pikaci_target_does_not_depend_on_wrapper_name() {
        let (_root, forge_repo, seed) = setup_repo();
        let run_store = JerichociRunStore::from_forge_repo(&forge_repo);
        let fixture = TestJerichociRunFixture::passed(
            "pikaci-run-456",
            Some("pre-merge-pika-rust"),
            Some("Run staged pika rust"),
        );
        run_store
            .write_fixture(&fixture)
            .expect("write persisted run fixture");
        let scripts_dir = seed.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("create scripts dir");
        let wrapper_path = scripts_dir.join("custom-structured-lane.sh");
        let run_started = event_line(fixture.clone().run_started_event());
        let run_finished = event_line(fixture.clone().run_finished_event());
        fs::write(
            &wrapper_path,
            format!(
                "#!/usr/bin/env bash\n\
set -euo pipefail\n\
if [[ \"$1\" != \"run\" || \"$2\" != \"pre-merge-pika-rust\" || \"$3\" != \"--output\" || \"$4\" != \"jsonl\" ]]; then\n\
  echo \"unexpected args: $*\" >&2\n\
  exit 99\n\
fi\n\
cat <<'EOF'\n\
{run_started}\n\
{run_finished}\n\
EOF\n"
            ),
        )
        .expect("write fake wrapper");
        let mut perms = fs::metadata(&wrapper_path)
            .expect("wrapper metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&wrapper_path, perms).expect("set wrapper perms");
        git(&seed, &["add", "scripts/custom-structured-lane.sh"]);
        git(&seed, &["commit", "-m", "add custom structured wrapper"]);
        git(&seed, &["push", "origin", "master"]);

        let head_sha = current_branch_head(&forge_repo, "master")
            .expect("resolve master head")
            .expect("master head");
        let result = run_ci_command_for_head_with_heartbeat(
            &forge_repo,
            &head_sha,
            &[
                "./scripts/custom-structured-lane.sh".to_string(),
                "run".to_string(),
                "pre-merge-pika-rust".to_string(),
            ],
            Some("pre-merge-pika-rust"),
            Duration::from_secs(1),
            || Ok(()),
            |_| Ok(()),
        )
        .expect("run structured lane with explicit metadata");

        assert!(result.success);
        assert_eq!(result.ci_run_id.as_deref(), Some("pikaci-run-456"));
        assert_eq!(result.ci_target_id.as_deref(), Some("pre-merge-pika-rust"));
        assert!(run_store.run_record_path("pikaci-run-456").is_file());
    }

    #[test]
    fn explicit_structured_pikaci_target_rejects_non_jsonl_output() {
        let (_root, forge_repo, _seed) = setup_repo();
        let head_sha = current_branch_head(&forge_repo, "master")
            .expect("resolve master head")
            .expect("master head");

        let err = run_ci_command_for_head_with_heartbeat(
            &forge_repo,
            &head_sha,
            &[
                "./scripts/custom-structured-lane.sh".to_string(),
                "run".to_string(),
                "pre-merge-pika-rust".to_string(),
                "--output".to_string(),
                "json".to_string(),
            ],
            Some("pre-merge-pika-rust"),
            Duration::from_secs(1),
            || Ok(()),
            |_| Ok(()),
        )
        .expect_err("non-jsonl structured command should fail");

        assert!(err
            .to_string()
            .contains("structured ci command must use `--output jsonl`"));
    }

    #[test]
    fn structured_pikaci_failure_log_keeps_job_and_run_messages() {
        let job_line = render_ci_stdout_line(
            &event_line(
                TestJerichociJobFixture {
                    status: RunStatus::Failed,
                    exit_code: Some(1),
                    message: Some("cargo test failed in crate pika_core".to_string()),
                    ..TestJerichociJobFixture::passed_remote_linux("job-one", "job one")
                }
                .job_finished_event("pikaci-run-123"),
            ),
            Some("pre-merge-pika-rust"),
        );
        assert!(job_line.contains("status=failed"));
        assert!(job_line.contains("cargo test failed in crate pika_core"));

        let run_line = render_ci_stdout_line(
            &event_line(
                TestJerichociRunFixture {
                    status: RunStatus::Failed,
                    message: Some("prepared-output helper exited 1".to_string()),
                    ..TestJerichociRunFixture::passed(
                        "pikaci-run-123",
                        Some("pre-merge-pika-rust"),
                        Some("Run staged pika rust"),
                    )
                }
                .run_finished_event(),
            ),
            Some("pre-merge-pika-rust"),
        );
        assert!(run_line.contains("status=failed"));
        assert!(run_line.contains("prepared-output helper exited 1"));
    }

    #[test]
    fn inspect_mirror_counts_refs_once_per_name() {
        let (root, mut forge_repo, seed) = setup_repo();
        let mirror = root.path().join("mirror.git");
        git(
            root.path(),
            &["init", "--bare", mirror.to_str().expect("mirror path")],
        );
        git(
            root.path(),
            &[
                "--git-dir",
                &forge_repo.canonical_git_dir,
                "remote",
                "add",
                "github",
                mirror.to_str().expect("mirror path"),
            ],
        );
        git(
            &seed,
            &[
                "remote",
                "add",
                "github",
                mirror.to_str().expect("mirror path"),
            ],
        );
        git(&seed, &["push", "github", "master"]);
        forge_repo.mirror_remote = Some("github".to_string());

        git(&seed, &["checkout", "-b", "feature/mirror-counts"]);
        std::fs::write(seed.join("counts.txt"), "v1\n").expect("write counts");
        git(&seed, &["add", "counts.txt"]);
        git(&seed, &["commit", "-m", "counts"]);
        git(&seed, &["push", "origin", "feature/mirror-counts"]);

        let outcome = inspect_mirror(&forge_repo, "github").expect("inspect mirror");
        assert_eq!(outcome.synced_ref_count, 1);
        assert_eq!(outcome.lagging_ref_count, 1);
    }

    #[test]
    fn bounded_command_times_out_and_kills_child() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 2"]);
        let err = run_command_output_bounded(cmd, Duration::from_millis(100), "sleep command")
            .expect_err("command should time out");
        assert!(err.to_string().contains("timed out after"));
    }

    #[test]
    fn current_mirror_lock_status_reports_stale_lock() {
        let (_root, forge_repo, _seed) = setup_repo();
        let mut guard = acquire_mirror_lock(&forge_repo, "manual").expect("acquire mirror lock");
        let stale_metadata = MirrorLockMetadata {
            pid: 4242,
            trigger_source: "manual".to_string(),
            operation: "git push --prune mirror".to_string(),
            started_at: "2026-03-24T00:00:00Z".to_string(),
            started_at_unix_secs: Utc::now().timestamp() - 600,
        };
        write_mirror_lock_metadata(&mut guard.file, &stale_metadata)
            .expect("rewrite stale mirror lock metadata");

        let status = current_mirror_lock_status(&forge_repo).expect("lock status");
        assert_eq!(status.state, "stale");
        assert_eq!(status.trigger_source.as_deref(), Some("manual"));
        assert_eq!(status.pid, Some(4242));
    }

    #[test]
    fn mirror_lock_file_lives_in_canonical_git_dir() {
        let (_root, forge_repo, _seed) = setup_repo();
        let lock_path = mirror_lock_path(&forge_repo);
        assert!(lock_path.ends_with("pika-git-mirror.lock"));
        assert!(lock_path.starts_with(&forge_repo.canonical_git_dir));
    }

    #[test]
    fn github_http_auth_header_uses_basic_with_x_access_token_username() {
        let header = github_http_auth_header("test-token");
        assert_eq!(
            header,
            "AUTHORIZATION: basic eC1hY2Nlc3MtdG9rZW46dGVzdC10b2tlbg=="
        );
    }
}
