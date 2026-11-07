#!/usr/bin/env bash
set -euo pipefail

trap 'echo "platform-cfg-budget-tests.sh: exit $? at line $LINENO: $BASH_COMMAND" >&2' ERR

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
CHECK="$ROOT_DIR/scripts/check-platform-cfg-budget.sh"
SANDBOX="$(mktemp -d)"
trap 'find "$SANDBOX" -depth -delete 2>/dev/null || true' EXIT

root="$SANDBOX/tree"
core="$root/crates/hypercolor-core/src"
daemon="$root/crates/hypercolor-daemon/src"
mkdir -p "$core/tests" "$daemon/nested"

write() { cat >"$1"; }

# Every predicate shape that names a platform counts once per file.
write "$daemon/plain.rs" <<'RS'
#[cfg(target_os = "linux")]
fn plain() {}
RS
write "$daemon/all.rs" <<'RS'
#[cfg(all(feature = "wgpu", target_os = "macos"))]
fn all() {}
RS
write "$daemon/multiline.rs" <<'RS'
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
))]
fn multiline() {}
RS
write "$daemon/negated.rs" <<'RS'
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn negated() {}
RS
write "$daemon/macro.rs" <<'RS'
fn macro_form() -> bool { cfg!(target_os = "windows") }
RS
write "$daemon/attr.rs" <<'RS'
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn attr() {}
RS
write "$daemon/family.rs" <<'RS'
#[cfg(target_family = "unix")]
fn family() {}
RS
write "$core/unix.rs" <<'RS'
#[cfg(unix)]
fn unix_only() {}
RS
write "$daemon/nested/windows.rs" <<'RS'
#[cfg(all(windows, feature = "x"))]
fn windows_only() {}
RS

# None of these count: comments, strings, test files, cfg(test) items, and
# predicates without a platform.
write "$core/prose.rs" <<'RS'
// #[cfg(target_os = "linux")] is described here, not used.
/* cfg(windows) */
const NOTE: &str = "#[cfg(target_os = \"macos\")]";
#[cfg(feature = "unix-socket")]
fn named_after_unix() {}
RS
write "$core/tests/fixture.rs" <<'RS'
#[cfg(target_os = "linux")]
fn fixture() {}
RS
write "$core/tests.rs" <<'RS'
#[cfg(windows)]
fn fixture() {}
RS
write "$core/inline.rs" <<'RS'
pub fn production() {}

#[cfg(all(test, unix))]
fn test_helper() {}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_only() {}
}
RS

output="$("$CHECK" --budget 9 "$root")"
[[ "$output" == *"9/9"* ]] || {
  echo "expected nine counted files at budget nine, got: $output" >&2
  exit 1
}

if "$CHECK" --budget 8 "$root" 2>"$SANDBOX/over.err"; then
  echo "expected nine files to exceed a budget of eight" >&2
  exit 1
fi
grep -q "budget exceeded: 9 files" "$SANDBOX/over.err" || {
  echo "expected the failure to name the count:" >&2
  cat "$SANDBOX/over.err" >&2
  exit 1
}
for listed in plain.rs multiline.rs negated.rs macro.rs attr.rs family.rs nested/windows.rs; do
  grep -q "crates/hypercolor-daemon/src/$listed" "$SANDBOX/over.err" || {
    echo "expected $listed in the failure listing:" >&2
    cat "$SANDBOX/over.err" >&2
    exit 1
  }
done
if grep -qE "prose.rs|inline.rs|tests" "$SANDBOX/over.err"; then
  echo "comments, strings, and test code must not count:" >&2
  cat "$SANDBOX/over.err" >&2
  exit 1
fi

# A tree the check cannot scan is an error, not a zero count.
mkdir -p "$SANDBOX/missing"
if "$CHECK" --budget 99 "$SANDBOX/missing" 2>"$SANDBOX/missing.err"; then
  echo "expected a missing source tree to fail" >&2
  exit 1
fi
grep -q "cannot scan" "$SANDBOX/missing.err" || {
  echo "expected the failure to say what it could not scan:" >&2
  cat "$SANDBOX/missing.err" >&2
  exit 1
}

echo "platform cfg budget tests passed"
