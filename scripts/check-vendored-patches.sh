#!/usr/bin/env bash
# Fail when a crate vendored through [patch.crates-io] no longer resolves to
# its vendor/ copy. Cargo only warns about an unused patch and builds the
# unpatched crate from crates.io instead, so a dependency bump that moves
# past the vendored version would otherwise ship without the patches.
#
# The check asks Cargo which package it actually resolves (`cargo pkgid`
# reads the lockfile and every config-level patch, offline), because
# Cargo.lock does not record where a path package lives. It guards against
# dependency drift, not deliberate local overrides: Cargo's `paths` config
# and source replacement are not checked.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

python3 - "$repo_root" Cargo.toml crates/hypercolor-ui/Cargo.toml <<'PY'
import pathlib
import subprocess
import sys
import tomllib
import urllib.parse

root = pathlib.Path(sys.argv[1]).resolve()
failures = []
checked = 0


def resolved(manifest, name):
    """Return (directory, version) of the package Cargo resolves for `name`."""
    result = subprocess.run(
        ["cargo", "pkgid", "--locked", "--offline", "--manifest-path", str(manifest), name],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise ValueError(result.stderr.strip() or f"cargo pkgid exited {result.returncode}")
    url, _, fragment = result.stdout.strip().partition("#")
    version = fragment.rpartition("@")[2]
    if not url.startswith("path+file://"):
        return url, version
    path = urllib.parse.unquote(urllib.parse.urlparse(url.removeprefix("path+")).path)
    return pathlib.Path(path).resolve(), version


for manifest_rel in sys.argv[2:]:
    manifest = root / manifest_rel
    patches = tomllib.loads(manifest.read_text()).get("patch", {}).get("crates-io", {})
    for spec in patches.values():
        if not isinstance(spec, dict) or "path" not in spec:
            continue
        checked += 1
        vendor_dir = (manifest.parent / spec["path"]).resolve()
        vendor_rel = vendor_dir.relative_to(root) if vendor_dir.is_relative_to(root) else vendor_dir
        package = tomllib.loads((vendor_dir / "Cargo.toml").read_text())["package"]
        name, version = package["name"], package["version"]
        try:
            source, resolved_version = resolved(manifest, name)
        except ValueError as error:
            failures.append(f"{manifest_rel}: cannot resolve {name} to one package: {error}")
            continue
        if source != vendor_dir or resolved_version != version:
            failures.append(
                f"{manifest_rel}: {name} resolves to {source} {resolved_version}, "
                f"not {vendor_rel} {version}"
            )

if failures:
    for failure in failures:
        print(f"error: {failure}", file=sys.stderr)
    print(
        "A dependency bump or override moved a vendored crate off its vendor/ "
        "copy. Re-vendor it at the new version with its patches reapplied, or "
        "drop the vendor once upstream carries the fixes (see the notes in its "
        "vendor directory).",
        file=sys.stderr,
    )
    sys.exit(1)

print(f"vendored patches: {checked} crate(s) resolve to vendor/")
PY
