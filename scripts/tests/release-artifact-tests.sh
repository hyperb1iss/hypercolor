#!/usr/bin/env bash
# Packaging tests for scripts/dist.sh and scripts/verify-release-artifact.sh.
#
# Set HYPERCOLOR_RELEASE_TEST_CLI to a built `hypercolor` CLI for this host
# to package it as the fixture's bin/hypercolor; the verifier then also runs
# the Rust candidate validator on the producer's output, and the parity
# tests run that validator on every rejected manifest too.
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
REAL_CLI = os.environ.get("HYPERCOLOR_RELEASE_TEST_CLI", "")
ASSET_ROOTS = {
    "ui_files": "share/hypercolor/ui",
    "bundled_effect_files": "share/hypercolor/effects/bundled",
    "docs_files": "share/hypercolor/docs",
    "skill_files": "share/hypercolor/agents/skills",
    "user_skill_files": "share/hypercolor/skills",
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
        for directory in ("bin", "desktop", "icons", "managed", "modules-load", "systemd"):
            shutil.copytree(SOURCE / "packaging" / directory, fixture / "packaging" / directory)
        shutil.copytree(SOURCE / "udev", fixture / "udev")
        for name in ("LICENSE", "NOTICE", "README.md"):
            shutil.copy2(SOURCE / name, fixture / name)
        for path in (
            ".agents/skills/probe/SKILL.md", ".agents/agents/probe.md",
            "skills/probe/SKILL.md",
        ):
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
        if REAL_CLI:
            shutil.copyfile(REAL_CLI, binaries / "hypercolor")
            (binaries / "hypercolor").chmod(0o755)
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

    def test_producer_emits_all_declared_roots_including_empty_docs_and_site(self):
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

    def rust_validate(self):
        """Run the packaged CLI's own candidate validator on this payload."""
        home = self.directory / "home"
        home.mkdir(exist_ok=True)
        digest = hashlib.sha256((self.payload / "manifest.json").read_bytes()).hexdigest()
        return subprocess.run(
            [
                str(self.payload / "bin/hypercolor"), "__install-release",
                "--install-prefix", str(home / ".local"),
                "--install-dir", str(home / ".local/bin"),
                "--expected-manifest-sha256", digest, "--validate-only",
            ],
            capture_output=True, text=True, check=False,
            env={**os.environ, "HOME": str(home)},
        )

    def assert_rejected(self, manifest, message):
        self.save_manifest(manifest)
        result = self.repack()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(message, result.stderr)
        if REAL_CLI:
            rust = self.rust_validate()
            self.assertNotEqual(rust.returncode, 0, rust.stdout + rust.stderr)
            self.assertIn("invalid release manifest", rust.stderr)

    def test_producer_declares_the_managed_package_from_the_store_inventory(self):
        managed = self.manifest()["managed_package"]
        self.assertEqual(managed["schema_version"], 1)
        self.assertEqual(managed["owner"], "linux-user-tarball")
        self.assertEqual(managed["launcher_contract"], 1)
        self.assertEqual(managed["components"], {
            "daemon": "bin/hypercolor-daemon",
            "cli": "bin/hypercolor",
            "ui": "share/hypercolor/ui",
            "bundled_effects": "share/hypercolor/effects/bundled",
        })
        inventory = json.loads((SOURCE / "packaging/managed/durable-stores.json").read_text())
        self.assertEqual(managed["compatibility"], inventory)
        self.assertGreater(len(inventory["stores"]), 0)
        result = self.repack()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    @unittest.skipUnless(REAL_CLI, "HYPERCOLOR_RELEASE_TEST_CLI is not set")
    def test_rust_validator_accepts_the_producer_output(self):
        rust = self.rust_validate()
        self.assertEqual(rust.returncode, 0, rust.stdout + rust.stderr)

    def test_linux_release_without_managed_package_is_rejected(self):
        manifest = self.manifest()
        del manifest["managed_package"]
        self.assert_rejected(manifest, "a Linux release must declare its managed_package")

    def test_unknown_top_level_fields_are_rejected(self):
        manifest = self.manifest()
        manifest["channel"] = "stable"
        self.save_manifest(manifest)
        result = self.repack()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("manifest has unknown fields", result.stderr)
        if REAL_CLI:
            rust = self.rust_validate()
            self.assertNotEqual(rust.returncode, 0, rust.stdout + rust.stderr)
            self.assertIn("unknown field", rust.stderr)

    def test_missing_wrong_and_unknown_components_are_rejected(self):
        original = self.manifest()
        cases = {
            "missing": (lambda c: c.pop("ui"), "must name exactly"),
            "extra": (lambda c: c.update(app="bin/hypercolor-app"), "must name exactly"),
            "wrong daemon": (
                lambda c: c.update(daemon="bin/hypercolor-app"), "must be bin/hypercolor-daemon",
            ),
            "wrong tree": (
                lambda c: c.update(ui="share/hypercolor/site"), "must be share/hypercolor/ui",
            ),
        }
        for label, (mutate, message) in cases.items():
            with self.subTest(case=label):
                manifest = json.loads(json.dumps(original))
                mutate(manifest["managed_package"]["components"])
                self.assert_rejected(manifest, message)
        self.save_manifest(original)

    def test_unknown_contract_owner_and_schema_are_rejected(self):
        original = self.manifest()
        for field, value, message in (
            ("schema_version", 2, "schema_version must be 1"),
            ("launcher_contract", 2, "launcher_contract must be 1"),
            ("owner", "distribution-package", "owner must be linux-user-tarball"),
        ):
            with self.subTest(field=field):
                manifest = json.loads(json.dumps(original))
                manifest["managed_package"][field] = value
                self.assert_rejected(manifest, message)
        manifest = json.loads(json.dumps(original))
        manifest["managed_package"]["signature"] = "unsigned"
        self.assert_rejected(manifest, "exactly its five contract fields")
        self.save_manifest(original)

    def test_store_declarations_are_validated(self):
        original = self.manifest()

        def store(**changes):
            entry = dict(original["managed_package"]["compatibility"]["stores"][0])
            entry.update(changes)
            return entry

        cases = {
            "empty": ([], "must declare 1..=64 stores"),
            "duplicate": ([store(), store()], "is declared twice"),
            "inverted range": (
                [store(readable_schema_min=3, readable_schema_max=2, written_schema=2)],
                "must read the schema it writes",
            ),
            "writes outside range": (
                [store(readable_schema_min=1, readable_schema_max=2, written_schema=3)],
                "must read the schema it writes",
            ),
            "unknown mode": ([store(migration_mode="eventually")], "unknown migration_mode"),
            "boolean schema": ([store(written_schema=True)], "must be a whole number"),
            "negative schema": ([store(readable_schema_min=-1)], "must be a whole number"),
            "bad name": ([store(name="Library")], "durable store name"),
            "bad format": ([store(storage_format="json lines")], "storage_format"),
            "extra field": ([dict(store(), note="x")], "exactly its six fields"),
        }
        for label, (stores, message) in cases.items():
            with self.subTest(case=label):
                manifest = json.loads(json.dumps(original))
                manifest["managed_package"]["compatibility"]["stores"] = stores
                self.assert_rejected(manifest, message)
        self.save_manifest(original)

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
