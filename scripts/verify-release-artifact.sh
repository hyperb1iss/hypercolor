#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
MACOS_SIGNING_ACTOR="${ROOT_DIR}/scripts/sign-macos-artifacts.sh"

if [[ "${1:-}" == "--macos-app" ]]; then
  app="${2:-}"
  dmg="${3:-}"
  provenance="${4:-}"
  target="${5:-}"
  [[ "$#" -eq 5 ]] || {
    echo "usage: scripts/verify-release-artifact.sh --macos-app <app> <dmg> <provenance> <target>" >&2
    exit 2
  }
  [[ -n "${APPLE_TEAM_ID:-}" ]] || {
    echo "APPLE_TEAM_ID is required for macOS release verification" >&2
    exit 1
  }
  "${MACOS_SIGNING_ACTOR}" verify-app \
    --app "${app}" \
    --dmg "${dmg}" \
    --provenance "${provenance}" \
    --target "${target}" \
    --team-id "${APPLE_TEAM_ID}"
  exit 0
fi

install_candidate=false
install_prefix=""
install_dir=""
no_service=false
archive_seen=false
checksum_seen=false
install_prefix_seen=false
install_dir_seen=false

if [[ "${1:-}" == "--install-candidate" ]]; then
  install_candidate=true
  shift
  tarball=""
  checksum_file=""
  while [[ "$#" -gt 0 ]]; do
    case "$1" in
      --archive)
        [[ "$#" -ge 2 && "$2" != --* && "${archive_seen}" == false ]] || {
          echo "--archive must be provided exactly once with a value" >&2
          exit 2
        }
        archive_seen=true
        tarball="$2"
        shift 2
        ;;
      --checksum)
        [[ "$#" -ge 2 && "$2" != --* && "${checksum_seen}" == false ]] || {
          echo "--checksum must be provided exactly once with a value" >&2
          exit 2
        }
        checksum_seen=true
        checksum_file="$2"
        shift 2
        ;;
      --install-prefix)
        [[ "$#" -ge 2 && "$2" != --* && "${install_prefix_seen}" == false ]] || {
          echo "--install-prefix must be provided exactly once with a value" >&2
          exit 2
        }
        install_prefix_seen=true
        install_prefix="$2"
        shift 2
        ;;
      --install-dir)
        [[ "$#" -ge 2 && "$2" != --* && "${install_dir_seen}" == false ]] || {
          echo "--install-dir must be provided exactly once with a value" >&2
          exit 2
        }
        install_dir_seen=true
        install_dir="$2"
        shift 2
        ;;
      --no-service)
        [[ "${no_service}" == false ]] || {
          echo "--no-service must be provided at most once" >&2
          exit 2
        }
        no_service=true
        shift
        ;;
      *)
        echo "unknown install-candidate option: $1" >&2
        exit 2
        ;;
    esac
  done
  if [[ -z "${tarball}" || -z "${checksum_file}" || -z "${install_prefix}" || -z "${install_dir}" ]]; then
    echo "usage: scripts/verify-release-artifact.sh --install-candidate --archive <tarball> --checksum <checksum> --install-prefix <prefix> --install-dir <bin-dir> [--no-service]" >&2
    exit 2
  fi
else
  tarball="${1:-}"
  checksum_file="${2:-${tarball}.sha256}"
fi

if [[ -z "${tarball}" ]]; then
  echo "usage: scripts/verify-release-artifact.sh <tarball> [checksum]" >&2
  exit 2
fi

for cmd in python3; do
  command -v "${cmd}" >/dev/null 2>&1 || {
    echo "missing required command: ${cmd}" >&2
    exit 1
  }
done

[[ -s "${tarball}" ]] || {
  echo "release tarball is missing or empty: ${tarball}" >&2
  exit 1
}
[[ -s "${checksum_file}" ]] || {
  echo "checksum file is missing or empty: ${checksum_file}" >&2
  exit 1
}

tmpdir="$(mktemp -d)"
cleanup() {
  rm -rf "${tmpdir}"
}
trap cleanup EXIT

extraction_identity="$(TARBALL="${tarball}" CHECKSUM="${checksum_file}" \
EXTRACT_DIR="${tmpdir}" python3 - <<'PY'
import hashlib
import os
import posixpath
import re
import shutil
import stat
import tarfile
from pathlib import Path

with open(os.environ["CHECKSUM"], encoding="utf-8") as checksum:
    fields = checksum.read().split()
if not fields or re.fullmatch(r"[a-fA-F0-9]{64}", fields[0]) is None:
    raise SystemExit(f"invalid SHA256 checksum in {os.environ['CHECKSUM']}")
