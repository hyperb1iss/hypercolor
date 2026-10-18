#!/usr/bin/env bash
set -euo pipefail

# Failures under set -e in this suite die silently in CI otherwise;
# name the dying command and line.
trap 'echo "cargo-cache-build-tests.sh: exit $? at line $LINENO: $BASH_COMMAND" >&2' ERR

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
WRAPPER="$ROOT_DIR/scripts/cargo-cache-build.sh"
SANDBOX="$(mktemp -d)"
signal_wrapper_pid=""
signal_child_pid=""
signal_grandchild_pid=""
kill_wrapper_pid=""
kill_child_pid=""
parallel_wrapper_one=""
parallel_wrapper_two=""
handoff_wrapper_pid=""
trap_wrapper_pid=""
cleanup() {
  for pid in "$signal_wrapper_pid" "$signal_child_pid" \
    "$signal_grandchild_pid" "$kill_wrapper_pid" "$kill_child_pid" \
    "$parallel_wrapper_one" "$parallel_wrapper_two" \
    "$handoff_wrapper_pid" "$trap_wrapper_pid"; do
    [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
  done
  rm -rf "$SANDBOX"
}
trap 'cleanup' EXIT
unset CARGO_TARGET_DIR
# Every case runs a copy of the wrapper whose checkout root is the sandbox,
# so a case that leaves the target dir unset resolves to a scratch target and
# the suite can never prune or lock this checkout's real one.
SANDBOX_CHECKOUT="$SANDBOX/checkout"
mkdir -p "$SANDBOX_CHECKOUT/scripts"
cp "$WRAPPER" "$ROOT_DIR/scripts/cargo-cache-lock.rs" \
  "$ROOT_DIR/scripts/cargo-cache-cargo-shim.sh" "$SANDBOX_CHECKOUT/scripts/"
WRAPPER="$SANDBOX_CHECKOUT/scripts/cargo-cache-build.sh"
# Quiet by default; the prune fixtures below opt back in.
export HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=0
# CI runs this suite after the cache action may have enabled the shared R2
# compiler cache; the fake server must start from a local-only configuration.
unset SCCACHE_BUCKET SCCACHE_ENDPOINT SCCACHE_REGION SCCACHE_S3_KEY_PREFIX \
  SCCACHE_S3_RW_MODE AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY

mkdir -p "$SANDBOX/bin" "$SANDBOX/caller"
cat >"$SANDBOX/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" >"$FAKE_CARGO_LOG.args"
printf '%s\n' "${CARGO_INCREMENTAL:-unset}" "${RUSTC_WRAPPER:-unset}" >"$FAKE_CARGO_LOG.mode"
# Distinguishes an unset RUSTC_WRAPPER from one set to an empty string;
# only the empty string overrides a build.rustc-wrapper from Cargo config.
printf '%s\n' "${RUSTC_WRAPPER-<unset>}" >"$FAKE_CARGO_LOG.wrapper-raw"
if [ -v CARGO_TARGET_DIR ]; then
  printf '%s\n' "$CARGO_TARGET_DIR" >"$FAKE_CARGO_LOG.target-env"
else
  : >"$FAKE_CARGO_LOG.target-env"
fi
if [ -n "${FAKE_BUILD_LOCK_PATH:-}" ]; then
  exec 10<>"$FAKE_BUILD_LOCK_PATH"
  if flock -n 10; then
    echo "target build lock was not held" >&2
    exit 9
  fi
fi
if [ -n "${FAKE_CARGO_EXPECT_GONE:-}" ]; then
  if [ -e "$FAKE_CARGO_EXPECT_GONE" ]; then
    printf 'present\n' >"$FAKE_CARGO_LOG.gone"
  else
    printf 'absent\n' >"$FAKE_CARGO_LOG.gone"
  fi
fi
if [ -n "${FAKE_LOCK_DESCENDANT_PID_FILE:-}" ]; then
  sleep 30 &
  printf '%s\n' "$!" >"$FAKE_LOCK_DESCENDANT_PID_FILE"
fi
if [ -n "${FAKE_FORBIDDEN_FDS:-}" ]; then
  for fd in $FAKE_FORBIDDEN_FDS; do
    if [ -e "/proc/$$/fd/$fd" ]; then
      echo "cargo inherited wrapper fd $fd" >&2
      exit 11
    fi
  done
fi
EOF
chmod +x "$SANDBOX/bin/cargo"
cat >"$SANDBOX/bin/trunk" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ -z "${FAKE_TRUNK_CARGO_PATH_LOG:-}" ] || command -v cargo >"$FAKE_TRUNK_CARGO_PATH_LOG"
cargo build "$@"
EOF
chmod +x "$SANDBOX/bin/trunk"
cat >"$SANDBOX/bin/read-stdin" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
IFS= read -r value
printf 'stdin:%s\n' "$value"
EOF
chmod +x "$SANDBOX/bin/read-stdin"
cat >"$SANDBOX/bin/read-loop" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ -z "${READ_LOOP_READY:-}" ] || : >"$READ_LOOP_READY"
printf 'loop-ready\n'
while IFS= read -r value; do
  printf 'loop:%s\n' "$value"
done
EOF
chmod +x "$SANDBOX/bin/read-loop"
cat >"$SANDBOX/bin/record-sleep" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$$" >"$RECORD_SLEEP_PID_FILE"
exec sleep "${RECORD_SLEEP_SECONDS:-30}"
EOF
chmod +x "$SANDBOX/bin/record-sleep"
cat >"$SANDBOX/bin/sccache" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  --version)
    if [ -n "${FAKE_SCCACHE_VERSION_STARTED:-}" ]; then
      : >"$FAKE_SCCACHE_VERSION_STARTED"
      sleep "${FAKE_SCCACHE_VERSION_DELAY:-0}"
    fi
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
    if [ -n "${SCCACHE_BUCKET:-}" ]; then
      printf 'start remote\n' >>"$FAKE_SCCACHE_LOG"
      [ "${FAKE_SCCACHE_FAIL_REMOTE:-0}" -eq 0 ] || exit 11
    else
      printf 'start\n' >>"$FAKE_SCCACHE_LOG"
    fi
    if [ -e "/proc/$$/fd/4" ] || [ -e "/proc/$$/fd/5" ] \
      || [ -e "/proc/$$/fd/7" ] || [ -e "/proc/$$/fd/9" ]; then
      echo "sccache inherited the configuration lock" >&2
      exit 10
    fi
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
    HYPERCOLOR_FORCE_PORTABLE_LOCK="${HYPERCOLOR_FORCE_PORTABLE_LOCK:-0}" \
    PATH="$SANDBOX/bin:$PATH" \
    "$WRAPPER" "$@" >/dev/null
}

assert_args() {
  local log_name="$1"
  shift
  printf '%s\n' "$@" >"$SANDBOX/expected"
  diff -u "$SANDBOX/expected" "$SANDBOX/$log_name.args"
}

