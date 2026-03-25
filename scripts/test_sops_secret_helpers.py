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


if __name__ == "__main__":
    unittest.main()
