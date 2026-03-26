use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::storage::Store;

pub(crate) struct GitTestRepo {
    _root: tempfile::TempDir,
    bare: PathBuf,
    seed: PathBuf,
    db_path: PathBuf,
}

impl GitTestRepo {
    pub(crate) fn new() -> Self {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        let db_path = root.path().join("pika-git.db");

        Self::git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        Self::git(root.path(), &["init", seed.to_str().expect("seed path")]);
        Self::git(&seed, &["config", "user.name", "Test User"]);
        Self::git(&seed, &["config", "user.email", "test@example.com"]);

        Self {
            _root: root,
            bare,
            seed,
            db_path,
        }
    }

    pub(crate) fn bare_path(&self) -> &Path {
        &self.bare
    }

    pub(crate) fn seed_path(&self) -> &Path {
        &self.seed
    }

    pub(crate) fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub(crate) fn open_store(&self) -> Store {
        Store::open(&self.db_path).expect("open store")
    }

    pub(crate) fn seed_git(&self, args: &[&str]) {
        Self::git(&self.seed, args);
    }

    pub(crate) fn write_seed(&self, relative: &str, contents: &str) {
        let path = self.seed.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, contents).expect("write seed file");
    }

    pub(crate) fn chmod_seed_executable(&self, relative: &str) {
        use std::os::unix::fs::PermissionsExt;

        let path = self.seed.join(relative);
        let mut perms = fs::metadata(&path).expect("script metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod seed file");
    }

    pub(crate) fn seed_add(&self, paths: &[&str]) {
        let mut args = Vec::with_capacity(paths.len() + 1);
        args.push("add");
        args.extend(paths.iter().copied());
        self.seed_git(&args);
    }

    pub(crate) fn seed_commit(&self, message: &str) {
        self.seed_git(&["commit", "-m", message]);
    }

    pub(crate) fn seed_push_master(&self) {
        self.seed_git(&["branch", "-M", "master"]);
        self.seed_git(&[
            "remote",
            "add",
            "origin",
            self.bare.to_str().expect("bare path"),
        ]);
        self.seed_git(&["push", "origin", "master"]);
    }

    pub(crate) fn seed_checkout_new_branch(&self, branch_name: &str) {
        self.seed_git(&["checkout", "-b", branch_name]);
    }

    pub(crate) fn seed_push_branch(&self, branch_name: &str) {
        self.seed_git(&["push", "origin", branch_name]);
    }

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
}
