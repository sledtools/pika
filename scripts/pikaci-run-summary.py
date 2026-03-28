#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Summarize the latest Jericho CI run and optionally stage a small "
            "artifact bundle for GitHub."
        )
    )
    parser.add_argument(
        "--state-root",
        default=".jerichoci",
        help="Jericho CI state root. Default: .jerichoci",
    )
    parser.add_argument(
        "--target-id",
        default="pre-merge-pika-rust",
        help="Run target id to summarize. Default: pre-merge-pika-rust",
    )
    parser.add_argument(
        "--run-id",
        help="Explicit run id to summarize. Default: latest run matching --target-id",
    )
    parser.add_argument(
        "--min-created-at-exclusive",
        help="Only consider runs created strictly after this timestamp.",
    )
    parser.add_argument(
        "--allow-missing",
        action="store_true",
        help="Exit successfully with empty outputs when no matching run exists.",
    )
    parser.add_argument(
        "--github-output",
        help="Path to append GitHub step outputs to.",
    )
    parser.add_argument(
        "--markdown-out",
        help="Path to write a markdown summary to.",
    )
    parser.add_argument(
        "--artifact-dir",
        help="Directory to stage a compact run artifact bundle into.",
    )
    return parser.parse_args()


def parse_time(value: str | None) -> datetime | None:
    if not value:
        return None
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def duration_seconds(start: str | None, end: str | None) -> int | None:
    start_dt = parse_time(start)
    end_dt = parse_time(end)
    if start_dt is None or end_dt is None:
        return None
    return max(0, round((end_dt - start_dt).total_seconds()))


@dataclass
class JobSummary:
    job_id: str
    status: str
    duration_seconds: int | None
    host_log_path: str | None
    guest_log_path: str | None


def load_run_record(run_json: Path) -> dict:
    return json.loads(run_json.read_text())


def find_latest_run(
    state_root: Path,
    target_id: str,
    min_created_at_exclusive: str | None,
) -> Path | None:
    runs_root = state_root / "runs"
    if not runs_root.is_dir():
        return None
    candidates: list[tuple[str, Path]] = []
    for run_dir in runs_root.iterdir():
        run_json = run_dir / "run.json"
        if not run_json.is_file():
            continue
        try:
            record = load_run_record(run_json)
        except json.JSONDecodeError:
            continue
        if record.get("target_id") == target_id:
            created_at = record.get("created_at", "")
            if min_created_at_exclusive and created_at <= min_created_at_exclusive:
                continue
            candidates.append((created_at, run_json))
    if not candidates:
        return None
    candidates.sort(key=lambda item: item[0])
    return candidates[-1][1]


def ensure_parent(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)


def copy_if_exists(source: Path | None, destination: Path) -> None:
    if source is None or not source.exists():
        return
    ensure_parent(destination)
    shutil.copy2(source, destination)


def main() -> int:
    args = parse_args()
    state_root = Path(args.state_root)
    run_json = None
    if args.run_id:
        run_json = state_root / "runs" / args.run_id / "run.json"
    else:
        run_json = find_latest_run(
            state_root, args.target_id, args.min_created_at_exclusive
        )
    if run_json is None or not run_json.exists():
        markdown = (
            "### Pikaci Linux lane\n\n"
            "- run id: `none`\n"
            "- target: `{}`\n"
            "- result: `unavailable`\n"
            "- duration: `unknown`\n"
            "- prepared outputs record: `none`\n".format(args.target_id)
        )
        if args.markdown_out:
            markdown_path = Path(args.markdown_out)
            ensure_parent(markdown_path)
            markdown_path.write_text(markdown)
        else:
            sys.stdout.write(markdown)
        if args.github_output:
            output_path = Path(args.github_output)
            with output_path.open("a", encoding="utf-8") as handle:
                handle.write("run_id=\n")
                handle.write("run_status=\n")
                handle.write("run_duration_seconds=\n")
                handle.write("artifact_name=\n")
        if args.allow_missing:
            return 0
        raise SystemExit(f"error: no pikaci runs found for target_id={args.target_id!r}")
    run_dir = run_json.parent
    run = load_run_record(run_json)

    run_id = run["run_id"]
    run_status = run["status"]
    run_duration = duration_seconds(run.get("created_at"), run.get("finished_at"))
    prepared_outputs_path = run.get("prepared_outputs_path")

    jobs: list[JobSummary] = []
    for job in run.get("jobs", []):
        jobs.append(
            JobSummary(
                job_id=job["id"],
                status=job["status"],
                duration_seconds=duration_seconds(
                    job.get("started_at"), job.get("finished_at")
                ),
                host_log_path=job.get("host_log_path"),
                guest_log_path=job.get("guest_log_path"),
            )
        )

    markdown_lines = [
        "### Pikaci Linux lane",
        "",
        f"- run id: `{run_id}`",
        f"- target: `{run.get('target_id', args.target_id)}`",
        f"- result: `{run_status}`",
        f"- duration: `{run_duration if run_duration is not None else 'unknown'}s`",
        f"- prepared outputs record: `{prepared_outputs_path or 'none'}`",
        "",
        "#### Jobs",
        "",
    ]
    for job in jobs:
        duration = (
            f"{job.duration_seconds}s" if job.duration_seconds is not None else "unknown"
        )
        markdown_lines.append(
            f"- `{job.job_id}`: `{job.status}` in `{duration}`"
        )
    markdown = "\n".join(markdown_lines) + "\n"

    if args.markdown_out:
        markdown_path = Path(args.markdown_out)
        ensure_parent(markdown_path)
        markdown_path.write_text(markdown)
    else:
        sys.stdout.write(markdown)

    if args.github_output:
        output_path = Path(args.github_output)
        with output_path.open("a", encoding="utf-8") as handle:
            handle.write(f"run_id={run_id}\n")
            handle.write(f"run_status={run_status}\n")
            handle.write(
                f"run_duration_seconds={'' if run_duration is None else run_duration}\n"
            )
            handle.write(f"artifact_name=pikaci-run-{run_id}\n")

    if args.artifact_dir:
        artifact_dir = Path(args.artifact_dir)
        artifact_dir.mkdir(parents=True, exist_ok=True)
        copy_if_exists(run_json, artifact_dir / "run.json")
        copy_if_exists(run_dir / "plan.json", artifact_dir / "plan.json")
        copy_if_exists(
            Path(prepared_outputs_path) if prepared_outputs_path else None,
            artifact_dir / "prepared-outputs.json",
        )
        for job in jobs:
            copy_if_exists(
                Path(job.host_log_path) if job.host_log_path else None,
                artifact_dir / "jobs" / job.job_id / "host.log",
            )
            copy_if_exists(
                Path(job.guest_log_path) if job.guest_log_path else None,
                artifact_dir / "jobs" / job.job_id / "guest.log",
            )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
