use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotProfile {
    Full,
    StagedLinuxRust,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SnapshotMetadata {
    pub source_root: String,
    pub snapshot_dir: String,
    pub git_head: Option<String>,
    pub git_dirty: Option<bool>,
    pub created_at: String,
    #[serde(default)]
    pub content_hash: Option<String>,
}

pub fn create_snapshot_with_profile(
    source_root: &Path,
    snapshot_dir: &Path,
    created_at: &str,
    profile: SnapshotProfile,
) -> anyhow::Result<SnapshotMetadata> {
    copy_filtered_tree(source_root, snapshot_dir, profile)?;
    let content_hash = Some(compute_snapshot_content_hash(snapshot_dir)?);
    let metadata = SnapshotMetadata {
        source_root: source_root.display().to_string(),
        snapshot_dir: snapshot_dir.display().to_string(),
        git_head: git_head(source_root),
        git_dirty: git_dirty(source_root),
        created_at: created_at.to_string(),
        content_hash,
    };
    write_json(snapshot_dir.join("pikaci-snapshot.json"), &metadata)?;
    Ok(metadata)
}

pub fn materialize_workspace(source: &Path, destination: &Path) -> anyhow::Result<()> {
    copy_filtered_tree(source, destination, SnapshotProfile::Full)
}

pub fn compute_source_content_hash_with_profile(
    source_root: &Path,
    profile: SnapshotProfile,
) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hash_filtered_source_tree(source_root, source_root, true, profile, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

pub fn compute_source_fingerprint_with_profile(
    source_root: &Path,
    profile: SnapshotProfile,
) -> anyhow::Result<String> {
    let Some(git_head) = git_head(source_root) else {
        return compute_source_content_hash_with_profile(source_root, profile);
    };
    let Some(changed_files) = git_changed_files(source_root) else {
        return compute_source_content_hash_with_profile(source_root, profile);
    };
    let Some(ignored_files) = git_ignored_files(source_root) else {
        return compute_source_content_hash_with_profile(source_root, profile);
    };

    let mut relevant_files = changed_files;
    relevant_files.extend(ignored_files);
    let mut relevant_files = relevant_files
        .into_iter()
        .filter(|path| !should_skip_relative_path(Path::new(path), profile))
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    relevant_files.sort();

    let mut normalized_files = Vec::new();
    for path in relevant_files {
        if normalized_files
            .iter()
            .any(|existing: &PathBuf| path.starts_with(existing))
        {
            continue;
        }
        normalized_files.push(path);
    }

    let relevant_files = normalized_files
        .into_iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let mut hasher = Sha256::new();
    hasher.update(b"git-head\0");
    hasher.update(git_head.as_bytes());
    hasher.update(b"\0");
    for relative in relevant_files {
        let path = source_root.join(&relative);
        hash_source_fingerprint_entry(
            source_root,
            &path,
            Path::new(&relative),
            profile,
            &mut hasher,
        )?;
    }

    Ok(hex::encode(hasher.finalize()))
}

fn hash_source_fingerprint_entry(
    source_root: &Path,
    path: &Path,
    relative: &Path,
    profile: SnapshotProfile,
    hasher: &mut Sha256,
) -> anyhow::Result<()> {
    hasher.update(b"path\0");
    hasher.update(relative.as_os_str().as_encoded_bytes());
    hasher.update(b"\0");

    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            hasher.update(b"symlink\0");
            let target =
                fs::read_link(path).with_context(|| format!("read symlink {}", path.display()))?;
            hasher.update(target.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
        }
        Ok(metadata) if metadata.is_file() => {
            hasher.update(b"file\0");
            let mut file =
                fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
            let mut buffer = [0u8; 8192];
            loop {
                let read = file
                    .read(&mut buffer)
                    .with_context(|| format!("read {}", path.display()))?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            hasher.update(b"\0");
        }
        Ok(metadata) if metadata.is_dir() => {
            hasher.update(b"dir\0");
            hasher.update(b"\0");
            hash_filtered_source_tree(
                source_root,
                path,
                relative == Path::new(""),
                profile,
                hasher,
            )?;
        }
        Ok(_) => {
            return Err(anyhow!("unsupported filesystem entry: {}", path.display()));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            hasher.update(b"missing\0");
            hasher.update(b"\0");
        }
        Err(error) => {
            return Err(error).with_context(|| format!("stat {}", path.display()));
        }
    }

    Ok(())
}

fn copy_filtered_tree(
    source: &Path,
    destination: &Path,
    profile: SnapshotProfile,
) -> anyhow::Result<()> {
    copy_tree(source, destination, true, profile)
}

fn copy_tree(
    source: &Path,
    destination: &Path,
    root: bool,
    profile: SnapshotProfile,
) -> anyhow::Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("create snapshot dir {}", destination.display()))?;

    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let source_path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if should_skip(&file_name, root, profile) {
            continue;
        }

        let destination_path = destination.join(file_name.as_ref());
        let metadata = fs::symlink_metadata(&source_path)
            .with_context(|| format!("stat {}", source_path.display()))?;
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&source_path)
                .with_context(|| format!("read symlink {}", source_path.display()))?;
            unix_fs::symlink(&target, &destination_path).with_context(|| {
                format!(
                    "recreate symlink {} -> {}",
                    destination_path.display(),
                    target.display()
                )
            })?;
        } else if metadata.is_dir() {
            copy_tree(&source_path, &destination_path, false, profile)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "copy file {} -> {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        } else {
            return Err(anyhow!(
                "unsupported filesystem entry: {}",
                source_path.display()
            ));
        }
    }

    Ok(())
}

