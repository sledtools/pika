from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "check_invariants.py"


def load_script_module():
    spec = importlib.util.spec_from_file_location("check_invariants", SCRIPT)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CheckInvariantsTests(unittest.TestCase):
    def test_load_spec_rejects_duplicate_ids(self) -> None:
        module = load_script_module()
        with tempfile.TemporaryDirectory() as tmp_dir:
            spec_path = Path(tmp_dir) / "invariants.toml"
            spec_path.write_text(
                """
version = 1

[[invariant]]
id = "DUP-001"
kind = "must"
statement = "first"

[[invariant]]
id = "DUP-001"
kind = "must"
statement = "second"
""".strip()
                + "\n",
                encoding="utf-8",
            )
            with self.assertRaises(SystemExit) as ctx:
                module.load_spec(spec_path)
            self.assertIn("duplicate invariant id DUP-001", str(ctx.exception))

    def test_build_prompt_includes_scope_and_hint(self) -> None:
        module = load_script_module()
        spec = {
            "version": 1,
            "name": "test",
            "invariant": [
                {
                    "id": "PIKACI-001",
                    "area": "pikaci",
                    "kind": "must",
                    "statement": "Example statement.",
                    "scope": ["crates/pikaci/**", "ci/**"],
                    "hint": "Focus on library boundaries.",
                }
            ],
        }
        prompt = module.build_prompt(spec, ROOT / "invariants" / "invariants.toml")
        self.assertIn("id: PIKACI-001", prompt)
        self.assertIn("scope: crates/pikaci/**, ci/**", prompt)
        self.assertIn("hint: Focus on library boundaries.", prompt)

    def test_validate_report_preserves_spec_order(self) -> None:
        module = load_script_module()
        spec = {
            "version": 1,
            "name": "test",
            "invariant": [
                {"id": "ONE", "kind": "must", "statement": "one", "scope": []},
                {"id": "TWO", "kind": "must", "statement": "two", "scope": []},
            ],
        }
        report = {
            "summary": "mixed",
            "results": [
                {
                    "id": "TWO",
                    "grade": "fail",
                    "rationale": "second",
                    "evidence": ["b.rs"],
                },
                {
                    "id": "ONE",
                    "grade": "pass",
                    "rationale": "first",
                    "evidence": ["a.rs"],
                },
            ],
        }
        normalized = module.validate_report(spec, report)
        self.assertEqual([entry["id"] for entry in normalized], ["ONE", "TWO"])

    def test_output_schema_matches_invariant_ids(self) -> None:
        module = load_script_module()
        spec = {
            "version": 1,
            "name": "test",
            "invariant": [
                {"id": "ONE", "kind": "must", "statement": "one", "scope": []},
                {"id": "TWO", "kind": "allowed", "statement": "two", "scope": []},
            ],
        }
        schema = module.output_schema(spec)
        self.assertEqual(
            schema["properties"]["results"]["items"]["properties"]["id"]["enum"],
            ["ONE", "TWO"],
        )
        json.dumps(schema)


if __name__ == "__main__":
    unittest.main()