# Exercise the policy through the real wrapper, including inherited CI state
# and explicit overrides. A build command must not discard local incrementals.
assert_mode() {
  local name="$1" incremental="$2" wrapper="$3"
  shift 3
  env -u CI -u CARGO_INCREMENTAL -u RUSTC_WRAPPER \
    -u HYPERCOLOR_FORCE_SCCACHE -u HYPERCOLOR_NO_SCCACHE -u HYPERCOLOR_ITERATE \
    FAKE_CARGO_LOG="$SANDBOX/$name" \
    FAKE_SCCACHE_LOG="$SANDBOX/mode.sccache.log" \
    HYPERCOLOR_CACHE_DIR="$SANDBOX/mode-cache" \
    CARGO_TARGET_DIR="$SANDBOX/mode-target" \
    PATH="$SANDBOX/bin:$PATH" \
    "$@" >/dev/null
  printf '%s\n' "$incremental" "$wrapper" >"$SANDBOX/expected-mode"
  diff -u "$SANDBOX/expected-mode" "$SANDBOX/$name.mode"
}

for subcommand in build test run check clippy; do
  assert_mode "local-$subcommand" 1 unset "$WRAPPER" cargo "$subcommand"
done
assert_mode local-nextest 1 unset "$WRAPPER" cargo nextest run
assert_mode ci-build 0 "$SANDBOX/bin/sccache" env CI=true "$WRAPPER" cargo build
assert_mode ci-test 0 "$SANDBOX/bin/sccache" env CI=1 "$WRAPPER" cargo test
assert_mode ci-nextest 0 "$SANDBOX/bin/sccache" env CARGO_INCREMENTAL=0 "$WRAPPER" cargo nextest run
assert_mode ci-check 0 unset env CARGO_INCREMENTAL=0 "$WRAPPER" cargo check
assert_mode forced-build 0 "$SANDBOX/bin/sccache" env HYPERCOLOR_FORCE_SCCACHE=1 "$WRAPPER" cargo build
assert_mode release-build 0 "$SANDBOX/bin/sccache" "$WRAPPER" cargo build --release
assert_mode override-ci 1 unset env CI=true HYPERCOLOR_ITERATE=1 "$WRAPPER" cargo build
assert_mode override-force 1 unset env CARGO_INCREMENTAL=1 HYPERCOLOR_FORCE_SCCACHE=1 "$WRAPPER" cargo test
assert_mode ambient-wrapper 1 unset env RUSTC_WRAPPER="$SANDBOX/bin/sccache" "$WRAPPER" cargo build

# Incremental mode must neutralize a config-level sccache wrapper (for
# example a workstation ~/.cargo/config.toml), which refuses every compile
# while CARGO_INCREMENTAL=1 is set. Cargo only lets an empty RUSTC_WRAPPER
# override the config, so the variable must be set and empty, not unset.
assert_wrapper_raw() {
  local name="$1" expected="$2"
  printf '%s\n' "$expected" >"$SANDBOX/expected-wrapper-raw"
  diff -u "$SANDBOX/expected-wrapper-raw" "$SANDBOX/$name.wrapper-raw"
}
assert_wrapper_raw local-check ""
assert_wrapper_raw local-build ""
assert_wrapper_raw ambient-wrapper ""
assert_mode other-wrapper 1 /usr/bin/env env RUSTC_WRAPPER=/usr/bin/env "$WRAPPER" cargo check
assert_wrapper_raw other-wrapper /usr/bin/env
assert_wrapper_raw release-build "$SANDBOX/bin/sccache"

relative_target="$SANDBOX/caller/relative-target"
(
  cd "$SANDBOX/caller"
  CARGO_TARGET_DIR=relative-target run_wrapper ambient cargo test -p hypercolor-core
)
assert_args ambient test --target-dir "$relative_target" -p hypercolor-core
test ! -s "$SANDBOX/ambient.target-env"

(
  cd "$SANDBOX/caller"
  CARGO_TARGET_DIR=nextest-target run_wrapper nextest \
    cargo nextest run --locked -p hypercolor-core
)
assert_args nextest nextest run --target-dir \
  "$SANDBOX/caller/nextest-target" --locked -p hypercolor-core
test ! -s "$SANDBOX/nextest.target-env"

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
  FAKE_TRUNK_CARGO_PATH_LOG="$SANDBOX/trunk.cargo-path" \
    CARGO_TARGET_DIR=ui-target run_wrapper trunk trunk build --release
)
assert_args trunk --config \
  "build.target-dir=\"$SANDBOX/caller/ui-target\"" \
  build build --release
test ! -s "$SANDBOX/trunk.target-env"
trunk_cargo_path="$(<"$SANDBOX/trunk.cargo-path")"
case "$trunk_cargo_path" in
  "$SANDBOX/caller/ui-target/.hypercolor-toolchain/"*/cargo) ;;
  *) exit 1 ;;
esac
test -f "$trunk_cargo_path"
test ! -L "$trunk_cargo_path"

(
  cd "$SANDBOX/caller"
  FAKE_TRUNK_CARGO_PATH_LOG="$SANDBOX/trunk-second.cargo-path" \
    CARGO_TARGET_DIR=ui-target-second run_wrapper trunk-second trunk build --release
)
second_trunk_cargo_path="$(<"$SANDBOX/trunk-second.cargo-path")"
test "$second_trunk_cargo_path" != "$trunk_cargo_path"
test -f "$trunk_cargo_path"
test -f "$second_trunk_cargo_path"

if script --version 2>&1 | grep -Fq 'util-linux'; then
  printf 'hello\n' | script -qec \
    "env HYPERCOLOR_CACHE_DIR=$SANDBOX/pty-cache HYPERCOLOR_NO_SCCACHE=1 CARGO_TARGET_DIR=$SANDBOX/pty-target PATH=$SANDBOX/bin:$PATH $WRAPPER read-stdin" \
    /dev/null >"$SANDBOX/pty-output"
  grep -Fq 'stdin:hello' "$SANDBOX/pty-output"

  printf 'nested\n' | script -qec \
    "env HYPERCOLOR_CACHE_DIR=$SANDBOX/nested-cache HYPERCOLOR_NO_SCCACHE=1 CARGO_TARGET_DIR=$SANDBOX/nested-target PATH=$SANDBOX/bin:$PATH bash -c '$WRAPPER read-stdin; printf nested-after\\n'" \
    /dev/null >"$SANDBOX/nested-pty-output"
  grep -Fq 'stdin:nested' "$SANDBOX/nested-pty-output"
  grep -Fq 'nested-after' "$SANDBOX/nested-pty-output"

  redirected_command="env HYPERCOLOR_CACHE_DIR=$SANDBOX/redirected-cache HYPERCOLOR_NO_SCCACHE=1 CARGO_TARGET_DIR=$SANDBOX/redirected-target RECORD_SLEEP_PID_FILE=$SANDBOX/redirected-child.pid PATH=$SANDBOX/bin:$PATH bash -c '$WRAPPER record-sleep </dev/null; printf redirected-after\\n'"
  redirected_status=0
  {
    printf '%s\n' "$redirected_command"
    for _ in {1..300}; do
      [ -s "$SANDBOX/redirected-child.pid" ] && break
      sleep 0.01
    done
    test -s "$SANDBOX/redirected-child.pid"
    printf '\003'
    sleep 0.3
    printf 'exit\n'
  } | timeout 5 script -qec 'bash --noprofile --norc -i' /dev/null \
    >"$SANDBOX/redirected-pty-output" || redirected_status=$?
  test "$redirected_status" -eq 0 || test "$redirected_status" -eq 130
  grep -Fq '^C' "$SANDBOX/redirected-pty-output"
  redirected_child_pid="$(<"$SANDBOX/redirected-child.pid")"
  if kill -0 "$redirected_child_pid" 2>/dev/null; then
    exit 1
  fi

  pty_command="env HYPERCOLOR_CACHE_DIR=$SANDBOX/job-cache HYPERCOLOR_NO_SCCACHE=1 CARGO_TARGET_DIR=$SANDBOX/job-target READ_LOOP_READY=$SANDBOX/read-loop.ready PATH=$SANDBOX/bin:$PATH $WRAPPER read-loop"
  pty_status=0
  {
    printf '%s\n' "$pty_command"
    for _ in {1..300}; do
      [ -e "$SANDBOX/read-loop.ready" ] && break
      sleep 0.01
    done
    test -e "$SANDBOX/read-loop.ready"
    printf '\032'
    sleep 0.3
    printf 'jobs\n'
    sleep 0.2
    printf 'fg\n'
    sleep 0.2
    printf 'resumed\n'
    sleep 0.2
    printf '\003'
    sleep 0.2
    printf 'exit\n'
  } | timeout 5 script -qec 'bash --noprofile --norc -i' /dev/null \
    >"$SANDBOX/job-output" || pty_status=$?
  test "$pty_status" -eq 0 || test "$pty_status" -eq 130
  grep -Fq 'Stopped' "$SANDBOX/job-output"
  grep -Fq 'loop:resumed' "$SANDBOX/job-output"
