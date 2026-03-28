import os
import stat
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


class PikaciToolsJsonTests(unittest.TestCase):
    def test_shell_helpers_load_structured_defaults_and_target_info(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            fake_bin = Path(tmp_dir) / "pikaci"
            fake_bin.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import json
                    import sys

                    args = sys.argv[1:]
                    if args == ["staged-linux-remote-defaults", "--json"]:
                        print(json.dumps({
                            "ssh_binary": "/usr/bin/ssh",
                            "ssh_nix_binary": "nix",
                            "ssh_host": "pika-build",
                            "remote_work_dir": "/var/tmp/jerichoci-prepared-output",
                            "remote_launcher_binary": "/run/current-system/sw/bin/pikaci-launch-fulfill-prepared-output",
                            "remote_helper_binary": "/run/current-system/sw/bin/pikaci-fulfill-prepared-output",
                            "store_uri": "ssh://pika-build",
                        }))
                        raise SystemExit(0)
                    if args == ["staged-linux-target-info", "pre-merge-pika-rust", "--json"]:
                        print(json.dumps({
                            "target_id": "pre-merge-pika-rust",
                            "target_description": "Run the VM-backed Rust tests from the pre-merge pika lane",
                            "shared_prepare_node_prefix": "pika-core-linux-rust",
                            "shared_prepare_description": "pika_core staged Linux Rust lane",
                            "workspace_deps_output_name": "ci.x86_64-linux.workspaceDeps",
                            "workspace_build_output_name": "ci.x86_64-linux.workspaceBuild",
                            "workspace_output_system": "x86_64-linux",
                            "workspace_deps_installable": ".#ci.x86_64-linux.workspaceDeps",
                            "workspace_build_installable": ".#ci.x86_64-linux.workspaceBuild",
                        }))
                        raise SystemExit(0)
                    raise SystemExit(f"unexpected args: {args}")
                    """
                )
            )
            fake_bin.chmod(fake_bin.stat().st_mode | stat.S_IXUSR)

            script = textwrap.dedent(
                f"""\
                source "{ROOT / 'scripts/lib/pikaci-tools.sh'}"
                export PIKACI_BIN="{fake_bin}"
                export JERICHOCI_PREPARED_OUTPUT_FULFILL_BINARY="/tmp/pikaci-fulfill-prepared-output"
                export JERICHOCI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY="/tmp/pikaci-launch-fulfill-prepared-output"
                load_pikaci_staged_linux_remote_defaults "{ROOT}"
                printf '%s\\n' "$default_ssh_host|$default_store_uri|$default_remote_work_dir"
                load_pikaci_staged_linux_target_info pre-merge-pika-rust
                printf '%s\\n' "$target_id|$deps_installable|$build_installable"
                """
            )
            completed = subprocess.run(
                ["bash", "-lc", script],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=True,
            )

            lines = completed.stdout.strip().splitlines()
            self.assertEqual(
                lines[0],
                "pika-build|ssh://pika-build|/var/tmp/jerichoci-prepared-output",
            )
            self.assertEqual(
                lines[1],
                "pre-merge-pika-rust|.#ci.x86_64-linux.workspaceDeps|.#ci.x86_64-linux.workspaceBuild",
            )

    def test_pikaci_ci_run_consumes_json_run_and_log_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            tmp_path = Path(tmp_dir)
            host_log = tmp_path / "host.log"
            host_log.write_text("host failure details\n")
            fake_bin = tmp_path / "pikaci"
            fake_bin.write_text(
                textwrap.dedent(
                    f"""\
                    #!/usr/bin/env python3
                    import json
                    import sys

                    args = sys.argv[1:]
                    if args == ["run", "fake-target", "--output", "json"]:
                        print(json.dumps({{
                            "run_id": "run-1",
                            "status": "failed",
                            "jobs": [],
                        }}))
                        raise SystemExit(1)
                    if args == ["logs", "run-1", "--metadata-json"]:
                        print(json.dumps({{
                            "run_id": "run-1",
                            "status": "failed",
                            "jobs": [{{
                                "id": "job-one",
                                "host_log_path": {str(host_log)!r},
                                "guest_log_path": "",
                                "host_log_exists": True,
                                "guest_log_exists": False,
                            }}],
                        }}))
                        raise SystemExit(0)
                    raise SystemExit(f"unexpected args: {{args}}")
                    """
                )
            )
            fake_bin.chmod(fake_bin.stat().st_mode | stat.S_IXUSR)

            completed = subprocess.run(
                ["bash", str(ROOT / "scripts/pikaci-ci-run.sh"), "fake-target"],
                cwd=ROOT,
                text=True,
                capture_output=True,
                env={
                    **os.environ,
                    "PIKACI_BIN": str(fake_bin),
                    "JERICHOCI_PREPARED_OUTPUT_FULFILL_BINARY": "/tmp/pikaci-fulfill-prepared-output",
                    "JERICHOCI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY": "/tmp/pikaci-launch-fulfill-prepared-output",
                },
            )

            self.assertEqual(completed.returncode, 1)
            self.assertIn('"run_id": "run-1"', completed.stdout)
            self.assertIn(
                "===== pikaci host log: run=run-1 job=job-one =====",
                completed.stderr,
            )
            self.assertIn("host failure details", completed.stderr)

    def test_pikaci_ci_run_degrades_cleanly_when_log_metadata_lookup_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            tmp_path = Path(tmp_dir)
            fake_bin = tmp_path / "pikaci"
            fake_bin.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import json
                    import sys

                    args = sys.argv[1:]
                    if args == ["run", "fake-target", "--output", "json"]:
                        print(json.dumps({
                            "run_id": "run-2",
                            "status": "failed",
                            "jobs": [],
                        }))
                        raise SystemExit(1)
                    if args == ["logs", "run-2", "--metadata-json"]:
                        print("not-json")
                        raise SystemExit(1)
                    raise SystemExit(f"unexpected args: {args}")
                    """
                )
            )
            fake_bin.chmod(fake_bin.stat().st_mode | stat.S_IXUSR)

            completed = subprocess.run(
                ["bash", str(ROOT / "scripts/pikaci-ci-run.sh"), "fake-target"],
                cwd=ROOT,
                text=True,
                capture_output=True,
                env={
                    **os.environ,
                    "PIKACI_BIN": str(fake_bin),
                    "JERICHOCI_PREPARED_OUTPUT_FULFILL_BINARY": "/tmp/pikaci-fulfill-prepared-output",
                    "JERICHOCI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY": "/tmp/pikaci-launch-fulfill-prepared-output",
                },
            )

            self.assertEqual(completed.returncode, 1)
            self.assertIn('"run_id": "run-2"', completed.stdout)
            self.assertIn(
                "warning: failed to load pikaci log metadata for run=run-2",
                completed.stderr,
            )
            self.assertNotIn("Traceback", completed.stderr)


if __name__ == "__main__":
    unittest.main()
