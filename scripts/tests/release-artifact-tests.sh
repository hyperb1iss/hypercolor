#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
ROOT_DIR="${ROOT_DIR}" python3 - <<'PY'
import hashlib
import json
import os
import shutil
import stat
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

SOURCE = Path(os.environ["ROOT_DIR"])
ASSET_ROOTS = {
    "ui_files": "share/hypercolor/ui",
    "bundled_effect_files": "share/hypercolor/effects/bundled",
    "docs_files": "share/hypercolor/docs",
    "skill_files": "share/hypercolor/agents/skills",
    "agent_files": "share/hypercolor/agents/agents",
    "site_files": "share/hypercolor/site",
}


class ReleaseArtifactTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix="hypercolor-release-test-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name)
        fixture = cls.root / "source"
        (fixture / "scripts").mkdir(parents=True)
        shutil.copy2(SOURCE / "scripts/dist.sh", fixture / "scripts/dist.sh")
        for directory in ("bin", "desktop", "icons", "modules-load", "systemd"):
            shutil.copytree(SOURCE / "packaging" / directory, fixture / "packaging" / directory)
        shutil.copytree(SOURCE / "udev", fixture / "udev")
        for name in ("LICENSE", "NOTICE", "README.md"):
            shutil.copy2(SOURCE / name, fixture / name)
        for path in (".agents/skills/probe/SKILL.md", ".agents/agents/probe.md"):
            file = fixture / path
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text("Packaging fixture only.\n")
        binaries = cls.root / "probe-binaries"
        binaries.mkdir()
        # These probes only exercise packaging commands, never qualify native runtime.
        for name in ("hypercolor", "hypercolor-daemon", "hypercolor-app"):
            file = binaries / name
            file.write_text("#!/usr/bin/env sh\nexit 0\n")
            file.chmod(0o755)
        assets = cls.root / "web-assets"
        for path in ("ui/index.html", "effects/probe.html"):
            file = assets / path
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text("<!doctype html><title>Packaging probe</title>\n")
        result = subprocess.run(
            [
                "bash", str(fixture / "scripts/dist.sh"), "--ci", "--skip-docs",
                "--web-assets", str(assets), "--bin-dir", str(binaries),
                "--target", "linux-amd64", "--version", "1.0.0-fixture",
            ],
            capture_output=True, text=True, check=False,
        )
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        cls.produced = fixture / "dist/hypercolor-1.0.0-fixture-linux-amd64"

    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(dir=self.root, prefix="case-"))
        self.payload = self.directory / self.produced.name
        shutil.copytree(self.produced, self.payload)

    def manifest(self):
        return json.loads((self.payload / "manifest.json").read_text())

    def save_manifest(self, manifest):
        (self.payload / "manifest.json").write_text(json.dumps(manifest) + "\n")

    def repack(self):
        archive = self.directory / "candidate.tar.gz"
        with tarfile.open(archive, "w:gz") as bundle:
            bundle.add(self.payload, arcname=self.payload.name)
        checksum = self.directory / "candidate.tar.gz.sha256"
        checksum.write_text(f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n")
        return subprocess.run(
            ["bash", str(SOURCE / "scripts/verify-release-artifact.sh"), str(archive), str(checksum)],
            capture_output=True, text=True, check=False,
        )

    def test_producer_emits_all_six_declared_roots_including_empty_docs_and_site(self):
        manifest = self.manifest()
        for field, relative in ASSET_ROOTS.items():
            self.assertTrue((self.payload / relative).is_dir(), relative)
            actual = sum(p.is_file() for p in (self.payload / relative).rglob("*"))
            self.assertEqual(manifest["assets"][field], actual)
            self.assertTrue(any(
                member["path"] == relative and member["type"] == "directory"
                for member in manifest["members"]
            ))
        self.assertEqual(manifest["assets"]["docs_files"], 0)
        self.assertEqual(manifest["assets"]["site_files"], 0)
        result = self.repack()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_missing_declared_roots_are_rejected_even_with_matching_member_inventory(self):
        for relative in ASSET_ROOTS.values():
            with self.subTest(root=relative):
                target = self.payload / relative
                backup = self.directory / "root-backup"
                shutil.move(target, backup)
                original = self.manifest()
                changed = dict(original)
                changed["members"] = [
                    member for member in original["members"]
                    if member["path"] != relative
                    and not member["path"].startswith(relative + "/")
                ]
                self.save_manifest(changed)
                result = self.repack()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("manifest asset root is missing or not a directory", result.stderr)
                shutil.move(backup, target)
                self.save_manifest(original)

    def test_each_declared_count_must_match_actual_files(self):
        original = self.manifest()
        for field in ASSET_ROOTS:
            with self.subTest(field=field):
                changed = json.loads(json.dumps(original))
                changed["assets"][field] += 1
                self.save_manifest(changed)
                result = self.repack()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("manifest asset count is wrong", result.stderr)
        self.save_manifest(original)

    def test_zero_count_root_must_be_a_directory_not_a_manifested_file(self):
        relative = ASSET_ROOTS["site_files"]
        target = self.payload / relative
        target.rmdir()
        target.write_bytes(b"")
        manifest = self.manifest()
        for member in manifest["members"]:
            if member["path"] == relative:
                member.update(
                    type="file", mode=stat.S_IMODE(target.stat().st_mode),
                    size=0, sha256=hashlib.sha256(b"").hexdigest(),
                )
        self.save_manifest(manifest)
        result = self.repack()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("manifest asset root is missing or not a directory", result.stderr)

    def test_both_packaged_user_units_declare_user_service_identity(self):
        declaration = "Environment=HYPERCOLOR_SERVICE_IDENTITY=user_service:systemd:hypercolor.service"
        for name in ("hypercolor.service", "hypercolor.service.system"):
            with self.subTest(unit=name):
                text = (self.payload / "lib/systemd/user" / name).read_text()
                identities = [
                    line for line in text.splitlines()
                    if line.startswith("Environment=HYPERCOLOR_SERVICE_IDENTITY=")
                ]
                self.assertEqual(identities, [declaration])
                self.assertIn("Restart=on-failure", text)
                self.assertIn("WantedBy=default.target", text)


unittest.main(verbosity=2)
PY