fi

run_wrapper default
assert_args default build --target-dir "$SANDBOX_CHECKOUT/target" --workspace
test ! -s "$SANDBOX/default.target-env"

lock_target="$SANDBOX/lock-target"
mkdir -p "$lock_target"
FAKE_BUILD_LOCK_PATH="$lock_target/.cargo-build-lock" \
  FAKE_LOCK_DESCENDANT_PID_FILE="$SANDBOX/lock-descendant.pid" \
  FAKE_FORBIDDEN_FDS="3 4 5 6 7 8 9" \
  CARGO_TARGET_DIR="$lock_target" run_wrapper build-lock cargo build
test -e "$lock_target/.cargo-build-lock"
lock_descendant_pid="$(<"$SANDBOX/lock-descendant.pid")"
kill -0 "$lock_descendant_pid"
exec 9<>"$lock_target/.cargo-build-lock"
lock_released=0
flock -n 9 && lock_released=1
exec 9>&-
kill "$lock_descendant_pid" 2>/dev/null || true
test "$lock_released" -eq 1

handoff_target="$SANDBOX/handoff-target"
handoff_ready="$SANDBOX/handoff.ready"
mkdir -p "$handoff_target"
FAKE_CARGO_LOG="$SANDBOX/handoff" \
  HYPERCOLOR_CACHE_DIR="$SANDBOX/handoff-cache" \
  HYPERCOLOR_NO_SCCACHE=1 \
  HYPERCOLOR_LOCK_HANDOFF_READY="$handoff_ready" \
  HYPERCOLOR_LOCK_HANDOFF_DELAY_MS=300 \
  CARGO_TARGET_DIR="$handoff_target" \
  PATH="$SANDBOX/bin:$PATH" \
  "$WRAPPER" cargo build >/dev/null &
handoff_wrapper_pid=$!
for _ in {1..100}; do
  [ -e "$handoff_ready" ] && break
  sleep 0.01
done
for _ in {1..20}; do
  exec 9<>"$handoff_target/.cargo-build-lock"
  if flock -n 9; then
    exit 1
  fi
  exec 9>&-
  sleep 0.01
done
wait "$handoff_wrapper_pid"
handoff_wrapper_pid=""
exec 9<>"$handoff_target/.cargo-build-lock"
flock -n 9
exec 9>&-

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

# A shared cache that fails sccache's startup storage check leaves the build
# on the local disk cache instead of failing it.
SCCACHE_BUCKET=hypercolor-test-cache SCCACHE_S3_RW_MODE=READ_ONLY FAKE_SCCACHE_FAIL_REMOTE=1 \
  run_refresh_wrapper refresh-remote-down refresh-remote-down-cargo cargo build \
  2>"$SANDBOX/refresh-remote-down.stderr"
printf 'stop\nstart remote\nstart\n' >"$SANDBOX/refresh-remote-down.expected"
diff -u "$SANDBOX/refresh-remote-down.expected" "$SANDBOX/refresh-remote-down.sccache.log"
grep -q 'using the local disk cache' "$SANDBOX/refresh-remote-down.stderr"
test -s "$SANDBOX/refresh-remote-down/sccache-server-config"
# The recorded configuration keeps later builds in the job on the local
# server instead of retrying the failed remote before every compile.
SCCACHE_BUCKET=hypercolor-test-cache SCCACHE_S3_RW_MODE=READ_ONLY FAKE_SCCACHE_FAIL_REMOTE=1 \
  run_refresh_wrapper refresh-remote-down refresh-remote-down-cargo cargo build
diff -u "$SANDBOX/refresh-remote-down.expected" "$SANDBOX/refresh-remote-down.sccache.log"

set +e
SCCACHE_BUCKET=hypercolor-test-cache FAKE_SCCACHE_FAIL_REMOTE=1 FAKE_SCCACHE_FAIL_START=1 \
  run_refresh_wrapper refresh-all-down refresh-all-down-cargo cargo build \
  >"$SANDBOX/refresh-all-down.stdout" 2>"$SANDBOX/refresh-all-down.stderr"
refresh_all_down_status=$?
set -e
test "$refresh_all_down_status" -ne 0
test ! -e "$SANDBOX/refresh-all-down/sccache-server-config"

# Changing the shared cache mode restarts the server with the new backend.
SCCACHE_BUCKET=hypercolor-test-cache SCCACHE_S3_RW_MODE=READ_ONLY \
  run_refresh_wrapper refresh-remote-mode refresh-remote-mode-cargo cargo build
SCCACHE_BUCKET=hypercolor-test-cache SCCACHE_S3_RW_MODE=READ_ONLY \
  run_refresh_wrapper refresh-remote-mode refresh-remote-mode-cargo cargo build
SCCACHE_BUCKET=hypercolor-test-cache SCCACHE_S3_RW_MODE=READ_WRITE \
  run_refresh_wrapper refresh-remote-mode refresh-remote-mode-cargo cargo build
printf 'stop\nstart remote\nstop\nstart remote\n' >"$SANDBOX/refresh-remote-mode.expected"
diff -u "$SANDBOX/refresh-remote-mode.expected" "$SANDBOX/refresh-remote-mode.sccache.log"

ready="$SANDBOX/refresh-lock.ready"
HYPERCOLOR_FORCE_PORTABLE_LOCK=1 FAKE_SCCACHE_STOP_READY="$ready" \
  FAKE_SCCACHE_STOP_DELAY=0.4 \
  run_refresh_wrapper refresh-lock refresh-lock-first cargo build &
first_refresh_pid=$!
for _ in {1..50}; do
  [ -e "$ready" ] && break
  sleep 0.01
