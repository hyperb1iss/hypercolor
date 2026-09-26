#!/usr/bin/env bash
# Packaging tests for scripts/dist.sh and scripts/verify-release-artifact.sh.
#
# Set HYPERCOLOR_RELEASE_TEST_CLI to a built `hypercolor` CLI for this host
# to package it as the fixture's bin/hypercolor; the tests then also run the
# Rust candidate validator on the producer's output, with and without the
# durable store inventory.
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

    INVENTORY = "share/hypercolor/durable-stores.json"

    def write_inventory(self, text):
        """Replace the shipped inventory and rebind its member entry."""
        path = self.payload / self.INVENTORY
        path.chmod(0o644)
        path.write_text(text)
        manifest = self.manifest()
        data = path.read_bytes()
        for member in manifest["members"]:
            if member["path"] == self.INVENTORY:
                member["size"] = len(data)
                member["sha256"] = hashlib.sha256(data).hexdigest()
        self.save_manifest(manifest)

    def assert_rejected(self, message):
        result = self.repack()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(message, result.stderr)

    def dist(self, *extra):
        """Run the producer on the class fixture tree with extra options."""
        fixture = self.root / "source"
        return subprocess.run(
            [
                "bash", str(fixture / "scripts/dist.sh"), "--ci", "--skip-docs",
                "--web-assets", str(self.root / "web-assets"),
                "--bin-dir", str(self.root / "probe-binaries"),
                "--version", "1.0.0-overlay", *extra,
            ],
            capture_output=True, text=True, check=False,
        )

    def test_producer_ships_the_durable_store_inventory(self):
        shipped = json.loads((self.payload / self.INVENTORY).read_text())
        inventory = json.loads((SOURCE / "packaging/managed/durable-stores.json").read_text())
        self.assertEqual(shipped, inventory)
        self.assertGreater(len(inventory["stores"]), 0)
        member = next(m for m in self.manifest()["members"] if m["path"] == self.INVENTORY)
        self.assertEqual((member["type"], member["mode"]), ("file", 0o644))
        self.assertNotIn("managed_package", self.manifest())
        result = self.repack()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    @unittest.skipUnless(REAL_CLI, "HYPERCOLOR_RELEASE_TEST_CLI is not set")
    def test_rust_validator_accepts_the_producer_output(self):
        validated = self.rust_validate()
        self.assertEqual(validated.returncode, 0, validated.stdout + validated.stderr)

    def test_a_release_without_the_inventory_still_verifies(self):
        (self.payload / self.INVENTORY).unlink()
        manifest = self.manifest()
        manifest["members"] = [m for m in manifest["members"] if m["path"] != self.INVENTORY]
        self.save_manifest(manifest)
        result = self.repack()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        if REAL_CLI:
            validated = self.rust_validate()
            self.assertEqual(validated.returncode, 0, validated.stdout + validated.stderr)

    def test_store_declarations_are_validated(self):
        original = json.loads((self.payload / self.INVENTORY).read_text())

        def store(**changes):
            entry = dict(original["stores"][0])
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
                self.write_inventory(json.dumps({"stores": stores}))
                self.assert_rejected(message)
        with self.subTest(case="extra top-level field"):
            self.write_inventory(json.dumps(dict(original, owner="x")))
            self.assert_rejected("must hold exactly its stores")
        with self.subTest(case="duplicated key"):
            text = json.dumps(original, indent=2)
            duplicated = text.replace('"stores": [', '"stores": [], "stores": [', 1)
            self.assertNotEqual(duplicated, text)
            self.write_inventory(duplicated)
            self.assert_rejected("has duplicated keys")

    def test_a_downstream_build_adds_its_own_stores_once(self):
        overlay = self.directory / "private-stores.json"
        extra = {
            "name": "cloud-state", "storage_format": "json", "readable_schema_min": 1,
            "readable_schema_max": 1, "written_schema": 1,
            "migration_mode": "backward_compatible",
        }
        overlay.write_text(json.dumps({"stores": [extra]}))
        result = self.dist("--target", "linux-amd64", "--durable-stores", str(overlay))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        produced = self.root / "source/dist/hypercolor-1.0.0-overlay-linux-amd64"
        stores = json.loads((produced / self.INVENTORY).read_text())["stores"]
        self.assertEqual(stores[-1], extra)
        inventory = json.loads((SOURCE / "packaging/managed/durable-stores.json").read_text())
        self.assertEqual(stores[:-1], inventory["stores"])

        overlay.write_text(json.dumps({"stores": [dict(extra, name="config")]}))
        result = self.dist("--target", "linux-amd64", "--durable-stores", str(overlay))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("declares store 'config' again", result.stdout + result.stderr)

        overlay.write_text(json.dumps({"stores": ["cloud-state"]}))
        result = self.dist("--target", "linux-amd64", "--durable-stores", str(overlay))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("as an object with a name", result.stdout + result.stderr)

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
