#!/usr/bin/env bash
# Fail when a crate vendored through [patch.crates-io] no longer resolves to
# its vendor/ copy. Cargo only warns about an unused patch and builds the
# unpatched crate from crates.io instead, so a dependency bump that moves
# past the vendored version would otherwise ship without the patches.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

python3 - "$repo_root" Cargo.toml crates/hypercolor-ui/Cargo.toml <<'PY'
import pathlib
import sys
import tomllib

root = pathlib.Path(sys.argv[1])
failures = []
checked = 0

for manifest_rel in sys.argv[2:]:
    manifest = root / manifest_rel
    patches = tomllib.loads(manifest.read_text()).get("patch", {}).get("crates-io", {})
    vendored = {
        name: spec["path"]
        for name, spec in patches.items()
        if isinstance(spec, dict) and "path" in spec
    }
    if not vendored:
        continue

    lockfile = manifest.parent / "Cargo.lock"
    lock = tomllib.loads(lockfile.read_text())
    lock_rel = lockfile.relative_to(root)
    unused = {entry["name"] for entry in lock.get("patch", {}).get("unused", [])}

    for name, path in sorted(vendored.items()):
        checked += 1
        vendor_dir = (manifest.parent / path).resolve()
        vendor_rel = vendor_dir.relative_to(root)
        version = tomllib.loads((vendor_dir / "Cargo.toml").read_text())["package"]["version"]
        entries = [pkg for pkg in lock.get("package", []) if pkg["name"] == name]
        from_registry = sorted(pkg["version"] for pkg in entries if "source" in pkg)
        from_vendor = [pkg for pkg in entries if "source" not in pkg and pkg["version"] == version]

        if name in unused:
            failures.append(f"{lock_rel}: the patch for {name} ({vendor_rel}) is unused")
        if from_registry:
            failures.append(
                f"{lock_rel}: {name} {', '.join(from_registry)} resolves from crates.io, "
                f"not {vendor_rel} ({version})"
            )
        if not from_vendor:
            failures.append(f"{lock_rel}: no {name} {version} from {vendor_rel}")

if failures:
    for failure in failures:
        print(f"error: {failure}", file=sys.stderr)
    print(
        "A dependency bump moved past a vendored crate. Re-vendor it at the new "
        "version with its patches reapplied, or drop the vendor once upstream "
        "carries the fixes (see the notes in its vendor directory).",
        file=sys.stderr,
    )
    sys.exit(1)

print(f"vendored patches: {checked} crate(s) resolve to vendor/")
PY