fn should_skip(name: &str, root: bool, profile: SnapshotProfile) -> bool {
    matches!(name, ".git" | ".pikaci" | ".direnv")
        || name == "target"
        || (root
            && profile == SnapshotProfile::StagedLinuxRust
            && matches!(name, "ios" | "android"))
        || (!root && matches!(name, "node_modules" | ".gradle" | "DerivedData" | "build"))
}

fn should_skip_relative_path(path: &Path, profile: SnapshotProfile) -> bool {
    let mut root = true;
    for component in path.components() {
        let std::path::Component::Normal(component) = component else {
            continue;
        };
        if should_skip(&component.to_string_lossy(), root, profile) {
            return true;
        }
        root = false;
    }
    false
}

pub fn git_head(source_root: &Path) -> Option<String> {
    run_git(source_root, &["rev-parse", "HEAD"])
}

pub fn git_dirty(source_root: &Path) -> Option<bool> {
    let output = run_git(
        source_root,
        &["status", "--short", "--untracked-files=normal"],
    )?;
    Some(!output.trim().is_empty())
}

pub fn git_changed_files(source_root: &Path) -> Option<Vec<String>> {
    let tracked = run_git_lines(
        source_root,
        &["diff", "--name-only", "--relative", "HEAD", "--"],
    )?;
    let untracked = run_git_lines(source_root, &["ls-files", "--others", "--exclude-standard"])?;

    let mut files = BTreeSet::new();
    files.extend(tracked);
    files.extend(untracked);
    Some(files.into_iter().collect())
}

pub fn git_ignored_files(source_root: &Path) -> Option<Vec<String>> {
    run_git_lines(
        source_root,
        &["ls-files", "--others", "--ignored", "--exclude-standard"],
    )
}

fn run_git(source_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(source_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn run_git_lines(source_root: &Path, args: &[&str]) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(source_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    Some(
        stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

fn write_json(path: PathBuf, value: &impl Serialize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).context("encode snapshot metadata")?;
    fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))
}

pub fn read_snapshot_metadata(snapshot_dir: &Path) -> anyhow::Result<SnapshotMetadata> {
    let path = snapshot_dir.join("pikaci-snapshot.json");
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))
}