expected = fields[0].lower()

flags = os.O_RDONLY
if hasattr(os, "O_CLOEXEC"):
    flags |= os.O_CLOEXEC
if hasattr(os, "O_NOFOLLOW"):
    flags |= os.O_NOFOLLOW
descriptor = os.open(os.environ["TARBALL"], flags)
destination = Path(os.environ["EXTRACT_DIR"])
snapshot_flags = os.O_RDWR | os.O_CREAT | os.O_EXCL
if hasattr(os, "O_CLOEXEC"):
    snapshot_flags |= os.O_CLOEXEC
snapshot_path = destination / ".verified-artifact.snapshot"
snapshot_descriptor = os.open(snapshot_path, snapshot_flags, 0o600)
if os.name == "posix":
    os.unlink(snapshot_path)

seen = set()
root = None
with (
    os.fdopen(descriptor, "rb") as artifact,
    os.fdopen(snapshot_descriptor, "w+b") as snapshot,
):
    if not stat.S_ISREG(os.fstat(artifact.fileno()).st_mode):
        raise SystemExit("release artifact must be a regular file")
    digest = hashlib.sha256()
    for chunk in iter(lambda: artifact.read(1024 * 1024), b""):
        digest.update(chunk)
        snapshot.write(chunk)
    actual = digest.hexdigest()
    if actual != expected:
        raise SystemExit(f"checksum mismatch for {os.environ['TARBALL']}")
    snapshot.flush()
    snapshot.seek(0)
    archive = tarfile.open(fileobj=snapshot, mode="r:gz")
    members = archive.getmembers()
    if not members:
        raise SystemExit("release tarball is empty")
    for member in members:
        normalized = posixpath.normpath(member.name)
        if (
            not member.name
            or member.name.startswith("/")
            or "\\" in member.name
            or normalized != member.name.rstrip("/")
            or normalized in (".", "..")
            or normalized.startswith("../")
        ):
            raise SystemExit(f"archive contains unsafe path: {member.name}")
        if normalized in seen:
            raise SystemExit(f"archive contains duplicate member: {normalized}")
        seen.add(normalized)
        top = normalized.split("/", 1)[0]
        if root is None:
            root = top
        elif root != top:
            raise SystemExit(f"archive contains multiple roots: {root}, {top}")
        if not (member.isdir() or member.isfile()):
            raise SystemExit(f"archive contains unsupported member type: {normalized}")
        if member.mode < 0 or member.mode & ~0o777:
            raise SystemExit(f"archive contains unsupported mode: {normalized}")
        if member.isdir() and member.mode & 0o700 != 0o700:
            raise SystemExit(f"archive contains unsafe directory mode: {normalized}")
    root_members = [member for member in members if member.name.rstrip("/") == root]
    if len(root_members) != 1 or not root_members[0].isdir():
        raise SystemExit(f"archive root must be one directory: {root}")

    manifest_name = f"{root}/manifest.json"
    manifest_members = [
        member
        for member in members
        if member.name.rstrip("/") == manifest_name
    ]
    if len(manifest_members) != 1 or not manifest_members[0].isfile():
        raise SystemExit("archive manifest must be one regular file")
    if manifest_members[0].size > 2 * 1024 * 1024:
        raise SystemExit("release manifest exceeds 2097152 bytes")
    manifest_source = archive.extractfile(manifest_members[0])
    if manifest_source is None:
        raise SystemExit("archive manifest cannot be read")
    manifest_digest = hashlib.sha256()
    manifest_size = 0
    for chunk in iter(lambda: manifest_source.read(1024 * 1024), b""):
        manifest_size += len(chunk)
        if manifest_size > 2 * 1024 * 1024:
            raise SystemExit("release manifest exceeds 2097152 bytes")
        manifest_digest.update(chunk)

    for member in members:
        path = destination / member.name.rstrip("/")
        if member.isdir():
            path.mkdir(parents=True, exist_ok=True)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            source = archive.extractfile(member)
            if source is None:
                raise SystemExit(f"archive file cannot be read: {member.name}")
            with path.open("xb") as output:
                shutil.copyfileobj(source, output)
        path.chmod(member.mode)
    archive.close()
print(root)
print(manifest_digest.hexdigest())
PY
)"
root_name="${extraction_identity%%$'\n'*}"
manifest_sha256="${extraction_identity#*$'\n'}"
if [[ "${root_name}" == "${manifest_sha256}" || ! "${manifest_sha256}" =~ ^[a-f0-9]{64}$ ]]; then
  echo "archive extraction did not return a valid manifest digest" >&2
  exit 1