done
(
  HYPERCOLOR_FORCE_PORTABLE_LOCK=1 \
    run_refresh_wrapper refresh-lock refresh-lock-second cargo build
  : >"$SANDBOX/refresh-lock.second-done"
) &
second_refresh_pid=$!
sleep 0.1
test ! -e "$SANDBOX/refresh-lock.second-done"
wait "$first_refresh_pid"
wait "$second_refresh_pid"
test -e "$SANDBOX/refresh-lock.second-done"

signal_target="$SANDBOX/signal-target"
mkdir -p "$signal_target"
export SIGNAL_CHILD_PID_FILE="$SANDBOX/signal-child.pid"
export SIGNAL_GRANDCHILD_PID_FILE="$SANDBOX/signal-grandchild.pid"
# Expansion belongs to the child shell.
# shellcheck disable=SC2016
FAKE_CARGO_LOG="$SANDBOX/signal" \
  HYPERCOLOR_CACHE_DIR="$SANDBOX/signal-cache" \
  HYPERCOLOR_NO_SCCACHE=1 \
  CARGO_TARGET_DIR="$signal_target" \
  PATH="$SANDBOX/bin:$PATH" \
  "$WRAPPER" bash -c \
    'printf "%s\n" "$$" >"$SIGNAL_CHILD_PID_FILE"; sleep 30 & printf "%s\n" "$!" >"$SIGNAL_GRANDCHILD_PID_FILE"; wait' \
  >/dev/null &
signal_wrapper_pid=$!
for _ in {1..100}; do
  [ -s "$SIGNAL_CHILD_PID_FILE" ] \
    && [ -s "$SIGNAL_GRANDCHILD_PID_FILE" ] \
    && break
  sleep 0.01
done
signal_child_pid="$(<"$SIGNAL_CHILD_PID_FILE")"
signal_grandchild_pid="$(<"$SIGNAL_GRANDCHILD_PID_FILE")"
exec 9<>"$signal_target/.cargo-build-lock"
if flock -n 9; then
  exit 1
fi
exec 9>&-
kill -TERM "$signal_wrapper_pid"
set +e
wait "$signal_wrapper_pid"
signal_status=$?
set -e
signal_wrapper_pid=""
test "$signal_status" -eq 143
if kill -0 "$signal_child_pid" 2>/dev/null; then
  exit 1
fi
if kill -0 "$signal_grandchild_pid" 2>/dev/null; then
  exit 1
fi
signal_child_pid=""
signal_grandchild_pid=""
exec 9<>"$signal_target/.cargo-build-lock"
flock -n 9
exec 9>&-

kill_target="$SANDBOX/kill-target"
mkdir -p "$kill_target"
export SIGNAL_CHILD_PID_FILE="$SANDBOX/kill-child.pid"
# Expansion belongs to the child shell.
# shellcheck disable=SC2016
FAKE_CARGO_LOG="$SANDBOX/kill" \
  HYPERCOLOR_CACHE_DIR="$SANDBOX/kill-cache" \
  HYPERCOLOR_NO_SCCACHE=1 \
  CARGO_TARGET_DIR="$kill_target" \
  PATH="$SANDBOX/bin:$PATH" \
  "$WRAPPER" bash -c \
    'printf "%s\n" "$$" >"$SIGNAL_CHILD_PID_FILE"; exec sleep 30' \
  >/dev/null &
kill_wrapper_pid=$!
for _ in {1..100}; do
  [ -s "$SIGNAL_CHILD_PID_FILE" ] && break
  sleep 0.01
done
kill_child_pid="$(<"$SIGNAL_CHILD_PID_FILE")"
kill -KILL "$kill_wrapper_pid"
set +e
wait "$kill_wrapper_pid" 2>/dev/null
kill_status=$?
set -e
kill_wrapper_pid=""
test "$kill_status" -eq 137
exec 9<>"$kill_target/.cargo-build-lock"
flock -n 9
exec 9>&-
kill_lease="$(printf '%s\n' "$kill_target"/.cargo-build-pgid.*)"
kill_process_group="$(<"$kill_lease")"
test "$kill_process_group" = "$kill_child_pid"
kill -0 -- "-$kill_process_group"
kill -TERM "$kill_child_pid"
for _ in {1..100}; do
  if ! kill -0 -- "-$kill_process_group" 2>/dev/null; then
    break
  fi
  sleep 0.01
done
if kill -0 -- "-$kill_process_group" 2>/dev/null; then
  exit 1
fi
kill_child_pid=""

parallel_target="$SANDBOX/parallel-target"
mkdir -p "$parallel_target"
for lane in one two; do
  ready_file="$SANDBOX/parallel-$lane.ready"
  # Expansion belongs to the child shell.
  # shellcheck disable=SC2016
  FAKE_CARGO_LOG="$SANDBOX/parallel-$lane" \
    HYPERCOLOR_CACHE_DIR="$SANDBOX/parallel-cache" \
    HYPERCOLOR_NO_SCCACHE=1 \
    CARGO_TARGET_DIR="$parallel_target" \
    PATH="$SANDBOX/bin:$PATH" \
    "$WRAPPER" bash -c 'printf ready >"$1"; exec sleep 30' _ \
    "$ready_file" >/dev/null &
  case "$lane" in
    one) parallel_wrapper_one=$! ;;
    two) parallel_wrapper_two=$! ;;
  esac
done
for _ in {1..100}; do
  [ -e "$SANDBOX/parallel-one.ready" ] \
    && [ -e "$SANDBOX/parallel-two.ready" ] \
    && break
  sleep 0.01
done
parallel_lease_count="$(find "$parallel_target" -maxdepth 1 \
  -name '.cargo-build-pgid.*' -type f | wc -l)"
test "$parallel_lease_count" -eq 2
exec 9<>"$parallel_target/.cargo-build-lock"
if flock -n 9; then
  exit 1
fi
exec 9>&-
kill -TERM "$parallel_wrapper_one" "$parallel_wrapper_two"
set +e
wait "$parallel_wrapper_one"
parallel_one_status=$?
wait "$parallel_wrapper_two"
parallel_two_status=$?
set -e
parallel_wrapper_one=""
parallel_wrapper_two=""
test "$parallel_one_status" -eq 143
test "$parallel_two_status" -eq 143
test -z "$(find "$parallel_target" -maxdepth 1 \
  -name '.cargo-build-pgid.*' -type f -print -quit)"

failure_target="$SANDBOX/failure-target"
mkdir -p "$failure_target"
set +e
FAKE_CARGO_LOG="$SANDBOX/failure" \
  HYPERCOLOR_CACHE_DIR="$SANDBOX/failure-cache" \
  HYPERCOLOR_NO_SCCACHE=1 \
  CARGO_TARGET_DIR="$failure_target" \
  PATH="$SANDBOX/bin:$PATH" \
  "$WRAPPER" bash -c 'exit 1' >/dev/null
failure_status=$?
set -e
test "$failure_status" -eq 1
test -z "$(find "$failure_target" -maxdepth 1 \
  -name '.cargo-build-pgid.*' -type f -print -quit)"

