#!/usr/bin/env python3
"""Build one qualification release archive from a published release tarball.

The archive keeps every member of the base release except three: the daemon
becomes the qualification daemon, the CLI becomes the build under test, and
``manifest.json`` names the qualification version with both new digests.
Every other byte, mode and asset count is the published release's, so the
installer validates the archive exactly as it validates a real one. Distinct
versions give distinct manifest digests and therefore distinct units.

A published release from before the managed package contract carries no
``managed_package`` block; the manifest gains the one ``scripts/dist.sh``
writes, with the store inventory from ``--stores``, because the installer
under test refuses a Linux candidate without it.

Prints the SHA-256 of the new ``manifest.json`` (the installer's expected
manifest digest) and writes it beside the archive.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import sys
import tarfile
from pathlib import Path

VERSION_PATTERN = re.compile(r"^[A-Za-z0-9._+-]{1,128}$")


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", type=Path, required=True, help="published release .tar.gz")
    parser.add_argument("--cli", type=Path, required=True, help="bin/hypercolor to install")
    parser.add_argument("--daemon", type=Path, required=True, help="qualification daemon")
    parser.add_argument("--version", required=True, help="qualification release version")
    parser.add_argument("--out", type=Path, required=True, help="output .tar.gz")
    parser.add_argument(
        "--stores",
        type=Path,
        default=Path(__file__).resolve().parents[3] / "packaging/managed/durable-stores.json",
        help="durable store inventory for managed_package.compatibility",
    )
    args = parser.parse_args()

    if not VERSION_PATTERN.fullmatch(args.version):
        parser.error(f"version {args.version!r} is not a release identity")
    replacements = {
        "bin/hypercolor": args.cli.read_bytes(),
        "bin/hypercolor-daemon": args.daemon.read_bytes(),
    }

    with tarfile.open(args.base, "r:gz") as base:
        members = base.getmembers()
        roots = {member.name.split("/", 1)[0] for member in members}
        if len(roots) != 1:
            raise SystemExit(f"{args.base} must contain exactly one top-level directory")
        (root,) = roots
        manifest_member = base.getmember(f"{root}/manifest.json")
        manifest = json.loads(base.extractfile(manifest_member).read())

        manifest["version"] = args.version
        seen = set()
        for entry in manifest["members"]:
            data = replacements.get(entry["path"])
            if data is not None:
                entry["size"] = len(data)
                entry["sha256"] = digest(data)
                seen.add(entry["path"])
        missing = set(replacements) - seen
        if missing:
            raise SystemExit(f"base manifest lacks {sorted(missing)}")
        if "managed_package" not in manifest:
            manifest["managed_package"] = {
                "schema_version": 1,
                "owner": "linux-user-tarball",
                "launcher_contract": 1,
                "components": {
                    "daemon": "bin/hypercolor-daemon",
                    "cli": "bin/hypercolor",
                    "ui": "share/hypercolor/ui",
                    "bundled_effects": "share/hypercolor/effects/bundled",
                },
                "compatibility": json.loads(args.stores.read_text()),
            }
        manifest_bytes = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()

        new_root = f"hypercolor-{args.version}-linux-amd64"
        args.out.parent.mkdir(parents=True, exist_ok=True)
        partial = args.out.with_name(args.out.name + ".partial")
        with tarfile.open(partial, "w:gz", compresslevel=1) as out:
            for member in members:
                relative = member.name[len(root) :].lstrip("/")
                info = tarfile.TarInfo(f"{new_root}/{relative}" if relative else new_root)
                info.type = member.type
                info.mode = member.mode
                info.mtime = member.mtime
                info.uname = info.gname = ""
                info.uid = info.gid = 0
                if member.isdir():
                    out.addfile(info)
                    continue
                if not member.isfile():
                    raise SystemExit(f"unexpected non-regular member {member.name}")
                if relative == "manifest.json":
                    data = manifest_bytes
                elif relative in replacements:
                    data = replacements[relative]
                else:
                    data = base.extractfile(member).read()
                info.size = len(data)
                out.addfile(info, io.BytesIO(data))
        partial.replace(args.out)

    manifest_sha256 = digest(manifest_bytes)
    args.out.with_name(args.out.name + ".manifest-sha256").write_text(manifest_sha256 + "\n")
    print(manifest_sha256)
    return 0


if __name__ == "__main__":
    sys.exit(main())
