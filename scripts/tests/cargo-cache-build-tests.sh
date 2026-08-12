#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
WRAPPER="$ROOT_DIR/scripts/cargo-cache-build.sh"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
unset CARGO_TARGET_DIR

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
if [ -n "${FAKE_BUILD_LOCK_PATH:-}" ]; then
  exec 9<>"$FAKE_BUILD_LOCK_PATH"
  if flock -n 9; then
    echo "target build lock was not held" >&2
    exit 9
  fi
fi
EOF
chmod +x "$SANDBOX/bin/cargo"
cat >"$SANDBOX/bin/trunk" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
cargo build "$@"
EOF
chmod +x "$SANDBOX/bin/trunk"
cat >"$SANDBOX/bin/sccache" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  --version)
    printf 'sccache 0.14.0\n'
    ;;
  --stop-server)
    printf 'stop\n' >>"$FAKE_SCCACHE_LOG"
    if [ -n "${FAKE_SCCACHE_STOP_READY:-}" ]; then
      : >"$FAKE_SCCACHE_STOP_READY"
      sleep "${FAKE_SCCACHE_STOP_DELAY:-0}"
    fi
    ;;
  --start-server)
    printf 'start\n' >>"$FAKE_SCCACHE_LOG"
    [ "${FAKE_SCCACHE_FAIL_START:-0}" -eq 0 ]
    ;;
esac
EOF
chmod +x "$SANDBOX/bin/sccache"

run_wrapper() {
  local log_name="$1"
  shift
  FAKE_CARGO_LOG="$SANDBOX/$log_name" \
    HYPERCOLOR_CACHE_DIR="$SANDBOX/cache" \
    HYPERCOLOR_NO_SCCACHE=1 \
    PATH="$SANDBOX/bin:$PATH" \
    "$WRAPPER" "$@" >/dev/null
}

run_refresh_wrapper() {
  local cache_name="$1"
  local log_name="$2"
  shift 2
  FAKE_CARGO_LOG="$SANDBOX/$log_name" \
    FAKE_SCCACHE_LOG="$SANDBOX/$cache_name.sccache.log" \
    HYPERCOLOR_CACHE_DIR="$SANDBOX/$cache_name" \
    HYPERCOLOR_FORCE_SCCACHE=1 \
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
  CARGO_TARGET_DIR=tauri-target run_wrapper tauri \
    cargo tauri build --config tauri.bundle.conf.json
)
assert_args tauri tauri build --config tauri.bundle.conf.json \
  -- --target-dir "$SANDBOX/caller/tauri-target"
test ! -s "$SANDBOX/tauri.target-env"

(
  cd "$SANDBOX/caller"
  CARGO_TARGET_DIR=ignored run_wrapper tauri-explicit \
    cargo tauri build -- --profile release --target-dir explicit-target
)
assert_args tauri-explicit tauri build \
  -- --profile release --target-dir explicit-target
test ! -s "$SANDBOX/tauri-explicit.target-env"

(
  cd "$SANDBOX/caller"
  CARGO_TARGET_DIR=deny-target run_wrapper deny cargo deny check
)
assert_args deny deny check
test "$(<"$SANDBOX/deny.target-env")" = "deny-target"

(
  cd "$SANDBOX/caller"
  CARGO_TARGET_DIR=ui-target run_wrapper trunk trunk build --release
)
assert_args trunk --config \
  "build.target-dir=\"$SANDBOX/caller/ui-target\"" \
  build build --release
test ! -s "$SANDBOX/trunk.target-env"

run_wrapper default
assert_args default build --target-dir "$ROOT_DIR/target" --workspace
test ! -s "$SANDBOX/default.target-env"

lock_target="$SANDBOX/lock-target"
mkdir -p "$lock_target"
FAKE_BUILD_LOCK_PATH="$lock_target/.cargo-build-lock" \
  CARGO_TARGET_DIR="$lock_target" run_wrapper build-lock cargo build
test -e "$lock_target/.cargo-build-lock"

run_refresh_wrapper refresh refresh-cargo cargo build -p hypercolor-core
printf 'stop\nstart\n' >"$SANDBOX/refresh.expected"
diff -u "$SANDBOX/refresh.expected" "$SANDBOX/refresh.sccache.log"
test -s "$SANDBOX/refresh/sccache-server-config"

set +e
FAKE_SCCACHE_FAIL_START=1 \
  run_refresh_wrapper refresh-failure refresh-failure-cargo cargo build \
  >"$SANDBOX/refresh-failure.stdout" 2>"$SANDBOX/refresh-failure.stderr"
refresh_failure_status=$?
set -e
test "$refresh_failure_status" -ne 0
test ! -e "$SANDBOX/refresh-failure/sccache-server-config"

ready="$SANDBOX/refresh-lock.ready"
FAKE_SCCACHE_STOP_READY="$ready" FAKE_SCCACHE_STOP_DELAY=0.4 \
  run_refresh_wrapper refresh-lock refresh-lock-first cargo build &
first_refresh_pid=$!
for _ in {1..50}; do
  [ -e "$ready" ] && break
  sleep 0.01
done
(
  run_refresh_wrapper refresh-lock refresh-lock-second cargo build
  : >"$SANDBOX/refresh-lock.second-done"
) &
second_refresh_pid=$!
sleep 0.1
test ! -e "$SANDBOX/refresh-lock.second-done"
wait "$first_refresh_pid"
wait "$second_refresh_pid"
test -e "$SANDBOX/refresh-lock.second-done"

printf 'cargo-cache-build tests: PASS\n'
