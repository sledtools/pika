#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import tomllib
from pathlib import Path


def repo_root() -> Path:
    override = os.environ.get("FORGE_GITHUB_CI_REPO_ROOT")
    if override:
        return Path(override).resolve()
    cwd = Path.cwd().resolve()
    if (cwd / "ci" / "forge-lanes.toml").exists():
        return cwd
    return Path(__file__).resolve().parent.parent


def manifest_path() -> Path:
    return repo_root() / "ci" / "forge-lanes.toml"


def git_output(cwd: Path, args: list[str]) -> str:
    completed = subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=True,
        text=True,
        capture_output=True,
    )
    return completed.stdout.strip()


def git_has_commit(cwd: Path, commit: str) -> bool:
    completed = subprocess.run(
        ["git", "cat-file", "-e", f"{commit}^{{commit}}"],
        cwd=cwd,
        text=True,
        capture_output=True,
    )
    return completed.returncode == 0


def fetch_head_into_compare_repo(compare_repo_root: Path, head_repo_root: Path, head: str) -> None:
    temp_ref = "refs/forge-github-ci/head"
    subprocess.run(
        ["git", "fetch", "--no-tags", str(head_repo_root), f"+HEAD:{temp_ref}"],
        cwd=compare_repo_root,
        check=True,
        text=True,
        capture_output=True,
    )
    fetched = git_output(compare_repo_root, ["rev-parse", temp_ref])
    if fetched != head and not git_has_commit(compare_repo_root, head):
        raise SystemExit(
            f"fetched head {fetched} from {head_repo_root} but expected {head}"
        )


def ensure_compare_commits(
    compare_repo_root: Path, base: str, head: str, head_repo_root: Path | None
) -> None:
    if not git_has_commit(compare_repo_root, base):
        raise SystemExit(f"missing base commit {base} in comparison repo {compare_repo_root}")
    if git_has_commit(compare_repo_root, head):
        return
    if head_repo_root is None:
        raise SystemExit(
            f"missing head commit {head} in comparison repo {compare_repo_root}"
        )
    fetch_head_into_compare_repo(compare_repo_root, head_repo_root, head)
    if not git_has_commit(compare_repo_root, head):
        raise SystemExit(
            f"missing head commit {head} in comparison repo {compare_repo_root} after fetch"
        )


def load_manifest(path: Path) -> dict:
    with path.open("rb") as fh:
        return tomllib.load(fh)


def match_path(path: str, patterns: list[str]) -> bool:
    for pattern in patterns:
        if glob_match(path, pattern):
            return True
    return False


def glob_match(path: str, pattern: str) -> bool:
    return re.fullmatch(glob_to_regex(pattern), path) is not None


def glob_to_regex(pattern: str) -> str:
    parts: list[str] = ["^"]
    i = 0
    while i < len(pattern):
        ch = pattern[i]
        if ch == "*":
            if i + 1 < len(pattern) and pattern[i + 1] == "*":
                if i + 2 < len(pattern) and pattern[i + 2] == "/":
                    parts.append("(?:.*/)?")
                    i += 3
                else:
                    parts.append(".*")
                    i += 2
            else:
                parts.append("[^/]*")
                i += 1
        elif ch == "?":
            parts.append("[^/]")
            i += 1
        else:
            parts.append(re.escape(ch))
            i += 1
    parts.append("$")
    return "".join(parts)


def changed_paths(base: str, head: str, compare_repo_root: Path, head_repo_root: Path | None) -> list[str]:
    ensure_compare_commits(compare_repo_root, base, head, head_repo_root)
    merge_base = git_output(compare_repo_root, ["merge-base", base, head])
    output = git_output(compare_repo_root, ["diff", "--name-only", f"{merge_base}..{head}"])
    return [line.strip() for line in output.splitlines() if line.strip()]


def lane_catalog(manifest: dict, mode: str) -> list[dict]:
    group = manifest.get(mode, {})
    return list(group.get("lanes", []))


def lane_timeout_minutes(lane_id: str) -> int:
    if lane_id == "nightly_apple_host_bundle":
        return 90
    if lane_id == "nightly_pika_ui_android":
        return 75
    return 60


def lane_entrypoint(lane: dict) -> str:
    target = lane.get("staged_linux_target")
    if target:
        return f"./scripts/pikaci-staged-linux-remote.sh run {target}"
    return lane["entrypoint"]


def lane_command(lane: dict) -> list[str]:
    target = lane.get("staged_linux_target")
    if target:
        return ["./scripts/pikaci-staged-linux-remote.sh", "run", target]
    return list(lane["command"])


def lane_concurrency_group(lane: dict) -> str | None:
    explicit = lane.get("concurrency_group")
    if explicit:
        return explicit
    target = lane.get("staged_linux_target")
    if target:
        return f"staged-linux:{target}"
    return None


