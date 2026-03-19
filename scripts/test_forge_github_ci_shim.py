from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "forge-github-ci-shim.py"


def git(cwd: Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=True,
        text=True,
        capture_output=True,
    )
    return completed.stdout.strip()


class ForgeGithubCiShimTests(unittest.TestCase):
    def test_branch_selection_uses_merge_base_for_unsynced_fork_pr(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            upstream = root / "upstream"
            fork = root / "fork"
            git(root, "init", upstream.as_posix())
            git(upstream, "config", "user.name", "Test User")
            git(upstream, "config", "user.email", "test@example.com")
            (upstream / "ci").mkdir()
            (upstream / "docs").mkdir()
            (upstream / "README.md").write_text("base\n", encoding="utf-8")
            (upstream / "docs" / "guide.md").write_text("docs\n", encoding="utf-8")
            (upstream / "ci" / "forge-lanes.toml").write_text(
                """
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "docs"
title = "docs"
entrypoint = "printf docs"
command = ["python3", "-c", "print('docs')"]
paths = ["docs/**"]

[[branch.lanes]]
id = "rust"
title = "rust"
entrypoint = "printf rust"
command = ["python3", "-c", "print('rust')"]
paths = ["Cargo.toml"]

[[nightly.lanes]]
id = "nightly"
title = "nightly"
entrypoint = "printf nightly"
command = ["python3", "-c", "print('nightly')"]
""".strip()
                + "\n",
                encoding="utf-8",
            )
            git(upstream, "add", "README.md", "docs/guide.md", "ci/forge-lanes.toml")
            git(upstream, "commit", "-m", "base")
            git(root, "clone", upstream.as_posix(), fork.as_posix())
            git(fork, "config", "user.name", "Test User")
            git(fork, "config", "user.email", "test@example.com")

            (fork / "docs" / "guide.md").write_text("docs changed in fork\n", encoding="utf-8")
            git(fork, "add", "docs/guide.md")
            git(fork, "commit", "-m", "fork docs change")
            head = git(fork, "rev-parse", "HEAD")

            (upstream / "README.md").write_text("upstream advanced\n", encoding="utf-8")
            git(upstream, "add", "README.md")
            git(upstream, "commit", "-m", "upstream advance")
            base = git(upstream, "rev-parse", "HEAD")

            self.assertNotEqual(base, head)
            self.assertNotEqual(
                subprocess.run(
                    ["git", "cat-file", "-e", f"{base}^{{commit}}"],
                    cwd=fork,
                    text=True,
                    capture_output=True,
                ).returncode,
                0,
            )

            completed = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "select",
                    "--mode",
                    "branch",
                    "--base",
                    base,
                    "--head",
                    head,
                    "--compare-repo-root",
                    str(upstream),
                    "--head-repo-root",
                    str(fork),
                ],
                cwd=REPO_ROOT,
                env={**os.environ, "FORGE_GITHUB_CI_REPO_ROOT": str(fork)},
                check=True,
                text=True,
                capture_output=True,
            )
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["changed_paths"], ["docs/guide.md"])
            self.assertEqual([lane["id"] for lane in payload["include"]], ["docs"])

    def test_branch_selection_matches_hidden_workflow_globs_in_unsynced_fork_pr(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            upstream = root / "upstream"
            fork = root / "fork"
            git(root, "init", upstream.as_posix())
            git(upstream, "config", "user.name", "Test User")
            git(upstream, "config", "user.email", "test@example.com")
            (upstream / "ci").mkdir()
            (upstream / ".github" / "workflows").mkdir(parents=True)
            (upstream / "README.md").write_text("base\n", encoding="utf-8")
            (upstream / ".github" / "workflows" / "pre-merge.yml").write_text(
                "name: base\n",
                encoding="utf-8",
            )
            (upstream / "ci" / "forge-lanes.toml").write_text(
                """
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "workflow"
title = "workflow"
entrypoint = "printf workflow"
command = ["python3", "-c", "print('workflow')"]
paths = [".github/**"]

[[nightly.lanes]]
id = "nightly"
title = "nightly"
entrypoint = "printf nightly"
command = ["python3", "-c", "print('nightly')"]
""".strip()
                + "\n",
                encoding="utf-8",
            )
            git(
                upstream,
                "add",
                "README.md",
                ".github/workflows/pre-merge.yml",
                "ci/forge-lanes.toml",
            )
            git(upstream, "commit", "-m", "base")
            git(root, "clone", upstream.as_posix(), fork.as_posix())
            git(fork, "config", "user.name", "Test User")
            git(fork, "config", "user.email", "test@example.com")

            (fork / ".github" / "workflows" / "pre-merge.yml").write_text(
                "name: fork change\n",
                encoding="utf-8",
            )
            git(fork, "add", ".github/workflows/pre-merge.yml")
            git(fork, "commit", "-m", "workflow change")
            head = git(fork, "rev-parse", "HEAD")

            (upstream / "README.md").write_text("upstream advanced\n", encoding="utf-8")
            git(upstream, "add", "README.md")
            git(upstream, "commit", "-m", "upstream advance")
            base = git(upstream, "rev-parse", "HEAD")

            self.assertNotEqual(
                subprocess.run(
                    ["git", "cat-file", "-e", f"{base}^{{commit}}"],
                    cwd=fork,
                    text=True,
                    capture_output=True,
                ).returncode,
                0,
            )

            completed = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "select",
                    "--mode",
                    "branch",
                    "--base",
                    base,
                    "--head",
                    head,
                    "--compare-repo-root",
                    str(upstream),
                    "--head-repo-root",
                    str(fork),
                ],
                cwd=REPO_ROOT,
                env={**os.environ, "FORGE_GITHUB_CI_REPO_ROOT": str(fork)},
                check=True,
                text=True,
                capture_output=True,
            )
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["changed_paths"], [".github/workflows/pre-merge.yml"])
            self.assertEqual([lane["id"] for lane in payload["include"]], ["workflow"])

    def test_branch_selection_matches_root_files_for_double_star_prefix(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init")
            git(repo, "config", "user.name", "Test User")
            git(repo, "config", "user.email", "test@example.com")
            (repo / "ci").mkdir()
            (repo / "foo.rs").write_text("fn main() {}\n", encoding="utf-8")
            (repo / "ci" / "forge-lanes.toml").write_text(
                """
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "root_rust"
title = "root-rust"
entrypoint = "printf root-rust"
command = ["python3", "-c", "print('root-rust')"]
paths = ["**/*.rs"]

[[nightly.lanes]]
id = "nightly"
title = "nightly"
entrypoint = "printf nightly"
command = ["python3", "-c", "print('nightly')"]
""".strip()
                + "\n",
                encoding="utf-8",
            )
            git(repo, "add", "foo.rs", "ci/forge-lanes.toml")
            git(repo, "commit", "-m", "base")
            base = git(repo, "rev-parse", "HEAD")

            (repo / "foo.rs").write_text("fn main() { println!(\"hi\"); }\n", encoding="utf-8")
            git(repo, "add", "foo.rs")
            git(repo, "commit", "-m", "root rust change")
            head = git(repo, "rev-parse", "HEAD")

            completed = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "select",
                    "--mode",
                    "branch",
                    "--base",
                    base,
                    "--head",
                    head,
                ],
                cwd=repo,
                check=True,
                text=True,
                capture_output=True,
            )
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["changed_paths"], ["foo.rs"])
            self.assertEqual([lane["id"] for lane in payload["include"]], ["root_rust"])

    def test_staged_linux_target_resolves_to_remote_command(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init")
            git(repo, "config", "user.name", "Test User")
            git(repo, "config", "user.email", "test@example.com")
            (repo / "ci").mkdir()
            (repo / "docs").mkdir()
            (repo / "docs" / "guide.md").write_text("docs\n", encoding="utf-8")
            (repo / "ci" / "forge-lanes.toml").write_text(
                """
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "linux"
title = "linux"
staged_linux_target = "pre-merge-pika-rust"
paths = ["docs/**"]

[[nightly.lanes]]
id = "nightly"
title = "nightly"
entrypoint = "./nightly.sh"
command = ["./nightly.sh"]
""".strip()
                + "\n",
                encoding="utf-8",
            )
            git(repo, "add", "docs/guide.md", "ci/forge-lanes.toml")
            git(repo, "commit", "-m", "base")
            base = git(repo, "rev-parse", "HEAD")

            (repo / "docs" / "guide.md").write_text("changed\n", encoding="utf-8")
            git(repo, "add", "docs/guide.md")
            git(repo, "commit", "-m", "docs change")
            head = git(repo, "rev-parse", "HEAD")

            completed = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "select",
                    "--mode",
                    "branch",
                    "--base",
                    base,
                    "--head",
                    head,
                    "--compare-repo-root",
                    str(repo),
                    "--head-repo-root",
                    str(repo),
                ],
                cwd=REPO_ROOT,
                env={**os.environ, "FORGE_GITHUB_CI_REPO_ROOT": str(repo)},
                check=True,
                text=True,
                capture_output=True,
            )
            payload = json.loads(completed.stdout)
            self.assertEqual([lane["id"] for lane in payload["include"]], ["linux"])
            self.assertEqual(
                payload["include"][0]["command"],
                ["./scripts/pikaci-staged-linux-remote.sh", "run", "pre-merge-pika-rust"],
            )

    def test_run_supports_staged_linux_target_only_lane(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init")
            git(repo, "config", "user.name", "Test User")
            git(repo, "config", "user.email", "test@example.com")
            (repo / "ci").mkdir()
            (repo / "scripts").mkdir()
            marker = repo / "ran.txt"
            (repo / "scripts" / "pikaci-staged-linux-remote.sh").write_text(
                f"""#!/usr/bin/env bash
set -euo pipefail
printf "%s %s\\n" "$1" "$2" > "{marker}"
""",
                encoding="utf-8",
            )
            (repo / "ci" / "forge-lanes.toml").write_text(
                """
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "x"
title = "x"
staged_linux_target = "pre-merge-pika-rust"

[[nightly.lanes]]
id = "nightly"
title = "nightly"
entrypoint = "./nightly.sh"
command = ["./nightly.sh"]
""".strip()
                + "\n",
                encoding="utf-8",
            )
            subprocess.run(
                ["chmod", "+x", str(repo / "scripts" / "pikaci-staged-linux-remote.sh")],
                check=True,
            )

            completed = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "run",
                    "--mode",
                    "branch",
                    "--lane-id",
                    "x",
                ],
                cwd=REPO_ROOT,
                env={**os.environ, "FORGE_GITHUB_CI_REPO_ROOT": str(repo)},
                check=True,
                text=True,
                capture_output=True,
            )
            self.assertIn(
                "./scripts/pikaci-staged-linux-remote.sh run pre-merge-pika-rust",
                completed.stdout,
            )
            self.assertEqual(marker.read_text(encoding="utf-8"), "run pre-merge-pika-rust\n")

    def test_branch_selection_uses_branch_head_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init")
            git(repo, "config", "user.name", "Test User")
            git(repo, "config", "user.email", "test@example.com")
            (repo / "ci").mkdir()
            (repo / "docs").mkdir()
            (repo / "README.md").write_text("base\n", encoding="utf-8")
            (repo / "docs" / "guide.md").write_text("docs\n", encoding="utf-8")
            (repo / "ci" / "forge-lanes.toml").write_text(
                """
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "docs"
title = "docs"
entrypoint = "printf docs"
command = ["python3", "-c", "print('docs')"]
paths = ["docs/**"]

[[nightly.lanes]]
id = "nightly"
title = "nightly"
entrypoint = "printf nightly"
command = ["python3", "-c", "print('nightly')"]
""".strip()
                + "\n",
                encoding="utf-8",
            )
            git(repo, "add", "README.md", "docs/guide.md", "ci/forge-lanes.toml")
            git(repo, "commit", "-m", "base")
            base = git(repo, "rev-parse", "HEAD")

            (repo / "ci" / "forge-lanes.toml").write_text(
                """
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "docs"
title = "docs"
entrypoint = "printf docs"
command = ["python3", "-c", "print('docs')"]
paths = ["docs/**"]

[[branch.lanes]]
id = "manifest_only"
title = "manifest-only"
entrypoint = "printf manifest-only"
command = ["python3", "-c", "print('manifest-only')"]
paths = ["ci/forge-lanes.toml"]

[[nightly.lanes]]
id = "nightly"
title = "nightly"
entrypoint = "printf nightly"
command = ["python3", "-c", "print('nightly')"]
""".strip()
                + "\n",
                encoding="utf-8",
            )
            git(repo, "add", "ci/forge-lanes.toml")
            git(repo, "commit", "-m", "manifest change")
            head = git(repo, "rev-parse", "HEAD")

            completed = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "select",
                    "--mode",
                    "branch",
                    "--base",
                    base,
                    "--head",
                    head,
                ],
                cwd=repo,
                check=True,
                text=True,
                capture_output=True,
            )
            payload = json.loads(completed.stdout)
            ids = [lane["id"] for lane in payload["include"]]
            self.assertIn("manifest_only", ids)
            self.assertIn("docs", ids)

    def test_nightly_mode_selects_all_nightly_lanes(self) -> None:
        completed = subprocess.run(
            ["python3", str(SCRIPT), "select", "--mode", "nightly"],
            cwd=REPO_ROOT,
            check=True,
            text=True,
            capture_output=True,
        )
        payload = json.loads(completed.stdout)
        ids = [lane["id"] for lane in payload["include"]]
        self.assertIn("nightly_linux", ids)
        self.assertIn("nightly_apple_host_bundle", ids)

    def test_repo_root_override_reads_manifest_from_external_checkout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init")
            git(repo, "config", "user.name", "Test User")
            git(repo, "config", "user.email", "test@example.com")
            (repo / "ci").mkdir()
            (repo / "ci" / "forge-lanes.toml").write_text(
                """
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "override_lane"
title = "override-lane"
entrypoint = "printf override"
command = ["python3", "-c", "print('override')"]
paths = ["README.md"]

[[nightly.lanes]]
id = "override_nightly"
title = "override-nightly"
entrypoint = "printf nightly"
command = ["python3", "-c", "print('nightly')"]
""".strip()
                + "\n",
                encoding="utf-8",
            )
            (repo / "README.md").write_text("override\n", encoding="utf-8")
            git(repo, "add", "README.md", "ci/forge-lanes.toml")
            git(repo, "commit", "-m", "override")

            completed = subprocess.run(
                ["python3", str(SCRIPT), "select", "--mode", "branch", "--all"],
                cwd=REPO_ROOT,
                env={**os.environ, "FORGE_GITHUB_CI_REPO_ROOT": str(repo)},
                check=True,
                text=True,
                capture_output=True,
            )
            payload = json.loads(completed.stdout)
            ids = [lane["id"] for lane in payload["include"]]
            self.assertEqual(ids, ["override_lane"])

    def test_workflow_uses_pull_request_not_pull_request_target(self) -> None:
        workflow = (REPO_ROOT / ".github" / "workflows" / "pre-merge.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("pull_request:\n", workflow)
        self.assertNotIn("pull_request_target:", workflow)
        self.assertIn("path: pr", workflow)
        self.assertIn("FORGE_GITHUB_CI_REPO_ROOT", workflow)
        self.assertIn("--compare-repo-root \"$GITHUB_WORKSPACE\"", workflow)
        self.assertIn("--head-repo-root \"$GITHUB_WORKSPACE/pr\"", workflow)


if __name__ == "__main__":
    unittest.main()
