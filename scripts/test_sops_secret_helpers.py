#!/usr/bin/env python3
import os
import pathlib
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]


def run(cmd, *, env=None, cwd=None, text=True):
    return subprocess.run(
        cmd,
        cwd=cwd or ROOT,
        env=env,
        text=text,
        capture_output=True,
        check=True,
    )


class SopsSecretHelpersTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmpdir = tempfile.TemporaryDirectory(prefix="pika-sops-helper-test.")
        self.dir = pathlib.Path(self.tmpdir.name)
        self.key_file = self.dir / "key.txt"
        self.yaml_plain = self.dir / "plain.yaml"
        self.yaml_enc = self.dir / "secret.sops.yaml"
        self.bin_plain = self.dir / "plain.bin"
        self.bin_enc = self.dir / "secret.bin.sops"
        self.bin_out = self.dir / "out.bin"
        self.apple_secret_env = self.dir / "apple.env"

        run(["age-keygen", "-o", str(self.key_file)])
        recipient = run(["age-keygen", "-y", str(self.key_file)]).stdout.strip()

        self.yaml_plain.write_text("FOO: bar\nMULTILINE: |-\n  line1\n  line2\n", encoding="utf-8")
        self.bin_plain.write_bytes(b"pika-secret-binary")

        env = os.environ.copy()
        env["SOPS_AGE_RECIPIENTS"] = recipient
        yaml_cipher = run(
            [
                "sops",
                "encrypt",
                "--input-type",
                "yaml",
                "--output-type",
                "yaml",
                str(self.yaml_plain),
            ],
            env=env,
        ).stdout
        self.yaml_enc.write_text(yaml_cipher, encoding="utf-8")

        bin_cipher = run(
            [
                "sops",
                "encrypt",
                "--input-type",
                "binary",
                "--output-type",
                "binary",
                str(self.bin_plain),
            ],
            env=env,
            text=False,
        ).stdout
        self.bin_enc.write_bytes(bin_cipher)

    def tearDown(self) -> None:
        self.tmpdir.cleanup()

    def test_read_key_with_sops_age_key_file(self) -> None:
        script = f"""
source "{ROOT / 'scripts/lib/sops-secrets.sh'}"
SOPS_AGE_KEY_FILE="{self.key_file}"
sops_secret_read_key "{self.yaml_enc}" FOO
"""
        proc = run(["bash", "-lc", script])
        self.assertEqual(proc.stdout.strip(), "bar")

    def test_read_key_with_age_secret_key_compat(self) -> None:
        age_secret_key = next(
            line
            for line in self.key_file.read_text(encoding="utf-8").splitlines()
            if line.startswith("AGE-SECRET-KEY-")
        )
        script = f"""
source "{ROOT / 'scripts/lib/sops-secrets.sh'}"
export AGE_SECRET_KEY="{age_secret_key}"
sops_secret_read_key "{self.yaml_enc}" MULTILINE
"""
        proc = run(["bash", "-lc", script])
        self.assertEqual(proc.stdout, "line1\nline2")

    def test_decrypt_binary(self) -> None:
        script = f"""
source "{ROOT / 'scripts/lib/sops-secrets.sh'}"
SOPS_AGE_KEY_FILE="{self.key_file}"
sops_secret_decrypt_binary "{self.bin_enc}" "{self.bin_out}"
"""
        run(["bash", "-lc", script])
        self.assertEqual(self.bin_out.read_bytes(), self.bin_plain.read_bytes())

    def test_encrypt_failure_returns_nonzero(self) -> None:
        missing = self.dir / "missing.yaml"
        out = self.dir / "should-not-exist.sops.yaml"
        script = f"""
source "{ROOT / 'scripts/lib/sops-secrets.sh'}"
SOPS_AGE_RECIPIENTS="$(age-keygen -y "{self.key_file}")"
sops_secret_encrypt_yaml_file "{missing}" "{out}" "$SOPS_AGE_RECIPIENTS"
"""
        proc = subprocess.run(
            ["bash", "-lc", script],
            cwd=ROOT,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(proc.returncode, 0)
        self.assertFalse(out.exists())

    def test_encrypt_yaml_round_trip(self) -> None:
        plain = self.dir / "write.yaml"
        enc = self.dir / "write.sops.yaml"
        script = f"""
source "{ROOT / 'scripts/lib/sops-secrets.sh'}"
SOPS_AGE_RECIPIENTS="$(age-keygen -y "{self.key_file}")"
sops_secret_write_yaml_file "{plain}" FOO "bar baz" MULTILINE $'line1\\nline2'
sops_secret_encrypt_yaml_file "{plain}" "{enc}" "$SOPS_AGE_RECIPIENTS"
SOPS_AGE_KEY_FILE="{self.key_file}"
sops_secret_read_key "{enc}" MULTILINE
"""
        proc = run(["bash", "-lc", script])
        self.assertEqual(proc.stdout, "line1\nline2")

    def test_missing_key_fails(self) -> None:
        script = f"""
source "{ROOT / 'scripts/lib/sops-secrets.sh'}"
SOPS_AGE_KEY_FILE="{self.key_file}"
sops_secret_read_key "{self.yaml_enc}" DOES_NOT_EXIST
"""
        proc = subprocess.run(
            ["bash", "-lc", script],
            cwd=ROOT,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(proc.returncode, 0)

    def test_multiline_shell_escape_round_trip(self) -> None:
        ssh_key = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc'def\n-----END OPENSSH PRIVATE KEY-----"
        script = f"""
value=$(cat <<'EOF'
{ssh_key}
EOF
)
printf 'PIKACI_APPLE_SSH_KEY=%q\\n' "$value" > "{self.apple_secret_env}"
# shellcheck disable=SC1090
source "{self.apple_secret_env}"
printf '%s' "$PIKACI_APPLE_SSH_KEY"
"""
        proc = run(["bash", "-lc", script])
        self.assertEqual(proc.stdout, ssh_key)

    def test_legacy_shell_env_reader_round_trip(self) -> None:
        ssh_key = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc'def\n-----END OPENSSH PRIVATE KEY-----"
        script = f"""
source "{ROOT / 'scripts/lib/release-secrets.sh'}"
value=$(cat <<'EOF'
{ssh_key}
EOF
)
printf 'PIKACI_APPLE_SSH_KEY=%q\\n' "$value" > "{self.apple_secret_env}"
release_secret_read_shell_env_value_from_file "{self.apple_secret_env}" PIKACI_APPLE_SSH_KEY
"""
        proc = run(["bash", "-lc", script])
        self.assertEqual(proc.stdout, ssh_key)

    def test_release_secret_prefers_primary_identity_file(self) -> None:
        primary = self.dir / "primary.txt"
        fallback = self.dir / "keys.txt"
        primary.write_text("primary\n", encoding="utf-8")
        fallback.write_text("fallback\n", encoding="utf-8")
        script = f"""
source "{ROOT / 'scripts/lib/release-secrets.sh'}"
PIKA_RELEASE_AGE_IDENTITY_FILE_PRIMARY_DEFAULT="{primary}"
PIKA_RELEASE_AGE_IDENTITY_FILE_DEFAULT="{fallback}"
release_secret_identity_file_for_read
"""
        proc = run(["bash", "-lc", script])
        self.assertEqual(proc.stdout.strip(), str(primary))

    def test_sops_encrypt_uses_repo_config_by_default(self) -> None:
        config = self.dir / ".sops.yaml"
        secret = self.dir / "secret.sops.yaml"
        plain = self.dir / "repo-secret.yaml"
        recipient = run(["age-keygen", "-y", str(self.key_file)]).stdout.strip()
        config.write_text(
            "creation_rules:\n"
            f"  - path_regex: .*\\.sops\\.yaml$\n"
            f"    age: [{recipient}]\n",
            encoding="utf-8",
        )
        plain.write_text("FOO: bar\n", encoding="utf-8")
        script = f"""
source "{ROOT / 'scripts/lib/sops-secrets.sh'}"
PIKA_SOPS_CONFIG_FILE="{config}"
SOPS_AGE_KEY_FILE="{self.key_file}"
sops_secret_encrypt_yaml_file "{plain}" "{secret}"
sops_secret_read_key "{secret}" FOO
"""
        proc = run(["bash", "-lc", script])
        self.assertEqual(proc.stdout.strip(), "bar")


if __name__ == "__main__":
    unittest.main()