failure_descendant_target="$SANDBOX/failure-descendant-target"
mkdir -p "$failure_descendant_target"
FAKE_CARGO_LOG="$SANDBOX/failure-descendant" \
  HYPERCOLOR_CACHE_DIR="$SANDBOX/failure-descendant-cache" \
  HYPERCOLOR_NO_SCCACHE=1 \
  CARGO_TARGET_DIR="$failure_descendant_target" \
  PATH="$SANDBOX/bin:$PATH" \
  "$WRAPPER" bash -c 'sleep 30 & exit 1' >/dev/null &
failure_descendant_wrapper=$!
set +e
wait "$failure_descendant_wrapper"
failure_descendant_status=$?
set -e
test "$failure_descendant_status" -eq 1
failure_descendant_lease="$(printf '%s\n' \
  "$failure_descendant_target"/.cargo-build-pgid.*)"
test -f "$failure_descendant_lease"
failure_descendant_group="$(<"$failure_descendant_lease")"
kill -0 -- "-$failure_descendant_group"
kill -TERM -- "-$failure_descendant_group"

# The daily incremental prune renames only idle direct children of a Cargo
# profile's incremental dir into a trash dir inside that profile, before
# Cargo starts. It never follows symlinks, never acts outside a canonical
# profile dir, re-checks each entry just before moving it, skips crate dirs
# with a live rustc session, and leaves every profile and incremental dir's
# mtime as it found it. The slow delete runs detached: the caller's stdout
# and any pipe it passed down close, and the target lock frees, while it
# still runs, and the target GC still counts the trash meanwhile.
prune_target="$SANDBOX/prune target"
prune_debug="$prune_target/debug"
prune_inc="$prune_debug/incremental"
prune_wasm_inc="$prune_target/wasm32-unknown-unknown/debug/incremental"
prune_idle_inc="$prune_target/release/incremental"
prune_outside="$SANDBOX/prune-outside"
old_stamp=202001010000
mkdir -p "$prune_inc/old crate/s-session" "$prune_inc/fresh-crate/s-old-session" \
  "$prune_inc/recent-crate" "$prune_inc/live-crate" "$prune_inc/racing-crate" \
  "$prune_wasm_inc/old-wasm" \
  "$prune_idle_inc/recent-release" \
  "$prune_target/unowned/incremental/old-unowned" \
  "$prune_outside/incremental/old-linked" \
  "$prune_debug/.hypercolor-prune.orphan/old-orphan" \
  "$prune_debug/.hypercolor-prune.recent/recent-orphan"
for profile in debug wasm32-unknown-unknown/debug release; do
  touch "$prune_target/$profile/.cargo-lock"
done
# Big enough to stand out in the GC's per-profile accounting.
head -c 4194304 /dev/zero >"$prune_inc/old crate/s-session/query-cache.bin"
touch "$prune_outside/.cargo-lock"
printf 'keep\n' >"$prune_outside/keep"
printf 'precious\n' >"$prune_outside/legacy-log"
ln -s "$prune_outside" "$prune_inc/old-link"
ln -s "$prune_outside" "$prune_target/linked"
ln -s "$prune_outside/legacy-log" "$prune_target/.hypercolor-incremental-prune.log"
: >"$prune_target/.hypercolor-incremental-prune"
# A session that opened after find's shortlist: the crate dir itself still
# looks idle, but its working session dir is fresh.
mkdir "$prune_inc/live-crate/s-hnb62h77g4-1jcu2p3-working"
touch -h -t "$old_stamp" "$prune_inc/old crate/s-session/query-cache.bin" \
  "$prune_inc/old crate/s-session" "$prune_inc/old crate" \
  "$prune_inc/fresh-crate/s-old-session" "$prune_inc/old-link" \
  "$prune_inc/live-crate" "$prune_inc/racing-crate" \
  "$prune_wasm_inc/old-wasm" "$prune_target/unowned/incremental/old-unowned" \
  "$prune_outside/incremental/old-linked" "$prune_target/linked" \
  "$prune_debug/.hypercolor-prune.orphan"
touch -d '2 days ago' "$prune_inc/recent-crate"
prune_dirs=("$prune_inc" "$prune_wasm_inc" "$prune_idle_inc" "$prune_debug" \
  "$prune_target/wasm32-unknown-unknown/debug" "$prune_target/release")
touch -t "$old_stamp" "${prune_dirs[@]}"
prune_mtimes() {
  stat -c '%n %y' "${prune_dirs[@]}"
}
prune_mtimes >"$SANDBOX/prune-mtimes.before"

mkdir -p "$SANDBOX/prune-bin"
# Holds the background delete until the test releases it.
cat >"$SANDBOX/prune-bin/rm" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  *.hypercolor-prune.*)
    : >"$SLOW_RM_STARTED"
    for _ in {1..1000}; do
      [ -e "$SLOW_RM_RELEASE" ] && break
      sleep 0.01
    done
    ;;
esac
exec "$REAL_RM" "$@"
EOF
# Changes entries after find has listed them, the way a rustc session
# opening, or a concurrent cargo clean, between the shortlist and the move
# would.
cat >"$SANDBOX/prune-bin/find" <<'EOF'
#!/usr/bin/env bash
if [ -n "${FAKE_FIND_DIR:-}" ] && [ "${1:-}" = "$FAKE_FIND_DIR" ]; then
  listing="$(mktemp)"
  "$REAL_FIND" "$@" >"$listing"
  status=$?
  [ -z "${FAKE_FIND_FRESHEN:-}" ] || touch "$FAKE_FIND_FRESHEN"
  [ -z "${FAKE_FIND_REMOVE:-}" ] || "$REAL_RM" -rf "$FAKE_FIND_REMOVE"
  cat "$listing"
  "$REAL_RM" -f "$listing"
  exit "$status"
fi
exec "$REAL_FIND" "$@"
EOF
# Copies a probe instead of renaming it, the way mv falls back when a
# rename would cross a mount.
# Can also remove an entry just before its batch renames it, or hold a
# batch rename until the test signals the wrapper.
cat >"$SANDBOX/prune-bin/mv" <<'EOF'
#!/usr/bin/env bash
if [ -n "${FAKE_MV_CROSS_MOUNT:-}" ] && [ "$#" -eq 3 ] && [ "$1" = -- ]; then
  case "$2" in
    */incremental/.hypercolor-prune.*)
      cp -R "$2" "$3" && exec "$REAL_RM" -rf "$2"
      ;;
  esac
fi
last="${!#}"
case "$last" in
  */.hypercolor-prune.*/)
    for arg in "$@"; do
      if [ -n "${FAKE_MV_DROP:-}" ] && [ "$arg" = "$FAKE_MV_DROP" ]; then
        "$REAL_RM" -rf "$FAKE_MV_DROP"
      fi
    done
    if [ -n "${FAKE_MV_HOLD_STARTED:-}" ]; then
      : >"$FAKE_MV_HOLD_STARTED"
      sleep 10
    fi
    ;;
esac
exec "$REAL_MV" "$@"
EOF
# An ls replacement such as eza, which prints no inode numbers.
cat >"$SANDBOX/prune-bin/ls" <<'EOF'
#!/usr/bin/env bash
if [ -n "${FAKE_LS_EZA:-}" ]; then
  printf 'drwx------ - user 17 Oct 12:00 %s\n' "${!#}"
  exit 0
