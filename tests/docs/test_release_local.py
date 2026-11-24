# SPDX-License-Identifier: MIT OR Apache-2.0
"""Tests of tools/release-local.sh with --dry-run (8.10). No network, no gh, no Docker.

Each test makes a throwaway git repository on the branch main, with fake archives, and runs the script on it.
Run: python3 -I -m unittest discover -s tests/docs
"""
import gzip
import hashlib
import io
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
VERSION = "0.1.0"
FILES = ["LICENSE-APACHE", "LICENSE-MIT", "README.md", "THIRD-PARTY-NOTICES.md", "pupitre"]


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout


def make_archive(dist: Path, arch: str, files=FILES) -> None:
    name = f"pupitre_{VERSION}_linux_{arch}"
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as tar:
        entry = tarfile.TarInfo(name + "/")
        entry.type = tarfile.DIRTYPE
        tar.addfile(entry)
        for member in files:
            data = member.encode()
            info = tarfile.TarInfo(f"{name}/{member}")
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    raw = gzip.compress(buffer.getvalue(), mtime=0)
    (dist / f"{name}.tar.gz").write_bytes(raw)
    (dist / f"{name}.tar.gz.sha256").write_text(f"{hashlib.sha256(raw).hexdigest()}  {name}.tar.gz\n")


class ReleaseLocal(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name) / "repo"
        (self.repo / "tools").mkdir(parents=True)
        (self.repo / "scripts").mkdir()
        (self.repo / ".github").mkdir()
        shutil.copy(ROOT / "tools/release-local.sh", self.repo / "tools/release-local.sh")
        shutil.copy(ROOT / "tools/changelog-section.sh", self.repo / "tools/changelog-section.sh")
        (self.repo / "Cargo.toml").write_text(f'[workspace.package]\nversion = "{VERSION}"\n')
        (self.repo / "CHANGELOG.md").write_text(f"# Changelog\n\n## Unreleased\n\n## {VERSION} - 2026-11-24\n\n### Added\n\n- A line.\n")
        (self.repo / "scripts/ok.sh").write_text("echo ok\n")
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "test@example.invalid")
        git(self.repo, "config", "user.name", "Test")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "Start")
        (self.repo / "dist").mkdir()
        (self.repo / ".gitignore").write_text("/dist/\n/target/\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "Ignore the output")
        make_archive(self.repo / "dist", "amd64")
        make_archive(self.repo / "dist", "arm64")
        # The host of the test can be amd64 or arm64: the script builds the archive of its architecture only when absent.

    def tearDown(self):
        self.tmp.cleanup()

    def run_script(self, *args: str):
        return subprocess.run(["bash", "tools/release-local.sh", *args], cwd=self.repo, capture_output=True, text=True)

    def test_dry_run_passes_and_writes_the_sums_and_the_notes(self):
        result = self.run_script(VERSION, "--dry-run", "--partial-gate", "Only the test of the script ran.")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        sums = (self.repo / "dist/SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(sums), 2)
        self.assertTrue(sums[0].endswith(f"pupitre_{VERSION}_linux_amd64.tar.gz"))
        notes = (self.repo / "target/release-notes.md").read_text()
        self.assertIn("- A line.", notes)
        self.assertIn("The hosted CI did not run", notes)
        self.assertIn("Only the test of the script ran.", notes)
        self.assertNotIn("passed", notes)
        self.assertIn("gh release create", result.stdout)

    def test_amd64_only(self):
        (self.repo / "dist" / f"pupitre_{VERSION}_linux_arm64.tar.gz").unlink()
        result = self.run_script(VERSION, "--dry-run", "--partial-gate", "x", "--amd64-only")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len((self.repo / "dist/SHA256SUMS").read_text().splitlines()), 1)
        self.assertIn("The arm64 archive is not built", (self.repo / "target/release-notes.md").read_text())

    def test_refusals(self):
        cases = {
            "bad version": ([ "1.2", "--dry-run", "--partial-gate", "x"], "bad version"),
            "empty gate text": ([VERSION, "--dry-run", "--partial-gate", "  "], "not empty"),
            "unknown option": ([VERSION, "--go"], "unknown option"),
            "other version": (["0.2.0", "--dry-run", "--partial-gate", "x"], "Cargo.toml has version"),
        }
        for name, (args, message) in cases.items():
            result = self.run_script(*args)
            self.assertNotEqual(result.returncode, 0, name)
            self.assertIn(message, result.stderr, name)

    def test_refuses_a_changelog_without_the_section(self):
        (self.repo / "CHANGELOG.md").write_text("# Changelog\n\n## Unreleased\n")
        git(self.repo, "commit", "-q", "-am", "Change")
        result = self.run_script(VERSION, "--dry-run", "--partial-gate", "x")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("CHANGELOG.md has no heading", result.stderr)

    def test_refuses_a_dirty_tree(self):
        (self.repo / "Cargo.toml").write_text((self.repo / "Cargo.toml").read_text() + "\n")
        result = self.run_script(VERSION, "--dry-run", "--partial-gate", "x")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not clean", result.stderr)

    def test_refuses_an_existing_tag(self):
        git(self.repo, "tag", f"v{VERSION}")
        result = self.run_script(VERSION, "--dry-run", "--partial-gate", "x")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exists", result.stderr)

    def test_refuses_another_branch(self):
        git(self.repo, "checkout", "-q", "-b", "work")
        result = self.run_script(VERSION, "--dry-run", "--partial-gate", "x")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("branch main", result.stderr)

    def test_refuses_a_bad_archive(self):
        make_archive(self.repo / "dist", "amd64", files=["pupitre"])
        result = self.run_script(VERSION, "--dry-run", "--partial-gate", "x")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("content of dist", result.stderr)

    def test_refuses_a_wrong_checksum(self):
        sha = self.repo / "dist" / f"pupitre_{VERSION}_linux_amd64.tar.gz.sha256"
        sha.write_text("0" * 64 + f"  pupitre_{VERSION}_linux_amd64.tar.gz\n")
        result = self.run_script(VERSION, "--dry-run", "--partial-gate", "x")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum", result.stderr)

    def test_refuses_a_script_that_pushes_an_image(self):
        (self.repo / "scripts/bad.sh").write_text("docker " + "push example\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "Bad")
        result = self.run_script(VERSION, "--dry-run", "--partial-gate", "x")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must not publish container images", result.stderr)


if __name__ == "__main__":
    unittest.main()