fi
root_dir="${tmpdir}/${root_name}"
manifest="${root_dir}/manifest.json"
[[ -d "${root_dir}" ]] || {
  echo "archive root is missing after extraction: ${root_name}" >&2
  exit 1
}
[[ -s "${manifest}" ]] || {
  echo "manifest is missing or empty" >&2
  exit 1
}

# Icons ship as pre-rendered PNGs; the scalable SVG returns to this list
# once the traced vector lands (e1bd5c14).
required_files=(
  LICENSE
  NOTICE
  README.md
  manifest.json
  share/applications/hypercolor.desktop
  share/icons/hicolor/48x48/apps/hypercolor.png
  share/icons/hicolor/128x128/apps/hypercolor.png
  share/icons/hicolor/256x256/apps/hypercolor.png
)
for file in "${required_files[@]}"; do
  [[ -f "${root_dir}/${file}" ]] || {
    echo "missing release file: ${file}" >&2
    exit 1
  }
done

required_bins=(
  hypercolor-daemon
  hypercolor
  hypercolor-app
  hypercolor-tui
  hypercolor-open
)
for bin in "${required_bins[@]}"; do
  [[ -x "${root_dir}/bin/${bin}" ]] || {
    echo "missing executable: bin/${bin}" >&2
    exit 1
  }
done

ROOT_NAME="${root_name}" ROOT_DIR="${root_dir}" MANIFEST="${manifest}" python3 - <<'PY'
import hashlib
import json
import os
import re
import stat
from pathlib import Path

root_name = os.environ["ROOT_NAME"]
root = Path(os.environ["ROOT_DIR"])
with open(os.environ["MANIFEST"], encoding="utf-8") as handle:
    manifest = json.load(handle)

name = manifest.get("name")
version = manifest.get("version")
platform = manifest.get("platform")
rust_target = manifest.get("rust_target")
if not all(isinstance(value, str) and value for value in (name, version, platform, rust_target)):
    raise SystemExit("manifest identity fields must be non-empty strings")

expected_root = f"{name}-{version}-{platform}"
if root_name != expected_root:
    raise SystemExit(f"archive root {root_name!r} does not match manifest {expected_root!r}")

expected_bins = {
    "hypercolor-daemon",
    "hypercolor",
    "hypercolor-app",
    "hypercolor-tui",
    "hypercolor-open",
}
binaries = manifest.get("binaries")
if (
    not isinstance(binaries, list)
    or len(binaries) != len(expected_bins)
    or any(not isinstance(binary, str) for binary in binaries)
    or set(binaries) != expected_bins
):
    raise SystemExit("manifest binaries do not match the release payload")

assets = manifest.get("assets")
asset_roots = {
    "ui_files": "share/hypercolor/ui",
    "bundled_effect_files": "share/hypercolor/effects/bundled",
    "docs_files": "share/hypercolor/docs",
    "skill_files": "share/hypercolor/agents/skills",
    "agent_files": "share/hypercolor/agents/agents",
    "site_files": "share/hypercolor/site",
}
if not isinstance(assets, dict) or set(assets) != set(asset_roots):
    raise SystemExit("manifest assets must be an object")
for key, relative_root in asset_roots.items():
    value = assets.get(key)
    minimum = 0 if key in {"docs_files", "site_files"} else 1
    if type(value) is not int or value < minimum:
        raise SystemExit(f"manifest assets.{key} is invalid")
    asset_root = root / relative_root
    if not asset_root.is_dir() or asset_root.is_symlink():
        raise SystemExit(f"manifest asset root is missing or not a directory: {relative_root}")
    actual_count = sum(1 for path in asset_root.rglob("*") if path.is_file())
    if actual_count != value:
        raise SystemExit(f"manifest asset count is wrong for {relative_root}")

members = manifest.get("members")
if not isinstance(members, list) or not members:
    raise SystemExit("manifest members must be a non-empty array")