fi
exec "$REAL_LS" "$@"
EOF
# BSD touch has no epoch dates; macOS hosts often pair it with GNU date.
cat >"$SANDBOX/prune-bin/touch" <<'EOF'
#!/usr/bin/env bash
if [ -n "${FAKE_TOUCH_BSD:-}" ]; then
  for arg in "$@"; do
    if [ "$arg" = -d ]; then
      echo "touch: illegal option -- d" >&2
      exit 1
    fi
  done
fi
exec "$REAL_TOUCH" "$@"
EOF
chmod +x "$SANDBOX/prune-bin/rm" "$SANDBOX/prune-bin/find" "$SANDBOX/prune-bin/mv" \
  "$SANDBOX/prune-bin/touch" "$SANDBOX/prune-bin/ls"
run_prune_wrapper() {
  env -u CI FAKE_CARGO_LOG="$SANDBOX/prune" \
    HYPERCOLOR_CACHE_DIR="$SANDBOX/prune-cache" HYPERCOLOR_NO_SCCACHE=1 \
    CARGO_TARGET_DIR="$prune_target" REAL_RM="$(command -v rm)" \
    REAL_FIND="$(command -v find)" REAL_MV="$(command -v mv)" \
    REAL_TOUCH="$(command -v touch)" REAL_LS="$(command -v ls)" \
    SLOW_RM_STARTED="$SANDBOX/slow-rm.started" \
    SLOW_RM_RELEASE="$SANDBOX/slow-rm.release" \
    PATH="$SANDBOX/prune-bin:$SANDBOX/bin:$PATH" \
    "$@" "$WRAPPER" cargo check
}
prune_claim() {
  printf '%s\n' "$1/.hypercolor-incremental-prune.$(date -u +%Y%m%d)"
}

# Builds under timezones 26 hours apart share one claim per UTC day.
tz_target="$SANDBOX/tz-target"
mkdir -p "$tz_target"
for zone in AAA-14 BBB+12; do
  env -u CI TZ="$zone" FAKE_CARGO_LOG="$SANDBOX/tz" \
    HYPERCOLOR_CACHE_DIR="$SANDBOX/prune-cache" HYPERCOLOR_NO_SCCACHE=1 \
    HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 CARGO_TARGET_DIR="$tz_target" \
    PATH="$SANDBOX/bin:$PATH" "$WRAPPER" cargo check >"$SANDBOX/tz-$zone.out" 2>&1
done
grep -Fq 'incremental prune: nothing idle for 3+ days' "$SANDBOX/tz-AAA-14.out"
grep -Fq 'incremental prune skipped: already ran today' "$SANDBOX/tz-BBB+12.out"
test -d "$(prune_claim "$tz_target")"

# A planted claim path is never followed, so its log cannot be truncated.
link_target="$SANDBOX/link-target"
mkdir -p "$link_target" "$SANDBOX/planted-claim"
printf 'precious\n' >"$SANDBOX/planted-claim/prune.log"
ln -s "$SANDBOX/planted-claim" "$(prune_claim "$link_target")"
prune_output="$(env -u CI FAKE_CARGO_LOG="$SANDBOX/link" \
  HYPERCOLOR_CACHE_DIR="$SANDBOX/prune-cache" HYPERCOLOR_NO_SCCACHE=1 \
  HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 CARGO_TARGET_DIR="$link_target" \
  PATH="$SANDBOX/bin:$PATH" "$WRAPPER" cargo check 2>&1)"
grep -Fq "incremental prune skipped: cannot claim today's run" <<<"$prune_output"
test "$(<"$SANDBOX/planted-claim/prune.log")" = precious

# A HOME reached through a symlink still protects the real home directory.
home_target="$SANDBOX/home-target"
mkdir -p "$home_target/debug/incremental/old-home"
touch "$home_target/debug/.cargo-lock"
touch -t "$old_stamp" "$home_target/debug/incremental/old-home"
ln -s "$home_target" "$SANDBOX/home-link"
prune_output="$(env -u CI HOME="$SANDBOX/home-link" \
  RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" \
  FAKE_CARGO_LOG="$SANDBOX/home" HYPERCOLOR_CACHE_DIR="$SANDBOX/prune-cache" \
  HYPERCOLOR_NO_SCCACHE=1 HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 \
  CARGO_TARGET_DIR="$home_target" PATH="$SANDBOX/bin:$PATH" \
  "$WRAPPER" cargo check 2>&1)"
grep -Fq "incremental prune skipped: refusing target directory '$(cd "$home_target" && pwd -P)'" \
  <<<"$prune_output"
test -d "$home_target/debug/incremental/old-home"

# A dir whose renames would cross a mount is left exactly as it was: no
# trash, no moves, no mtime changes, and a line that says so.
xdev_target="$SANDBOX/xdev-target"
xdev_inc="$xdev_target/debug/incremental"
mkdir -p "$xdev_inc/old-xdev"
touch "$xdev_target/debug/.cargo-lock"
touch -t "$old_stamp" "$xdev_inc/old-xdev" "$xdev_inc" "$xdev_target/debug"
stat -c '%n %y' "$xdev_inc" "$xdev_target/debug" >"$SANDBOX/xdev-mtimes.before"
prune_output="$(run_prune_wrapper FAKE_MV_CROSS_MOUNT=1 HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 \
  CARGO_TARGET_DIR="$xdev_target")"
grep -Fq 'incremental prune: moved nothing, skipped 1 dirs whose renames would cross a mount' \
  <<<"$prune_output"
test -z "$(grep -F 'deleting in the background' <<<"$prune_output")"
test -d "$xdev_inc/old-xdev"
test -z "$(find "$xdev_target" -name '.hypercolor-prune.*' -print -quit)"
stat -c '%n %y' "$xdev_inc" "$xdev_target/debug" >"$SANDBOX/xdev-mtimes.after"
diff -u "$SANDBOX/xdev-mtimes.before" "$SANDBOX/xdev-mtimes.after"

# An entry that disappears between the shortlist and its move, or between
# its re-check and its batch rename, is not counted as moved.
vanish_target="$SANDBOX/vanish-target"
vanish_inc="$vanish_target/debug/incremental"
mkdir -p "$vanish_inc/old-a" "$vanish_inc/vanishing-b" "$vanish_inc/dropped-c"
touch "$vanish_target/debug/.cargo-lock"
touch -t "$old_stamp" "$vanish_inc/old-a" "$vanish_inc/vanishing-b" "$vanish_inc/dropped-c"
prune_output="$(run_prune_wrapper HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 \
  CARGO_TARGET_DIR="$vanish_target" FAKE_FIND_DIR="$vanish_inc" \
  FAKE_FIND_REMOVE="$vanish_inc/vanishing-b" FAKE_MV_DROP="$vanish_inc/dropped-c" \
  SLOW_RM_RELEASE=/ \
  SLOW_RM_STARTED="$SANDBOX/vanish-rm.started")"
grep -Fq 'incremental prune: moved 1 entries idle for 3+ days to trash' <<<"$prune_output"

