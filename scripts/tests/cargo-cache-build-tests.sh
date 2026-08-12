#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
WRAPPER="$ROOT_DIR/scripts/cargo-cache-build.sh"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

mkdir -p "$SANDBOX/bin" "$SANDBOX/caller"
cat >"$SANDBOX/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" >"$FAKE_CARGO_LOG.args"
if [ -v CARGO_TARGET_DIR ]; then
  printf '%s\n' "$CARGO_TARGET_DIR" >"$FAKE_CARGO_LOG.target-env"
else
  : >"$FAKE_CARGO_LOG.target-env"
fi
EOF
chmod +x "$SANDBOX/bin/cargo"
ln -s cargo "$SANDBOX/bin/trunk"

run_wrapper() {
  local log_name="$1"
  shift
  FAKE_CARGO_LOG="$SANDBOX/$log_name" \
    HYPERCOLOR_CACHE_DIR="$SANDBOX/cache" \
    HYPERCOLOR_NO_SCCACHE=1 \
    PATH="$SANDBOX/bin:$PATH" \
    "$WRAPPER" "$@" >/dev/null
}

assert_args() {
  local log_name="$1"
  shift
  printf '%s\n' "$@" >"$SANDBOX/expected"
  diff -u "$SANDBOX/expected" "$SANDBOX/$log_name.args"
}

relative_target="$SANDBOX/caller/relative-target"
(
  cd "$SANDBOX/caller"
  CARGO_TARGET_DIR=relative-target run_wrapper ambient cargo test -p hypercolor-core
)
assert_args ambient test --target-dir "$relative_target" -p hypercolor-core
test ! -s "$SANDBOX/ambient.target-env"

(
  cd "$SANDBOX/caller"
  CARGO_TARGET_DIR=ignored run_wrapper explicit \
    cargo +nightly test --target-dir explicit-target -p hypercolor-core
)
assert_args explicit +nightly test --target-dir explicit-target -p hypercolor-core
test ! -s "$SANDBOX/explicit.target-env"

(
  cd "$SANDBOX/caller"
  CARGO_TARGET_DIR=deny-target run_wrapper deny cargo deny check
)
assert_args deny deny check
test "$(<"$SANDBOX/deny.target-env")" = "deny-target"

(
  cd "$SANDBOX/caller"
  CARGO_TARGET_DIR=ui-target run_wrapper trunk env -u NO_COLOR trunk build --release
)
assert_args trunk build --release
test "$(<"$SANDBOX/trunk.target-env")" = "ui-target"

run_wrapper default
assert_args default build --target-dir "$ROOT_DIR/target" --workspace
test ! -s "$SANDBOX/default.target-env"

printf 'cargo-cache-build tests: PASS\n'
