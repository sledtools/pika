#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import textwrap
import tomllib
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SPEC_PATH = ROOT / "invariants" / "invariants.toml"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run a Codex-backed architecture invariant review."
    )
    parser.add_argument(
        "--spec",
        type=Path,
        default=DEFAULT_SPEC_PATH,
        help=f"Path to invariants TOML (default: {DEFAULT_SPEC_PATH})",
    )
    parser.add_argument(
        "--model",
        default=os.environ.get("PIKA_INVARIANTS_CODEX_MODEL", "").strip() or None,
        help="Optional Codex model override.",
    )
    parser.add_argument(
        "--json-out",
        type=Path,
        help="Optional path to write the raw JSON report.",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Print Codex stdout/stderr while running.",
    )
    return parser.parse_args()


def load_spec(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        spec = tomllib.load(handle)

    if spec.get("version") != 1:
        raise SystemExit(f"unsupported invariants spec version in {path}")

    invariants = spec.get("invariant")
    if not isinstance(invariants, list) or not invariants:
        raise SystemExit(f"{path} must define at least one [[invariant]] entry")

    ids: set[str] = set()
    for entry in invariants:
        if not isinstance(entry, dict):
            raise SystemExit(f"{path} contains a non-table [[invariant]] entry")
        invariant_id = entry.get("id")
        if not isinstance(invariant_id, str) or not invariant_id.strip():
            raise SystemExit(f"{path} contains an invariant without a non-empty id")
        if invariant_id in ids:
            raise SystemExit(f"{path} contains duplicate invariant id {invariant_id}")
        ids.add(invariant_id)
        if entry.get("kind") not in {"must", "allowed"}:
            raise SystemExit(
                f"{path} invariant {invariant_id} has unsupported kind {entry.get('kind')!r}"
            )
        if not isinstance(entry.get("statement"), str) or not entry["statement"].strip():
            raise SystemExit(f"{path} invariant {invariant_id} must have a statement")
        scope = entry.get("scope", [])
        if not isinstance(scope, list) or any(not isinstance(item, str) for item in scope):
            raise SystemExit(
                f"{path} invariant {invariant_id} must use a string array for scope"
            )

    return spec


def build_prompt(spec: dict[str, Any], spec_path: Path) -> str:
    invariant_blocks: list[str] = []
    for entry in spec["invariant"]:
        scope = entry.get("scope", [])
        scope_text = ", ".join(scope) if scope else "(entire repository)"
        lines = [
            f"- id: {entry['id']}",
            f"  area: {entry.get('area', 'unspecified')}",
            f"  kind: {entry['kind']}",
            f"  statement: {entry['statement']}",
            f"  scope: {scope_text}",
        ]
        hint = entry.get("hint")
        if isinstance(hint, str) and hint.strip():
            lines.append(f"  hint: {hint.strip()}")
        invariant_blocks.append("\n".join(lines))

    return textwrap.dedent(
        f"""\
        You are reviewing architecture invariants for the repository at {ROOT}.

        Study the current codebase and grade each invariant as either "pass" or "fail".
        Treat "fail" as the default when the code is mixed, ambiguous, partially compliant, or clearly violates the statement.
        Focus on the current implementation, not on design intent.
        Do not modify files.

        Return JSON matching the provided schema.

        For each invariant:
        - keep the rationale concise
        - include 1 to 3 concrete file references in `evidence`
        - evidence entries should be repository-relative paths, optionally with a short note after a colon

        Invariants spec source: {spec_path}

        Invariants:
        {os.linesep.join(invariant_blocks)}
        """
    ).strip()


def output_schema(spec: dict[str, Any]) -> dict[str, Any]:
    ids = [entry["id"] for entry in spec["invariant"]]
    return {
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "required": ["summary", "results"],
        "additionalProperties": False,
        "properties": {
            "summary": {"type": "string"},
            "results": {
                "type": "array",
                "minItems": len(ids),
                "maxItems": len(ids),
                "items": {
                    "type": "object",
                    "required": ["id", "grade", "rationale", "evidence"],
                    "additionalProperties": False,
                    "properties": {
                        "id": {"type": "string", "enum": ids},
                        "grade": {"type": "string", "enum": ["pass", "fail"]},
                        "rationale": {"type": "string"},
                        "evidence": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": 3,
                            "items": {"type": "string"},
                        },
                    },
                },
            },
        },
    }