# The cutoff stamp works with BSD touch next to GNU date.
mixed_target="$SANDBOX/mixed-target"
mkdir -p "$mixed_target/debug/incremental/old-mixed"
touch "$mixed_target/debug/.cargo-lock"
touch -t "$old_stamp" "$mixed_target/debug/incremental/old-mixed"
prune_output="$(run_prune_wrapper HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 FAKE_TOUCH_BSD=1 \
  CARGO_TARGET_DIR="$mixed_target" SLOW_RM_RELEASE=/ \
  SLOW_RM_STARTED="$SANDBOX/mixed-rm.started" 2>&1)"
grep -Fq 'incremental prune: moved 1 entries idle for 3+ days to trash' <<<"$prune_output"

# The inode probe reads the system ls, so an ls replacement on PATH does
# not make every dir look cross-mount.
eza_target="$SANDBOX/eza-target"
mkdir -p "$eza_target/debug/incremental/old-eza"
touch "$eza_target/debug/.cargo-lock"
touch -t "$old_stamp" "$eza_target/debug/incremental/old-eza"
prune_output="$(run_prune_wrapper HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 FAKE_LS_EZA=1 \
  CARGO_TARGET_DIR="$eza_target" SLOW_RM_RELEASE=/ \
  SLOW_RM_STARTED="$SANDBOX/eza-rm.started")"
grep -Fq 'incremental prune: moved 1 entries idle for 3+ days to trash' <<<"$prune_output"

# A profile that cargo clean removes while its trash is being deleted stays
# gone: no stray file takes its place to break the next build.
gone_target="$SANDBOX/gone-target"
mkdir -p "$gone_target/debug/incremental/old-gone"
touch "$gone_target/debug/.cargo-lock"
touch -t "$old_stamp" "$gone_target/debug/incremental/old-gone" \
  "$gone_target/debug/incremental" "$gone_target/debug"
run_prune_wrapper HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 CARGO_TARGET_DIR="$gone_target" \
  SLOW_RM_STARTED="$SANDBOX/gone-rm.started" \
  SLOW_RM_RELEASE="$SANDBOX/gone-rm.release" >/dev/null
for _ in {1..300}; do
  [ -e "$SANDBOX/gone-rm.started" ] && break
  sleep 0.01
done
test -e "$SANDBOX/gone-rm.started"
rm -rf "$gone_target/debug"
: >"$SANDBOX/gone-rm.release"
for _ in {1..1000}; do
  grep -q '^finished' "$(prune_claim "$gone_target")/prune.log" 2>/dev/null && break
  sleep 0.01
done
grep -q '^finished' "$(prune_claim "$gone_target")/prune.log"
test ! -e "$gone_target/debug"
test ! -L "$gone_target/debug"
# The first thing the next build does in that profile.
mkdir "$gone_target/debug"

# A build that writes into a profile while its trash is being deleted keeps
# that activity: the profile is not handed back its old mtime.
busy_target="$SANDBOX/busy-target"
mkdir -p "$busy_target/debug/incremental/old-busy"
touch "$busy_target/debug/.cargo-lock"
touch -t "$old_stamp" "$busy_target/debug/incremental/old-busy" \
  "$busy_target/debug/incremental" "$busy_target/debug" "$SANDBOX/old-marker"
run_prune_wrapper HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 CARGO_TARGET_DIR="$busy_target" \
  SLOW_RM_STARTED="$SANDBOX/busy-rm.started" \
  SLOW_RM_RELEASE="$SANDBOX/busy-rm.release" >/dev/null
for _ in {1..300}; do
  [ -e "$SANDBOX/busy-rm.started" ] && break
  sleep 0.01
done
test -e "$SANDBOX/busy-rm.started"
test ! "$busy_target/debug" -nt "$SANDBOX/old-marker"
: >"$busy_target/debug/fresh-artifact"
: >"$SANDBOX/busy-rm.release"
for _ in {1..1000}; do
  grep -q '^finished' "$(prune_claim "$busy_target")/prune.log" 2>/dev/null && break
  sleep 0.01
done
grep -q '^finished' "$(prune_claim "$busy_target")/prune.log"
test "$busy_target/debug" -nt "$SANDBOX/old-marker"

# Ctrl-C at a terminal during the rename pass restores the mtimes, then
# stops the wrapper by the signal itself, so a shell loop around it stops
# too. A pty gives the loop a real foreground job and a real Ctrl-C.
if script --version 2>&1 | grep -Fq 'util-linux'; then
  loop_target="$SANDBOX/loop-target"
  mkdir -p "$loop_target/debug/incremental/old-loop"
  touch "$loop_target/debug/.cargo-lock"
  touch -t "$old_stamp" "$loop_target/debug/incremental/old-loop" \
    "$loop_target/debug/incremental" "$loop_target/debug"
  stat -c '%n %y' "$loop_target/debug/incremental" "$loop_target/debug" \
    >"$SANDBOX/loop-mtimes.before"
  loop_command="for i in 1 2; do env -u CI FAKE_CARGO_LOG=$SANDBOX/loop HYPERCOLOR_CACHE_DIR=$SANDBOX/prune-cache HYPERCOLOR_NO_SCCACHE=1 HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 CARGO_TARGET_DIR=$loop_target REAL_RM=$(command -v rm) REAL_MV=$(command -v mv) REAL_FIND=$(command -v find) REAL_TOUCH=$(command -v touch) REAL_LS=$(command -v ls) FAKE_MV_HOLD_STARTED=$SANDBOX/loop-mv.started PATH=$SANDBOX/prune-bin:$SANDBOX/bin:$PATH $WRAPPER cargo check >/dev/null 2>&1; printf '%s\\n' \$i >>$SANDBOX/loop.log; done"
  loop_status=0
  {
    printf '%s\n' "$loop_command"
    for _ in {1..500}; do
      [ -e "$SANDBOX/loop-mv.started" ] && break
      sleep 0.01
    done
    test -e "$SANDBOX/loop-mv.started"
    printf '\003'
    sleep 0.5
    printf 'exit\n'
  } | timeout 15 script -qec 'bash --noprofile --norc -i' /dev/null \
    >"$SANDBOX/loop-pty-output" || loop_status=$?
  test "$loop_status" -eq 0 || test "$loop_status" -eq 130
  test -e "$SANDBOX/loop-mv.started"
  test ! -s "$SANDBOX/loop.log"
  stat -c '%n %y' "$loop_target/debug/incremental" "$loop_target/debug" \
    >"$SANDBOX/loop-mtimes.after"
  diff -u "$SANDBOX/loop-mtimes.before" "$SANDBOX/loop-mtimes.after"
fi

# The traps come off after the rename pass: a TERM that lands later, while
# the wrapper waits on a slow command, stops it at once rather than after.
trap_target="$SANDBOX/trap-target"
mkdir -p "$trap_target"
env -u CI FAKE_CARGO_LOG="$SANDBOX/trap" FAKE_SCCACHE_LOG="$SANDBOX/trap.sccache.log" \
  FAKE_SCCACHE_VERSION_STARTED="$SANDBOX/trap-version.started" \
  FAKE_SCCACHE_VERSION_DELAY=4 HYPERCOLOR_CACHE_DIR="$SANDBOX/prune-cache" \
  HYPERCOLOR_FORCE_SCCACHE=1 HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 \
  CARGO_TARGET_DIR="$trap_target" PATH="$SANDBOX/bin:$PATH" \
  "$WRAPPER" cargo build >"$SANDBOX/trap.out" 2>&1 &