expected_paths = set()
for member in members:
    if not isinstance(member, dict):
        raise SystemExit("manifest member must be an object")
    relative = member.get("path")
    member_type = member.get("type")
    mode = member.get("mode")
    if (
        not isinstance(relative, str)
        or not relative
        or relative.startswith("/")
        or ".." in Path(relative).parts
        or relative == "manifest.json"
        or relative in expected_paths
    ):
        raise SystemExit(f"manifest member path is invalid or duplicated: {relative!r}")
    if type(mode) is not int or mode < 0 or mode > 0o777:
        raise SystemExit(f"manifest mode is invalid for {relative}")
    expected_paths.add(relative)
    path = root / relative
    metadata = path.lstat()
    if stat.S_IMODE(metadata.st_mode) != mode:
        raise SystemExit(f"manifest mode mismatch for {relative}")
    if member_type == "directory":
        if not stat.S_ISDIR(metadata.st_mode):
            raise SystemExit(f"manifest type mismatch for {relative}")
        if set(member) != {"path", "type", "mode"}:
            raise SystemExit(f"manifest directory fields are invalid for {relative}")
    elif member_type == "file":
        if not stat.S_ISREG(metadata.st_mode):
            raise SystemExit(f"manifest type mismatch for {relative}")
        if set(member) != {"path", "type", "mode", "size", "sha256"}:
            raise SystemExit(f"manifest file fields are invalid for {relative}")
        size = member.get("size")
        digest_value = member.get("sha256")
        if type(size) is not int or size < 0:
            raise SystemExit(f"manifest size is invalid for {relative}")
        if not isinstance(digest_value, str) or re.fullmatch(r"[a-f0-9]{64}", digest_value) is None:
            raise SystemExit(f"manifest digest is invalid for {relative}")
        if metadata.st_size != size:
            raise SystemExit(f"manifest size mismatch for {relative}")
        digest = hashlib.sha256()
        with path.open("rb") as file:
            for chunk in iter(lambda: file.read(1024 * 1024), b""):
                digest.update(chunk)
        if digest.hexdigest() != digest_value:
            raise SystemExit(f"manifest digest mismatch for {relative}")
    else:
        raise SystemExit(f"manifest type is invalid for {relative}")

actual_paths = {
    path.relative_to(root).as_posix()
    for path in root.rglob("*")
    if path.relative_to(root).as_posix() != "manifest.json"
}
if actual_paths != expected_paths:
    missing = sorted(expected_paths - actual_paths)
    unexpected = sorted(actual_paths - expected_paths)
    raise SystemExit(
        f"manifest member set mismatch: missing={missing}, unexpected={unexpected}"
    )
PY

platform="$(MANIFEST="${manifest}" python3 - <<'PY'
import json
import os

with open(os.environ["MANIFEST"], encoding="utf-8") as handle:
    print(json.load(handle)["platform"])
PY
)"

case "${platform}" in
  linux-*)
    [[ -f "${root_dir}/lib/systemd/user/hypercolor.service" ]] || {
      echo "missing Linux systemd unit" >&2
      exit 1
    }
    [[ -f "${root_dir}/lib/udev/rules.d/99-hypercolor.rules" ]] || {
      echo "missing Linux udev rules" >&2
      exit 1
    }
    [[ -f "${root_dir}/etc/modules-load.d/i2c-dev.conf" ]] || {
      echo "missing Linux modules-load config" >&2
      exit 1
    }
    ;;
  macos-*)
    [[ -f "${root_dir}/share/hypercolor/launchd/tech.hyperbliss.hypercolor.plist" ]] || {
      echo "missing macOS launchd plist" >&2
      exit 1
    }
    [[ -n "${APPLE_TEAM_ID:-}" ]] || {
      echo "APPLE_TEAM_ID is required for macOS release verification" >&2
      exit 1
    }
    case "${platform}" in
      macos-arm64) macos_target="aarch64-apple-darwin" ;;
      macos-amd64) macos_target="x86_64-apple-darwin" ;;
      *) echo "unsupported macOS platform: ${platform}" >&2; exit 1 ;;
    esac
    "${MACOS_SIGNING_ACTOR}" verify-standalone \
      --directory "${root_dir}" \
      --target "${macos_target}" \
      --team-id "${APPLE_TEAM_ID}"
    ;;
esac

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) host_platform="linux-amd64" ;;
  Linux-aarch64) host_platform="linux-arm64" ;;
  Darwin-arm64) host_platform="macos-arm64" ;;
  Darwin-x86_64) host_platform="macos-amd64" ;;
  *) host_platform="" ;;
esac

if [[ "${install_candidate}" == true ]]; then
  [[ "${host_platform}" == "${platform}" ]] || {
    echo "release platform ${platform} does not match host ${host_platform:-unknown}" >&2
    exit 1
  }
  candidate_args=(
    __install-release
    --install-prefix "${install_prefix}"
    --install-dir "${install_dir}"
    --expected-manifest-sha256 "${manifest_sha256}"
  )
  if [[ "${no_service}" == true ]]; then
    candidate_args+=(--no-service)
  fi
  "${root_dir}/bin/hypercolor" "${candidate_args[@]}"
elif [[ "${host_platform}" == "${platform}" ]]; then
  "${root_dir}/bin/hypercolor" --version >/dev/null
fi

echo "verified ${root_name}"
