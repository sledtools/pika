use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use tempfile::TempDir;

use crate::config::ForgeRepoConfig;

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
pub struct CiExecutionResult {
    pub success: bool,
    pub log: String,
}

#[derive(Debug, Clone)]
pub struct MirrorSyncOutcome {
    pub remote_name: String,
    pub local_default_head: Option<String>,
    pub remote_default_head: Option<String>,
    pub lagging_ref_count: i64,
    pub synced_ref_count: i64,
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
    let git_dir = canonical_git_dir(repo);
    fs::create_dir_all(git_dir.join("hooks"))
        .with_context(|| format!("create hooks dir in {}", git_dir.display()))?;

    let update_hook = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nref_name=\"$1\"\nif [[ \"$ref_name\" == \"refs/heads/{default_branch}\" ]]; then\n  echo \"direct pushes to {default_branch} are rejected; merge through the forge\" >&2\n  exit 1\nfi\n",
        default_branch = repo.default_branch
    );
    write_hook(&git_dir.join("hooks/update"), &update_hook)?;

    let hook_url = repo
        .hook_url
        .as_deref()
        .ok_or_else(|| anyhow!("forge hook_url missing for {}", repo.repo))?;
    let post_receive_hook = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\npayload=\"$(mktemp)\"\ntrap 'rm -f \"$payload\"' EXIT\ncat >\"$payload\"\nif [[ ! -s \"$payload\" ]]; then\n  exit 0\nfi\nsignature=\"sha256=$(openssl dgst -sha256 -hmac {secret} -hex \"$payload\" | sed 's/^.*= //')\"\nif ! curl --silent --show-error --fail -X POST -H \"content-type: text/plain\" -H \"x-pika-signature-256: ${{signature}}\" --data-binary @\"$payload\" {url} >/dev/null; then\n  echo \"warning: failed to notify pika forge post-receive hook\" >&2\nfi\n",
        secret = shell_quote(secret),
        url = shell_quote(hook_url),
    );
    write_hook(&git_dir.join("hooks/post-receive"), &post_receive_hook)?;

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

pub fn read_file_at_ref(
    repo: &ForgeRepoConfig,
    git_ref: &str,
    relative_path: &str,
) -> anyhow::Result<String> {
    git_bare(repo, ["show", &format!("{git_ref}:{relative_path}")])
        .with_context(|| format!("read `{relative_path}` from `{git_ref}` in canonical bare repo"))
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

pub fn run_ci_command_for_head_with_heartbeat<F>(
    repo: &ForgeRepoConfig,
    head_sha: &str,
    command: &[String],
    heartbeat_interval: Duration,
    mut heartbeat: F,
) -> anyhow::Result<CiExecutionResult>
where
    F: FnMut() -> anyhow::Result<()>,
{
    if command.is_empty() {
        bail!("ci command is empty");
    }
    let worktree = temp_worktree(repo, head_sha)?;
    let ci_result = (|| {
        let stdout_path = worktree.path().join(".pika-ci.stdout.log");
        let stderr_path = worktree.path().join(".pika-ci.stderr.log");
        let stdout = fs::File::create(&stdout_path)
            .with_context(|| format!("create ci stdout log {}", stdout_path.display()))?;
        let stderr = fs::File::create(&stderr_path)
            .with_context(|| format!("create ci stderr log {}", stderr_path.display()))?;
        let mut cmd = Command::new(&command[0]);
        cmd.args(&command[1..])
            .current_dir(worktree.path())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let mut child = cmd
            .spawn()
            .with_context(|| format!("run ci command in {}", worktree.path().display()))?;
        heartbeat()?;
        let poll_interval = Duration::from_millis(250);
        let mut next_heartbeat = Instant::now() + heartbeat_interval;
        let status = loop {
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
        let stdout_bytes = fs::read(&stdout_path)
            .with_context(|| format!("read ci stdout log {}", stdout_path.display()))?;
        let stderr_bytes = fs::read(&stderr_path)
            .with_context(|| format!("read ci stderr log {}", stderr_path.display()))?;
        let mut log = String::from_utf8_lossy(&stdout_bytes).into_owned();
        if !stderr_bytes.is_empty() {
            if !log.is_empty() && !log.ends_with('\n') {
                log.push('\n');
            }
            log.push_str(&String::from_utf8_lossy(&stderr_bytes));
        }
        Ok(CiExecutionResult {
            success: status.success(),
            log,
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
) -> anyhow::Result<MirrorSyncOutcome> {
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

fn push_mirror(
    repo: &ForgeRepoConfig,
    remote_name: &str,
    github_token: Option<&str>,
) -> anyhow::Result<()> {
    let mut cmd = Command::new("git");
    cmd.arg("--git-dir").arg(canonical_git_dir(repo));
    let remote_url = remote_url(repo, remote_name)?;
    if remote_url.contains("github.com") {
        if let Some(token) = github_token {
            cmd.args([
                "-c",
                &format!("http.extraheader=Authorization: Bearer {token}"),
            ]);
        }
    }
    let output = cmd
        .args(["push", "--prune", remote_name, "refs/heads/*:refs/heads/*"])
        .output()
        .with_context(|| format!("push canonical refs to mirror remote `{remote_name}`"))?;
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
    let output = git_bare(repo, ["ls-remote", "--heads", remote_name])
        .with_context(|| format!("list remote heads for mirror `{remote_name}`"))?;
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    use super::{
        close_branch, create_merge_commit, current_branch_head, inspect_mirror, install_hooks,
        merge_branch, publish_merge_refs, run_ci_command_for_head_with_heartbeat, write_merge_tree,
    };
    use crate::config::ForgeRepoConfig;

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
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            ci_command: vec!["./ci.sh".to_string()],
            hook_url: Some("http://127.0.0.1:9/news/webhook".to_string()),
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
        let (_root, forge_repo, seed) = setup_repo();
        install_hooks(&forge_repo, "secret").expect("install hooks");

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
            Duration::from_secs(1),
            || Ok(()),
        )
        .expect_err("ci command spawn should fail");

        assert!(err.to_string().contains("run ci command"));
        assert_eq!(worktree_count(&forge_repo), before);
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
}