trap_wrapper_pid=$!
for _ in {1..500}; do
  [ -e "$SANDBOX/trap-version.started" ] && break
  sleep 0.01
done
test -e "$SANDBOX/trap-version.started"
trap_signal_at="$(date +%s%N)"
kill -TERM "$trap_wrapper_pid"
set +e
wait "$trap_wrapper_pid"
trap_status=$?
set -e
trap_wrapper_pid=""
test "$trap_status" -eq 143
test $(( ($(date +%s%N) - trap_signal_at) / 1000000 )) -lt 2000
grep -Fq 'incremental prune: nothing idle for 3+ days' "$SANDBOX/trap.out"

# fd 12 is the capture pipe too: the substitution only returns once every
# holder of it, background jobs included, has closed it.
prune_output="$(run_prune_wrapper HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 \
  FAKE_FIND_DIR="$prune_inc" FAKE_FIND_FRESHEN="$prune_inc/racing-crate" \
  FAKE_CARGO_EXPECT_GONE="$prune_inc/old crate" 12>&1)"
# The renames finish before Cargo starts, so this build's own compiles can
# never race them.
test "$(<"$SANDBOX/prune.gone")" = absent
grep -Fq 'incremental prune: moved 3 entries idle for 3+ days to trash, found 1 stale trash dirs; deleting in the background' <<<"$prune_output"
for _ in {1..300}; do
  [ -e "$SANDBOX/slow-rm.started" ] && break
  sleep 0.01
done
test -e "$SANDBOX/slow-rm.started"
# While the delete is held: the GC's per-profile accounting still sees the
# moved bytes, the trash sits inside its profile, every mtime is already
# back, and the target lock is free.
test -n "$(find "$prune_target" -maxdepth 2 -name '.hypercolor-prune.*' \
  ! -name '.hypercolor-prune.recent' ! -name '.hypercolor-prune.orphan' -print -quit)"
gc_total="$(HYPERCOLOR_CACHE_DIR="$SANDBOX/gc-cache" HYPERCOLOR_GC_REPOS="" \
  HYPERCOLOR_GC_TARGETS="$prune_target" "$ROOT_DIR/scripts/cargo-target-gc.sh" --dry-run \
  | sed -n 's/^\[cargo-gc\] \([^ ]*\) across .*/\1/p')"
gc_bytes="$(numfmt --from=iec-i --suffix=B "$gc_total")"
test "${gc_bytes%B}" -ge 4194304
test -n "$(find "$prune_debug" -maxdepth 1 -name '.hypercolor-prune.*' \
  ! -name '.hypercolor-prune.recent' ! -name '.hypercolor-prune.orphan' -print -quit)"
test -z "$(find "$prune_target" -maxdepth 1 -name '.hypercolor-prune.*' -print -quit)"
prune_mtimes >"$SANDBOX/prune-mtimes.after"
diff -u "$SANDBOX/prune-mtimes.before" "$SANDBOX/prune-mtimes.after"
exec 9<>"$prune_target/.cargo-build-lock"
flock -n 9
exec 9>&-
: >"$SANDBOX/slow-rm.release"
prune_log="$(prune_claim "$prune_target")/prune.log"
for _ in {1..1000}; do
  grep -q '^finished' "$prune_log" 2>/dev/null && break
  sleep 0.01
done
grep -q '^finished' "$prune_log"
for gone in "$prune_inc/old crate" "$prune_inc/old-link" "$prune_wasm_inc/old-wasm" \
  "$prune_debug/.hypercolor-prune.orphan" \
  "$prune_target/.hypercolor-incremental-prune" \
  "$prune_target/.hypercolor-incremental-prune.log"; do
  test ! -e "$gone"
  test ! -L "$gone"
done
for kept in "$prune_inc/fresh-crate/s-old-session" "$prune_inc/recent-crate" \
  "$prune_inc/live-crate" "$prune_inc/racing-crate" "$prune_idle_inc/recent-release" \
  "$prune_target/unowned/incremental/old-unowned" \
  "$prune_outside/incremental/old-linked" "$prune_target/linked" \
  "$prune_debug/.hypercolor-prune.recent/recent-orphan"; do
  test -e "$kept"
done
test "$(<"$prune_outside/keep")" = keep
test "$(<"$prune_outside/legacy-log")" = precious
test "$(find "$prune_target" -maxdepth 3 -name '.hypercolor-prune.*' | wc -l)" -eq 1
prune_mtimes >"$SANDBOX/prune-mtimes.after"
diff -u "$SANDBOX/prune-mtimes.before" "$SANDBOX/prune-mtimes.after"

prune_output="$(run_prune_wrapper HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3)"
grep -Fq 'incremental prune skipped: already ran today' <<<"$prune_output"
for disabled in off false 0 00; do
  prune_output="$(run_prune_wrapper HYPERCOLOR_INCREMENTAL_PRUNE_DAYS="$disabled")"
  grep -Fq 'incremental prune disabled' <<<"$prune_output"
done
prune_output="$(run_prune_wrapper CI=true HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3)"
grep -Fq 'incremental prune skipped in CI' <<<"$prune_output"
prune_output="$(run_prune_wrapper HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3d 2>&1)"
grep -Fq "must be a day count or off, not '3d'" <<<"$prune_output"
# The next day's claim replaces the previous one.
mv "$(prune_claim "$prune_target")" "$prune_target/.hypercolor-incremental-prune.20200101"
prune_output="$(run_prune_wrapper HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3)"
grep -Fq 'incremental prune: nothing idle for 3+ days' <<<"$prune_output"
test -z "$(grep -F 'deleting in the background' <<<"$prune_output")"
test ! -e "$prune_target/.hypercolor-incremental-prune.20200101"
test -d "$(prune_claim "$prune_target")"
prune_mtimes >"$SANDBOX/prune-mtimes.after"
diff -u "$SANDBOX/prune-mtimes.before" "$SANDBOX/prune-mtimes.after"

# Racing builds: exactly one claims the day.
claim_target="$SANDBOX/claim-target"
mkdir -p "$claim_target"
claim_pids=()
for lane in {1..16}; do
  env -u CI FAKE_CARGO_LOG="$SANDBOX/claim-$lane" \
    HYPERCOLOR_CACHE_DIR="$SANDBOX/prune-cache" HYPERCOLOR_NO_SCCACHE=1 \
    HYPERCOLOR_INCREMENTAL_PRUNE_DAYS=3 CARGO_TARGET_DIR="$claim_target" \
    PATH="$SANDBOX/bin:$PATH" "$WRAPPER" cargo check >"$SANDBOX/claim-$lane.out" 2>&1 &
  claim_pids+=("$!")
done
for pid in "${claim_pids[@]}"; do
  wait "$pid"
done
test "$(cat "$SANDBOX"/claim-*.out | grep -c 'incremental prune: nothing idle')" -eq 1
test "$(cat "$SANDBOX"/claim-*.out | grep -c 'incremental prune skipped: already ran today')" -eq 15

printf 'cargo-cache-build tests: PASS\n'