fn compute_snapshot_content_hash(snapshot_dir: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hash_snapshot_tree(snapshot_dir, snapshot_dir, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

fn hash_filtered_source_tree(
    root: &Path,
    current: &Path,
    current_is_root: bool,
    profile: SnapshotProfile,
    hasher: &mut Sha256,
) -> anyhow::Result<()> {
    let mut entries = fs::read_dir(current)
        .with_context(|| format!("read {}", current.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("collect {}", current.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if should_skip(&name, current_is_root, profile) {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .with_context(|| format!("strip {} from {}", root.display(), path.display()))?;
        let metadata =
            fs::symlink_metadata(&path).with_context(|| format!("stat {}", path.display()))?;

        if metadata.file_type().is_symlink() {
            hasher.update(b"symlink\0");
            hasher.update(relative.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
            let target =
                fs::read_link(&path).with_context(|| format!("read symlink {}", path.display()))?;
            hasher.update(target.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
        } else if metadata.is_dir() {
            hasher.update(b"dir\0");
            hasher.update(relative.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
            hash_filtered_source_tree(root, &path, false, profile, hasher)?;
        } else if metadata.is_file() {
            hasher.update(b"file\0");
            hasher.update(relative.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
            let mut file =
                fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
            let mut buffer = [0u8; 8192];
            loop {
                let read = file
                    .read(&mut buffer)
                    .with_context(|| format!("read {}", path.display()))?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            hasher.update(b"\0");
        } else {
            return Err(anyhow!("unsupported filesystem entry: {}", path.display()));
        }
    }

    Ok(())
}

fn hash_snapshot_tree(root: &Path, current: &Path, hasher: &mut Sha256) -> anyhow::Result<()> {
    let mut entries = fs::read_dir(current)
        .with_context(|| format!("read {}", current.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("collect {}", current.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if current == root && name == "pikaci-snapshot.json" {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .with_context(|| format!("strip {} from {}", root.display(), path.display()))?;
        let metadata =
            fs::symlink_metadata(&path).with_context(|| format!("stat {}", path.display()))?;

        if metadata.file_type().is_symlink() {
            hasher.update(b"symlink\0");
            hasher.update(relative.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
            let target =
                fs::read_link(&path).with_context(|| format!("read symlink {}", path.display()))?;
            hasher.update(target.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
        } else if metadata.is_dir() {
            hasher.update(b"dir\0");
            hasher.update(relative.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
            hash_snapshot_tree(root, &path, hasher)?;
        } else if metadata.is_file() {
            hasher.update(b"file\0");
            hasher.update(relative.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
            let mut file =
                fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
            let mut buffer = [0u8; 8192];
            loop {
                let read = file
                    .read(&mut buffer)
                    .with_context(|| format!("read {}", path.display()))?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            hasher.update(b"\0");
        } else {
            return Err(anyhow!("unsupported filesystem entry: {}", path.display()));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        SnapshotProfile, compute_snapshot_content_hash, compute_source_content_hash_with_profile,
        compute_source_fingerprint_with_profile, create_snapshot_with_profile, should_skip,
    };
    use std::fs;
    use std::process::Command;

    #[test]
    fn snapshot_skip_filters_ignore_expected_paths() {
        assert!(should_skip(".git", true, SnapshotProfile::Full));
        assert!(should_skip(".pikaci", true, SnapshotProfile::Full));
        assert!(should_skip(".direnv", true, SnapshotProfile::Full));
        assert!(should_skip("target", true, SnapshotProfile::Full));
        assert!(should_skip("node_modules", false, SnapshotProfile::Full));
        assert!(should_skip(".gradle", false, SnapshotProfile::Full));
        assert!(should_skip("DerivedData", false, SnapshotProfile::Full));
        assert!(should_skip("build", false, SnapshotProfile::Full));
        assert!(!should_skip("Cargo.toml", true, SnapshotProfile::Full));
        assert!(!should_skip("src", false, SnapshotProfile::Full));
        assert!(!should_skip("build", true, SnapshotProfile::Full));
    }

    #[test]
    fn staged_linux_rust_snapshot_skips_mobile_roots() {
        assert!(should_skip("ios", true, SnapshotProfile::StagedLinuxRust));
        assert!(should_skip(
            "android",
            true,
            SnapshotProfile::StagedLinuxRust
        ));
        assert!(!should_skip("rust", true, SnapshotProfile::StagedLinuxRust));
        assert!(!should_skip(
            "crates",
            true,
            SnapshotProfile::StagedLinuxRust
        ));
    }

    #[test]
    fn snapshot_content_hash_ignores_metadata_file() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-snapshot-hash-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("rust")).expect("create rust dir");
        fs::write(root.join("rust/Cargo.toml"), "name = \"pika\"\n").expect("write cargo file");
        fs::write(
            root.join("pikaci-snapshot.json"),
            "{\"content_hash\":\"old\"}\n",
        )
        .expect("write metadata");

        let before = compute_snapshot_content_hash(&root).expect("hash before");
        fs::write(
            root.join("pikaci-snapshot.json"),
            "{\"content_hash\":\"new\"}\n",
        )
        .expect("rewrite metadata");
        let after = compute_snapshot_content_hash(&root).expect("hash after");

        assert_eq!(before, after);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn filtered_source_hash_matches_created_snapshot_hash() {
        let temp_root =
            std::env::temp_dir().join(format!("pikaci-source-hash-test-{}", uuid::Uuid::new_v4()));
        let root = temp_root.join("source");
        let snapshot_dir = temp_root.join("snapshot");
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::create_dir_all(root.join(".git")).expect("create git dir");
        fs::create_dir_all(root.join("nested").join("node_modules")).expect("create nested skip");
        fs::write(root.join("src").join("lib.rs"), "pub fn demo() {}\n").expect("write source");
        fs::write(root.join("nested").join("keep.txt"), "keep").expect("write keep");
        fs::write(
            root.join("nested").join("node_modules").join("skip.txt"),
            "skip",
        )
        .expect("write skip");

        let source_hash =
            compute_source_content_hash_with_profile(&root, SnapshotProfile::Full).expect("hash");
        let snapshot = create_snapshot_with_profile(
            &root,
            &snapshot_dir,
            "2026-03-15T00:00:00Z",
            SnapshotProfile::Full,
        )
        .expect("create snapshot");

        assert_eq!(snapshot.content_hash.as_deref(), Some(source_hash.as_str()));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn source_fingerprint_ignores_filtered_path_changes() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-source-fingerprint-ignore-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").expect("write cargo");
        fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").expect("write source");
        init_test_git_repo(&root);

        let before = compute_source_fingerprint_with_profile(&root, SnapshotProfile::Full)
            .expect("fingerprint before");
        fs::create_dir_all(root.join("target")).expect("create target dir");
        fs::write(root.join("target/stale.txt"), "ignored").expect("write ignored");
        let after = compute_source_fingerprint_with_profile(&root, SnapshotProfile::Full)
            .expect("fingerprint after");

        assert_eq!(before, after);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_fingerprint_changes_for_relevant_workspace_edits() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-source-fingerprint-change-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").expect("write cargo");
        fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").expect("write source");
        init_test_git_repo(&root);

        let before = compute_source_fingerprint_with_profile(&root, SnapshotProfile::Full)
            .expect("fingerprint before");
        fs::write(root.join("src/lib.rs"), "pub fn demo() -> u32 { 42 }\n")
            .expect("rewrite source");
        let after = compute_source_fingerprint_with_profile(&root, SnapshotProfile::Full)
            .expect("fingerprint after");

        assert_ne!(before, after);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_fingerprint_changes_for_ignored_but_included_files() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-source-fingerprint-ignored-include-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("android")).expect("create android dir");
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").expect("write cargo");
        fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").expect("write source");
        fs::write(root.join(".gitignore"), "android/local.properties\n").expect("write gitignore");
        fs::write(
            root.join("android/local.properties"),
            "sdk.dir=/tmp/android-one\n",
        )
        .expect("write local properties");
        init_test_git_repo(&root);

        let before = compute_source_fingerprint_with_profile(&root, SnapshotProfile::Full)
            .expect("fingerprint before");
        fs::write(
            root.join("android/local.properties"),
            "sdk.dir=/tmp/android-two\n",
        )
        .expect("rewrite local properties");
        let after = compute_source_fingerprint_with_profile(&root, SnapshotProfile::Full)
            .expect("fingerprint after");

        assert_ne!(before, after);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_fingerprint_changes_for_nested_untracked_git_checkout_edits() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-source-fingerprint-nested-git-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").expect("write cargo");
        fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").expect("write source");
        init_test_git_repo(&root);

        let nested = root.join("openclaw");
        fs::create_dir_all(&nested).expect("create nested dir");
        run_git_test_command(&nested, &["init"]);
        run_git_test_command(
            &nested,
            &["config", "user.email", "pikaci-tests@example.com"],
        );
        run_git_test_command(&nested, &["config", "user.name", "pikaci tests"]);
        fs::write(
            nested.join("package.json"),
            "{\n  \"name\": \"openclaw\"\n}\n",
        )
        .expect("write nested package");
        run_git_test_command(&nested, &["add", "."]);
        run_git_test_command(&nested, &["commit", "-m", "init"]);

        let before = compute_source_fingerprint_with_profile(&root, SnapshotProfile::Full)
            .expect("fingerprint before");
        fs::write(
            nested.join("package.json"),
            "{\n  \"name\": \"openclaw-next\"\n}\n",
        )
        .expect("rewrite nested package");
        let after = compute_source_fingerprint_with_profile(&root, SnapshotProfile::Full)
            .expect("fingerprint after");

        assert_ne!(before, after);

        let _ = fs::remove_dir_all(root);
    }

    fn init_test_git_repo(root: &std::path::Path) {
        run_git_test_command(root, &["init"]);
        run_git_test_command(root, &["config", "user.email", "pikaci-tests@example.com"]);
        run_git_test_command(root, &["config", "user.name", "pikaci tests"]);
        run_git_test_command(root, &["add", "."]);
        run_git_test_command(root, &["commit", "-m", "init"]);
    }

    fn run_git_test_command(root: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .expect("run git command");
        assert!(
            status.success(),
            "git command failed: git {}",
            args.join(" ")
        );
    }
}