def lane_to_matrix_entry(lane: dict, mode: str) -> dict:
    command = lane_command(lane)
    staged_linux_target = lane.get("staged_linux_target")
    uses_apple_remote = any("pikaci-apple-remote.sh" in part for part in command)
    uses_staged_linux = staged_linux_target is not None
    return {
        "id": lane["id"],
        "title": lane["title"],
        "entrypoint": lane_entrypoint(lane),
        "command": command,
        "command_shell": shlex.join(command),
        "mode": mode,
        "runner": "ubuntu-latest" if uses_apple_remote else "blacksmith-16vcpu-ubuntu-2404",
        "timeout_minutes": lane_timeout_minutes(lane["id"]),
        "needs_openclaw_checkout": lane["id"] in {"pikachat_openclaw_e2e", "nightly_pikachat"},
        "needs_gradle_cache": mode == "nightly" and lane["id"] == "nightly_pika_ui_android",
        "uses_apple_remote": uses_apple_remote,
        "uses_staged_linux": uses_staged_linux,
        "concurrency_group": lane_concurrency_group(lane) or lane["id"],
    }


def select_lanes(
    manifest: dict,
    mode: str,
    base: str | None,
    head: str | None,
    force_all: bool,
    compare_repo_root: Path | None,
    head_repo_root: Path | None,
) -> dict:
    lanes = lane_catalog(manifest, mode)
    changed = []
    selected = []
    if mode == "nightly" or force_all or not base or not head:
        selected = lanes
    else:
        changed = changed_paths(base, head, compare_repo_root or repo_root(), head_repo_root)
        if not changed or "ci/forge-lanes.toml" in changed:
            selected = lanes
        else:
            selected = [
                lane
                for lane in lanes
                if not lane.get("paths") or match_path_any(changed, lane.get("paths", []))
            ]
    return {
        "mode": mode,
        "manifest_path": "ci/forge-lanes.toml",
        "changed_paths": changed,
        "selected_count": len(selected),
        "selected_titles": [lane["title"] for lane in selected],
        "include": [lane_to_matrix_entry(lane, mode) for lane in selected],
    }


def match_path_any(paths: list[str], patterns: list[str]) -> bool:
    return any(match_path(path, patterns) for path in paths)


def write_github_output(path: str, payload: dict) -> None:
    with open(path, "a", encoding="utf-8") as fh:
        fh.write("matrix<<__FORGE_MATRIX__\n")
        fh.write(json.dumps(payload["include"], separators=(",", ":")))
        fh.write("\n__FORGE_MATRIX__\n")
        fh.write(f"selected_count={payload['selected_count']}\n")
        fh.write("selected_titles<<__FORGE_TITLES__\n")
        fh.write("\n".join(payload["selected_titles"]))
        fh.write("\n__FORGE_TITLES__\n")
        fh.write("changed_paths<<__FORGE_CHANGED__\n")
        fh.write("\n".join(payload["changed_paths"]))
        fh.write("\n__FORGE_CHANGED__\n")


def cmd_select(args: argparse.Namespace) -> int:
    manifest = load_manifest(manifest_path())
    compare_repo_root = Path(args.compare_repo_root).resolve() if args.compare_repo_root else None
    head_repo_root = Path(args.head_repo_root).resolve() if args.head_repo_root else None
    payload = select_lanes(
        manifest,
        args.mode,
        args.base,
        args.head,
        args.all,
        compare_repo_root,
        head_repo_root,
    )
    if args.github_output:
        write_github_output(args.github_output, payload)
    print(json.dumps(payload, indent=2))
    return 0


def find_lane(manifest: dict, mode: str, lane_id: str) -> dict:
    for lane in lane_catalog(manifest, mode):
        if lane["id"] == lane_id:
            return lane
    raise SystemExit(f"unknown {mode} lane: {lane_id}")


def cmd_run(args: argparse.Namespace) -> int:
    manifest = load_manifest(manifest_path())
    lane = find_lane(manifest, args.mode, args.lane_id)
    command = lane_command(lane)
    print(f"running {lane['title']}: {shlex.join(command)}", flush=True)
    completed = subprocess.run(command, cwd=repo_root())
    return completed.returncode


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)

    select = sub.add_parser("select")
    select.add_argument("--mode", choices=["branch", "nightly"], required=True)
    select.add_argument("--base")
    select.add_argument("--head")
    select.add_argument("--all", action="store_true")
    select.add_argument("--compare-repo-root")
    select.add_argument("--head-repo-root")
    select.add_argument("--github-output")
    select.set_defaults(func=cmd_select)

    run = sub.add_parser("run")
    run.add_argument("--mode", choices=["branch", "nightly"], required=True)
    run.add_argument("--lane-id", required=True)
    run.set_defaults(func=cmd_run)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
