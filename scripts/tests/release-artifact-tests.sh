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

    def assert_rejected(self, manifest, message, rust):
        """Both validators refuse `manifest`, each for the named reason."""
        self.save_manifest(manifest)
        result = self.repack()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(message, result.stderr)
        if REAL_CLI:
            validated = self.rust_validate()
            self.assertNotEqual(validated.returncode, 0, validated.stdout + validated.stderr)
            self.assertIn("release candidate validation failed", validated.stderr)
            self.assertIn(rust, validated.stderr)

    def relabel(self, platform, rust_target):
        """Rename the payload so its archive root matches another label."""
        manifest = self.manifest()
        manifest["platform"] = platform
        manifest["rust_target"] = rust_target
        renamed = self.directory / f"hypercolor-{manifest['version']}-{platform}"
        self.payload.rename(renamed)
        self.payload = renamed
        return manifest

    def dist(self, *extra, version="1.0.0-overlay"):
        """Run the producer on the class fixture tree with extra options."""
        fixture = self.root / "source"
        return subprocess.run(
            [
                "bash", str(fixture / "scripts/dist.sh"), "--ci", "--skip-docs",
                "--web-assets", str(self.root / "web-assets"),
                "--bin-dir", str(self.root / "probe-binaries"),
                "--version", version, *extra,
            ],
            capture_output=True, text=True, check=False,
        )

    def test_producer_ships_companion_unit_templates(self):
        template = self.directory / "recover.service.in"
        template.write_text(
            "[Service]\nExecStart=@LAUNCHER@ --role update-executor -- __recover-release\n"
        )
        spec = self.directory / "companions.json"
        spec.write_text(json.dumps({"units": [{
            "unit": "hypercolor-update-recover.service", "source": str(template),
            "enable": True,
        }]}))
        result = self.dist(
            "--target", "linux-amd64", "--companion-units", str(spec),
            version="1.0.0-companions",
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        produced = self.root / "source/dist/hypercolor-1.0.0-companions-linux-amd64"
        manifest = json.loads((produced / "manifest.json").read_text())
        shipped = "share/hypercolor/systemd/hypercolor-update-recover.service.in"
        self.assertEqual(manifest["managed_package"]["companion_units"], [{
            "unit": "hypercolor-update-recover.service", "template": shipped, "enable": True,
        }])
        self.assertEqual((produced / shipped).read_text(), template.read_text())
        self.assertTrue(any(member["path"] == shipped for member in manifest["members"]))
        payload = self.directory / produced.name
        shutil.copytree(produced, payload)
        self.payload = payload
        result = self.repack()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        if REAL_CLI:
            validated = self.rust_validate()
            self.assertEqual(validated.returncode, 0, validated.stdout + validated.stderr)

    def test_companion_templates_must_render(self):
        spec = self.directory / "companions-relative.json"
        good = self.directory / "good.service.in"
        good.write_text("[Service]\nExecStart=@LAUNCHER@ --role update-executor\n")
        spec.write_text(json.dumps({"units": [{
            "unit": "hypercolor-update-recover.service", "source": "good.service.in",
            "enable": True,
        }]}))
        result = self.dist(
            "--target", "linux-amd64", "--companion-units", str(spec),
            version="1.0.0-relative",
        )
        self.assertEqual(result.returncode, 0, "a source relative to its spec file resolves: "
                         + result.stdout + result.stderr)

        bad = self.directory / "bad.service.in"
        bad.write_text("[Service]\nExecStart=@NOPE@\n")
        spec.write_text(json.dumps({"units": [{
            "unit": "hypercolor-update-recover.service", "source": str(bad), "enable": True,
        }]}))
        result = self.dist(
            "--target", "linux-amd64", "--companion-units", str(spec),
            version="1.0.0-unrenderable",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unknown placeholder @NOPE@", result.stdout + result.stderr)

        twice = {"unit": "hypercolor-update-recover.service", "source": "good.service.in",
                 "enable": True}
        spec.write_text(json.dumps({"units": [twice, twice]}))
        result = self.dist(
            "--target", "linux-amd64", "--companion-units", str(spec),
            version="1.0.0-twice",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("declared twice", result.stdout + result.stderr)

        large = self.directory / "large.service.in"
        large.write_text("[Service]\n" + "#" * (16 * 1024))
        spec.write_text(json.dumps({"units": [{
            "unit": "hypercolor-update-recover.service", "source": str(large), "enable": True,
        }]}))
        result = self.dist(
            "--target", "linux-amd64", "--companion-units", str(spec),
            version="1.0.0-large",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exceeds 16384 bytes", result.stdout + result.stderr)

        produced = self.root / "source/dist/hypercolor-1.0.0-relative-linux-amd64"
        payload = self.directory / produced.name
        shutil.copytree(produced, payload)
        self.payload = payload
        shipped = "share/hypercolor/systemd/hypercolor-update-recover.service.in"
        (payload / shipped).write_text("[Service]\nExecStart=@NOPE@\n")
        manifest = self.manifest()
        for member in manifest["members"]:
            if member["path"] == shipped:
                data = (payload / shipped).read_bytes()
                member["size"] = len(data)
                member["sha256"] = hashlib.sha256(data).hexdigest()
        self.assert_rejected(manifest, "unknown placeholder @NOPE@", "unknown placeholder @NOPE@")

    def test_companion_unit_declarations_are_validated(self):
        original = self.manifest()
        ui = "share/hypercolor/ui/index.html"

        def unit(name, template=ui, enable=False, **extra):
            return {"unit": name, "template": template, "enable": enable, **extra}

        cases = {
            "daemon unit": ([unit("hypercolor.service")], "must be named", "must be named"),
            "foreign name": ([unit("sshd.service")], "must be named", "must be named"),
            "enabled template": (
                [unit("hypercolor-activator@.service", enable=True)],
                "cannot be enabled without an instance", "cannot be enabled without an instance",
            ),
            "missing template": (
                [unit("hypercolor-recover.service", template="share/nope.in")],
                "must be a declared file", "must be a declared file",
            ),
            "duplicate": (
                [unit("hypercolor-recover.service"), unit("hypercolor-recover.service")],
                "declared twice", "declared twice",
            ),
            "extra field": (
                [unit("hypercolor-recover.service", note="x")],
                "exactly unit, template and enable", "unknown field `note`",
            ),
        }
        for label, (units, message, rust) in cases.items():
            with self.subTest(case=label):
                manifest = json.loads(json.dumps(original))
                manifest["managed_package"]["companion_units"] = units
                self.assert_rejected(manifest, message, rust)
        self.save_manifest(original)

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
        validated = self.rust_validate()
        self.assertEqual(validated.returncode, 0, validated.stdout + validated.stderr)

    def test_linux_release_without_managed_package_is_rejected(self):
        manifest = self.manifest()
        del manifest["managed_package"]
        self.assert_rejected(
            manifest, "a Linux release must declare its managed_package",
            "must declare its managed_package",
        )

    def test_any_label_but_macos_must_carry_the_block(self):
        original = self.manifest()
        for platform, target in (
            ("x86_64-unknown-linux-musl", "x86_64-unknown-linux-musl"),
            ("macos-arm64", "x86_64-unknown-linux-gnu"),
        ):
            with self.subTest(platform=platform, target=target):
                self.save_manifest(original)
                manifest = self.relabel(platform, target)
                del manifest["managed_package"]
                self.assert_rejected(
                    manifest, "only a macOS release may omit it",
                    "must declare its managed_package",
                )

    def test_only_a_linux_label_may_carry_the_block(self):
        manifest = self.relabel("macos-arm64", "aarch64-apple-darwin")
        self.save_manifest(manifest)
        result = self.repack()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("a macos-arm64 release cannot declare it", result.stderr)
        if REAL_CLI:
            validated = self.rust_validate()
            self.assertNotEqual(validated.returncode, 0)
            self.assertIn("a macos-arm64 release cannot declare it", validated.stderr)

    @unittest.skipUnless(REAL_CLI, "HYPERCOLOR_RELEASE_TEST_CLI is not set")
    def test_the_linux_installer_refuses_a_genuine_macos_release(self):
        manifest = self.relabel("macos-arm64", "aarch64-apple-darwin")
        del manifest["managed_package"]
        self.save_manifest(manifest)
        validated = self.rust_validate()
        self.assertNotEqual(validated.returncode, 0, validated.stdout + validated.stderr)
        self.assertIn("must declare its managed_package contract", validated.stderr)

    def test_duplicated_keys_are_rejected(self):
        text = json.dumps(self.manifest(), indent=2)
        duplicated = text.replace(
            '"owner": "linux-user-tarball"',
            '"owner": "linux-user-tarball",\n    "owner": "linux-user-tarball"',
            1,
        )
        self.assertNotEqual(duplicated, text)
        (self.payload / "manifest.json").write_text(duplicated + "\n")
        result = self.repack()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("manifest has duplicated keys", result.stderr)
        if REAL_CLI:
            validated = self.rust_validate()
            self.assertNotEqual(validated.returncode, 0)
            self.assertIn("duplicate field `owner`", validated.stderr)

    def test_unknown_top_level_fields_are_rejected(self):
        manifest = self.manifest()
        manifest["channel"] = "stable"
        self.assert_rejected(manifest, "manifest has unknown fields", "unknown field `channel`")

    def test_missing_wrong_and_unknown_components_are_rejected(self):
        original = self.manifest()
        cases = {
            "missing": (
                lambda c: c.pop("ui"), "must name exactly", "missing field `ui`",
            ),
            "extra": (
                lambda c: c.update(app="bin/hypercolor-app"), "must name exactly",
                "unknown field `app`",
            ),
            "wrong daemon": (
                lambda c: c.update(daemon="bin/hypercolor-app"),
                "must be bin/hypercolor-daemon", "must be bin/hypercolor-daemon",
            ),
            "wrong tree": (
                lambda c: c.update(ui="share/hypercolor/site"),
                "must be share/hypercolor/ui", "must be share/hypercolor/ui",
            ),
        }
        for label, (mutate, message, rust) in cases.items():
            with self.subTest(case=label):
                manifest = json.loads(json.dumps(original))
                mutate(manifest["managed_package"]["components"])
                self.assert_rejected(manifest, message, rust)
        self.save_manifest(original)

    def test_unknown_contract_owner_and_schema_are_rejected(self):
        original = self.manifest()
        for field, value, message, rust in (
            ("schema_version", 2, "schema_version must be 1", "schema_version 2 is not"),
            ("launcher_contract", 2, "launcher_contract must be 1", "launcher_contract 2 is not"),
            (
                "owner", "distribution-package", "owner must be linux-user-tarball",
                "is not \"linux-user-tarball\"",
            ),
        ):
            with self.subTest(field=field):
                manifest = json.loads(json.dumps(original))
                manifest["managed_package"][field] = value
                self.assert_rejected(manifest, message, rust)
        manifest = json.loads(json.dumps(original))
        manifest["managed_package"]["signature"] = "unsigned"
        self.assert_rejected(
            manifest, "exactly its five contract fields", "unknown field `signature`",
        )
        self.save_manifest(original)

    def test_store_declarations_are_validated(self):
        original = self.manifest()

        def store(**changes):
            entry = dict(original["managed_package"]["compatibility"]["stores"][0])
            entry.update(changes)
            return entry

        cases = {
            "empty": ([], "must declare 1..=64 stores", "must declare 1..=64 stores"),
            "duplicate": ([store(), store()], "is declared twice", "is declared twice"),
            "inverted range": (
                [store(readable_schema_min=3, readable_schema_max=2, written_schema=2)],
                "must read the schema it writes", "must read the schema it writes",
            ),
            "writes outside range": (
                [store(readable_schema_min=1, readable_schema_max=2, written_schema=3)],
                "must read the schema it writes", "must read the schema it writes",
            ),
            "unknown mode": (
                [store(migration_mode="eventually")], "unknown migration_mode",
                "unknown variant `eventually`",
            ),
            "boolean schema": (
                [store(written_schema=True)], "must be a whole number", "invalid type: boolean",
            ),
            "negative schema": (
                [store(readable_schema_min=-1)], "must be a whole number", "invalid value",
            ),
            "bad name": ([store(name="Library")], "durable store name", "durable store name"),
            "bad format": (
                [store(storage_format="json lines")], "storage_format", "storage_format",
            ),
            "extra field": (
                [dict(store(), note="x")], "exactly its six fields", "unknown field `note`",
            ),
        }
        for label, (stores, message, rust) in cases.items():
            with self.subTest(case=label):
                manifest = json.loads(json.dumps(original))
                manifest["managed_package"]["compatibility"]["stores"] = stores
                self.assert_rejected(manifest, message, rust)
        self.save_manifest(original)

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
        stores = json.loads((produced / "manifest.json").read_text())[
            "managed_package"]["compatibility"]["stores"]
        self.assertEqual(stores[-1], extra)
        inventory = json.loads((SOURCE / "packaging/managed/durable-stores.json").read_text())
        self.assertEqual(stores[:-1], inventory["stores"])

        overlay.write_text(json.dumps({"stores": [dict(extra, name="config")]}))
        result = self.dist("--target", "linux-amd64", "--durable-stores", str(overlay))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("declares store 'config' again", result.stdout + result.stderr)

    def test_an_unnamed_linux_target_is_refused_by_the_producer(self):
        result = self.dist("--target", "x86_64-unknown-linux-musl")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no release platform name", result.stdout + result.stderr)

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
