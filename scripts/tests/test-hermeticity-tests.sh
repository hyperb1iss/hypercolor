#!/usr/bin/env bash
set -euo pipefail

trap 'echo "test-hermeticity-tests.sh: exit $? at line $LINENO: $BASH_COMMAND" >&2' ERR

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
CHECK="$ROOT_DIR/scripts/check-test-hermeticity.sh"
SANDBOX="$(mktemp -d)"
trap 'find "$SANDBOX" -depth -delete 2>/dev/null || true' EXIT

# One crate per case; `place` writes a file relative to that crate.
case_root() {
  local root="$SANDBOX/$1"
  mkdir -p "$root/crates/demo/src" "$root/crates/demo/tests"
  printf '%s\n' "$root"
}

place() {
  local root="$1" relative="$2"
  mkdir -p "$(dirname "$root/crates/demo/$relative")"
  cat >"$root/crates/demo/$relative"
}

expect_pass() {
  local name="$1"
  "$CHECK" "$SANDBOX/$name" >/dev/null 2>"$SANDBOX/$name.err" || {
    echo "expected case '$name' to pass:" >&2
    cat "$SANDBOX/$name.err" >&2
    exit 1
  }
}

expect_fail() {
  local name="$1" needle="$2"
  if "$CHECK" "$SANDBOX/$name" >/dev/null 2>"$SANDBOX/$name.err"; then
    echo "expected case '$name' to fail" >&2
    exit 1
  fi
  grep -qF "$needle" "$SANDBOX/$name.err" || {
    echo "expected case '$name' to report '$needle':" >&2
    cat "$SANDBOX/$name.err" >&2
    exit 1
  }
}

# Scoped constructions, mDNS turned off, and names inside comments or
# strings all pass. Production code may browse the whole network.
root="$(case_root scoped)"
place "$root" tests/demo_tests.rs <<'RS'
fn t() {
    let publisher = ServiceDaemon::new().expect("daemon");
    MdnsScope::Loopback.restrict(&publisher).expect("scope");
    let browser = MdnsBrowser::with_scope(MdnsScope::Loopback);
    let scanner = WledScanner::with_known_ips(Vec::new(), Some(MdnsScope::Loopback), timeout);
    let quiet = WledScanner::with_known_ips(vec![ip], None, timeout);
    let host = HostPorts::isolated(&claim);
    let hue = HueScanner::with_options(Vec::new(), store, timeout, false, None)
        .with_nupnp_url(url);
    // MdnsBrowser::new() is named here only in a comment.
    let text = "UsbScanner::new()";
}
RS
place "$root" src/scanner.rs <<'RS'
fn browse() { let _ = MdnsBrowser::new(); let _ = UsbScanner::new(); }
RS
expect_pass scoped

# The WLED regression: the real test file with its scanners widened to the
# network still restricts its publisher, which must not excuse them.
root="$(case_root wled-regression)"
sed 's/Some(MdnsScope::Loopback)/Some(MdnsScope::Network)/' \
  "$ROOT_DIR/crates/hypercolor-driver-wled/tests/wled_protocol_tests.rs" |
  place "$root" tests/wled_protocol_tests.rs
expect_fail wled-regression "WledScanner::with_known_ips browses mDNS"
[[ "$(grep -c 'WledScanner::with_known_ips browses mDNS' "$SANDBOX/wled-regression.err")" == 2 ]] || {
  echo "expected both unscoped WLED scanners to be reported:" >&2
  cat "$SANDBOX/wled-regression.err" >&2
  exit 1
}

# A scope on one daemon never covers a second one.
root="$(case_root second-daemon)"
place "$root" tests/demo_tests.rs <<'RS'
fn t() {
    let first = ServiceDaemon::new().expect("daemon");
    MdnsScope::Loopback.restrict(&first).expect("scope");
    let second = ServiceDaemon::new().expect("daemon");
}
RS
expect_fail second-daemon "never calls MdnsScope::Loopback.restrict(&second)"

# A restriction in another test never covers this one, even on the same name.
root="$(case_root other-test)"
place "$root" tests/demo_tests.rs <<'RS'
fn first() {
    let daemon = ServiceDaemon::new().expect("daemon");
    daemon.register(info);
}

fn second() {
    let daemon = ServiceDaemon::new().expect("daemon");
    MdnsScope::Loopback.restrict(&daemon).expect("scope");
}
RS
expect_fail other-test "tests/demo_tests.rs:2: ServiceDaemon::new never calls MdnsScope::Loopback.restrict(&daemon)"

# Restricting after the daemon already reached the network is too late.
root="$(case_root late-restriction)"
place "$root" tests/demo_tests.rs <<'RS'
fn t() {
    let daemon = ServiceDaemon::new().expect("daemon");
    daemon.register(info);
    MdnsScope::Loopback.restrict(&daemon).expect("scope");
}
RS
expect_fail late-restriction "ServiceDaemon::new uses daemon before MdnsScope::Loopback.restrict(&daemon)"

# Inline cfg(test) modules count as test code.
root="$(case_root inline-module)"
place "$root" src/lib.rs <<'RS'
pub fn production() {}

#[cfg(test)]
mod tests {
    #[test]
    fn browses() {
        let index = HidUsageIndex::enumerate();
    }
}
RS
expect_fail inline-module "src/lib.rs:7: HidUsageIndex::enumerate enumerates the host HID stack"

# Scanners with no hermetic form fail in test code.
root="$(case_root forbidden)"
place "$root" tests/demo_tests.rs <<'RS'
fn t() {
    let govee = GoveeLanScanner::new(Vec::new(), timeout);
    let hue = HueScanner::with_options(Vec::new(), store, timeout, false, None);
    let host = HostPorts::native();
    let wled = WledScanner::new();
}
RS
expect_fail forbidden "GoveeLanScanner::new multicasts a Govee scan"
expect_fail forbidden "HueScanner::with_options asks the Hue cloud"
expect_fail forbidden "HostPorts::native composes the real host"
expect_fail forbidden "WledScanner::new browses mDNS"

# A missing crates directory is an error, not a clean pass.
mkdir -p "$SANDBOX/empty"
expect_fail empty "cannot scan"

"$CHECK" "$ROOT_DIR" >/dev/null
echo "test hermeticity check tests passed"
