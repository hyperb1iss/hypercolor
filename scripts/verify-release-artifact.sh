#!/usr/bin/env bash
set -euo pipefail

tarball="${1:-}"
checksum_file="${2:-${tarball}.sha256}"

if [[ -z "${tarball}" ]]; then
  echo "usage: scripts/verify-release-artifact.sh <tarball> [checksum]" >&2
  exit 2
fi

for cmd in tar python3; do
  command -v "${cmd}" >/dev/null 2>&1 || {
    echo "missing required command: ${cmd}" >&2
    exit 1
  }
done

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print tolower($1)}'
  else
    command -v shasum >/dev/null 2>&1 || {
      echo "missing required command: sha256sum or shasum" >&2
      exit 1
    }
    shasum -a 256 "$1" | awk '{print tolower($1)}'
  fi
}

[[ -s "${tarball}" ]] || {
  echo "release tarball is missing or empty: ${tarball}" >&2
  exit 1
}
[[ -s "${checksum_file}" ]] || {
  echo "checksum file is missing or empty: ${checksum_file}" >&2
  exit 1
}

expected="$(awk 'NF { print tolower($1); exit }' "${checksum_file}")"
if [[ ! "${expected}" =~ ^[a-f0-9]{64}$ ]]; then
  echo "invalid SHA256 checksum in ${checksum_file}" >&2
  exit 1
fi

actual="$(sha256_file "${tarball}")"
if [[ "${actual}" != "${expected}" ]]; then
  echo "checksum mismatch for ${tarball}" >&2
  exit 1
fi

entries_file="$(mktemp)"
tmpdir="$(mktemp -d)"
cleanup() {
  rm -f "${entries_file}"
  rm -rf "${tmpdir}"
}
trap cleanup EXIT

tar tzf "${tarball}" > "${entries_file}"
if [[ ! -s "${entries_file}" ]]; then
  echo "release tarball is empty: ${tarball}" >&2
  exit 1
fi

root_name=""
while IFS= read -r entry; do
  [[ "${entry}" != /* ]] || {
    echo "archive contains absolute path: ${entry}" >&2
    exit 1
  }
  [[ "${entry}" != ".." && "${entry}" != ../* && "${entry}" != */../* && "${entry}" != */.. ]] || {
    echo "archive contains unsafe path segment: ${entry}" >&2
    exit 1
  }

  top="${entry%%/*}"
  if [[ -z "${root_name}" ]]; then
    root_name="${top}"
  elif [[ "${top}" != "${root_name}" ]]; then
    echo "archive contains multiple roots: ${root_name}, ${top}" >&2
    exit 1
  fi
done < "${entries_file}"

tar xzf "${tarball}" -C "${tmpdir}"
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

required_files=(
  LICENSE
  NOTICE
  README.md
  manifest.json
  share/applications/hypercolor.desktop
  share/icons/hicolor/scalable/apps/hypercolor.svg
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
  hypercolor-tray
  hypercolor-tui
  hypercolor-open
)
for bin in "${required_bins[@]}"; do
  [[ -x "${root_dir}/bin/${bin}" ]] || {
    echo "missing executable: bin/${bin}" >&2
    exit 1
  }
done

ROOT_NAME="${root_name}" MANIFEST="${manifest}" python3 - <<'PY'
import json
import os

root_name = os.environ["ROOT_NAME"]
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
    "hypercolor-tray",
    "hypercolor-tui",
    "hypercolor-open",
}
if set(manifest.get("binaries", [])) != expected_bins:
    raise SystemExit("manifest binaries do not match the release payload")

assets = manifest.get("assets")
if not isinstance(assets, dict):
    raise SystemExit("manifest assets must be an object")
for key in ("ui_files", "bundled_effect_files", "skill_files", "agent_files"):
    value = assets.get(key)
    if not isinstance(value, int) or value <= 0:
        raise SystemExit(f"manifest assets.{key} must be greater than zero")
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
    ;;
esac

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) host_platform="linux-amd64" ;;
  Linux-aarch64) host_platform="linux-arm64" ;;
  Darwin-arm64) host_platform="macos-arm64" ;;
  *) host_platform="" ;;
esac

if [[ "${host_platform}" == "${platform}" ]]; then
  "${root_dir}/bin/hypercolor" --version >/dev/null
fi

echo "verified ${root_name}"
