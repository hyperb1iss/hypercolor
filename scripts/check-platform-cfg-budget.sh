#!/usr/bin/env bash
# Hold shared engine code to its platform-cfg budget (Design 72).
#
# Platform crates own native OS calls. Inside hypercolor-core and the
# daemon, platform branches belong at the composition roots AGENTS.md
# lists, so the number of non-test files that branch on platform may only
# shrink. A file counts when any cfg predicate it carries (an attribute,
# cfg!, or cfg_attr, single- or multi-line, inside all/any/not) names
# target_os, target_family, unix, or windows. Comments and strings never
# count, and neither do test files or cfg(test) items.
#
# Usage: check-platform-cfg-budget.sh [--budget N] [repo_root]
set -euo pipefail

budget=34
if [[ "${1:-}" == "--budget" ]]; then
  budget="$2"
  shift 2
fi
repo_root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec python3 -B - "$repo_root" "$script_dir/lib" "$budget" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
sys.path.insert(0, sys.argv[2])
budget = int(sys.argv[3])
from rust_source import RustFile, closing, is_test_predicate, rust_files  # noqa: E402

PREDICATE = re.compile(r"\bcfg(?:_attr)?\s*!?\s*\(")
PLATFORM = re.compile(r"\b(target_os|target_family|unix|windows)\b")
SCANNED = ["crates/hypercolor-core/src", "crates/hypercolor-daemon/src"]

try:
    paths = rust_files(root, SCANNED)
except FileNotFoundError as error:
    sys.exit(f"platform cfg budget check failed: {error}")

counted = []
for path in paths:
    try:
        rust = RustFile.load(root, path)
    except (OSError, UnicodeDecodeError, ValueError) as error:
        sys.exit(f"platform cfg budget check failed: cannot scan {path}: {error}")
    if rust.whole_file_is_test:
        continue
    for match in PREDICATE.finditer(rust.masked):
        predicate = rust.masked[match.end() : closing(rust.masked, match.end() - 1)]
        if rust.in_test_code(match.start()) or is_test_predicate(predicate):
            continue
        if PLATFORM.search(predicate):
            counted.append(rust.relative)
            break

count = len(counted)
if count > budget:
    print(
        f"platform cfg budget exceeded: {count} files in core and daemon, budget {budget}",
        file=sys.stderr,
    )
    for relative in counted:
        print(f"  {relative}", file=sys.stderr)
    print(
        "Move the platform branch behind a platform crate seam "
        "(docs/design/72-cross-platform-boundary-review.md).",
        file=sys.stderr,
    )
    sys.exit(1)
print(f"platform cfg budget: {count}/{budget} files in core and daemon")
if count < budget:
    print(f"  the count fell; lower the budget in scripts/check-platform-cfg-budget.sh to {count}")
PY