def run_codex_review(
    prompt: str,
    schema: dict[str, Any],
    model: str | None,
    verbose: bool = False,
) -> tuple[dict[str, Any], str]:
    with tempfile.TemporaryDirectory(prefix="invariants-review-") as tmp_dir:
        tmp = Path(tmp_dir)
        schema_path = tmp / "schema.json"
        output_path = tmp / "report.json"
        schema_path.write_text(json.dumps(schema, indent=2) + "\n", encoding="utf-8")

        cmd = [
            "codex",
            "-a",
            "never",
            "exec",
            "--sandbox",
            "workspace-write",
            "--ephemeral",
            "--color",
            "never",
            "--output-schema",
            str(schema_path),
            "-o",
            str(output_path),
            "-C",
            str(ROOT),
        ]
        if model:
            cmd.extend(["-m", model])
        cmd.append(prompt)

        completed = subprocess.run(
            cmd,
            cwd=ROOT,
            text=True,
            capture_output=True,
        )
        combined_output = completed.stdout
        if completed.stderr:
            combined_output += completed.stderr
        if verbose and combined_output.strip():
            print(combined_output, file=sys.stderr, end="" if combined_output.endswith("\n") else "\n")
        if completed.returncode != 0:
            raise RuntimeError(
                "codex invariant review failed\n"
                + combined_output.strip()
            )
        payload = json.loads(output_path.read_text(encoding="utf-8"))
        return payload, combined_output


def validate_report(spec: dict[str, Any], report: dict[str, Any]) -> list[dict[str, Any]]:
    expected = {entry["id"] for entry in spec["invariant"]}
    results = report.get("results")
    if not isinstance(results, list):
        raise SystemExit("Codex report did not return a `results` array")
    seen: set[str] = set()
    normalized: list[dict[str, Any]] = []
    for result in results:
        if not isinstance(result, dict):
            raise SystemExit("Codex report contains a non-object result entry")
        invariant_id = result.get("id")
        if invariant_id not in expected:
            raise SystemExit(f"Codex report returned unexpected invariant id {invariant_id!r}")
        if invariant_id in seen:
            raise SystemExit(f"Codex report duplicated invariant id {invariant_id}")
        seen.add(invariant_id)
        if result.get("grade") not in {"pass", "fail"}:
            raise SystemExit(f"Codex report returned invalid grade for {invariant_id}")
        evidence = result.get("evidence")
        if not isinstance(evidence, list) or not evidence:
            raise SystemExit(f"Codex report returned no evidence for {invariant_id}")
        normalized.append(result)
    if seen != expected:
        missing = ", ".join(sorted(expected - seen))
        raise SystemExit(f"Codex report omitted invariant ids: {missing}")
    order = {entry["id"]: index for index, entry in enumerate(spec["invariant"])}
    normalized.sort(key=lambda item: order[item["id"]])
    return normalized


def print_report(spec: dict[str, Any], report: dict[str, Any]) -> int:
    results = validate_report(spec, report)
    summary = report.get("summary", "").strip()
    print(f"invariants: {spec.get('name', 'unnamed')}")
    if summary:
        print(f"summary: {summary}")
    failures = 0
    for result in results:
        grade = result["grade"].upper()
        if result["grade"] == "fail":
            failures += 1
        print(f"{grade} {result['id']}: {result['rationale'].strip()}")
        for evidence in result["evidence"]:
            print(f"  evidence: {evidence}")
    return 1 if failures else 0


def main() -> int:
    args = parse_args()
    spec = load_spec(args.spec)
    prompt = build_prompt(spec, args.spec)
    report, _ = run_codex_review(
        prompt=prompt,
        schema=output_schema(spec),
        model=args.model,
        verbose=args.verbose,
    )
    if args.json_out:
        args.json_out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return print_report(spec, report)


if __name__ == "__main__":
    raise SystemExit(main())
