#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

input_version="${1:-}"

cargo_version() {
  cargo metadata --format-version 1 --no-deps | python3 -c '
import json
import sys

metadata = json.load(sys.stdin)
for package in metadata["packages"]:
    if package["name"] == "hypercolor-daemon":
        print(package["version"])
        break
else:
    raise SystemExit("hypercolor-daemon package not found")
'
}

version="${input_version#v}"
if [[ "${GITHUB_REF_TYPE:-}" == "tag" && -n "${GITHUB_REF_NAME:-}" ]]; then
  version="${GITHUB_REF_NAME#v}"
elif [[ -z "${version}" ]]; then
  version="$(cargo_version)-ci.0"
fi

if [[ ! "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$ ]]; then
  echo "Invalid release version: ${version}" >&2
  exit 1
fi

base_version="${version%%-*}"
package_version="$(cargo_version)"
if [[ "${base_version}" != "${package_version}" ]]; then
  echo "Release version ${version} does not match Cargo version ${package_version}" >&2
  exit 1
fi

printf '%s\n' "${version}"
