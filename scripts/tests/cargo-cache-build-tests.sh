#!/usr/bin/env bash
set -euo pipefail

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
cleanup() {
  for pid in "$signal_wrapper_pid" "$signal_child_pid" \
    "$signal_grandchild_pid" "$kill_wrapper_pid" "$kill_child_pid" \
    "$parallel_wrapper_one" "$parallel_wrapper_two" \
    "$handoff_wrapper_pid"; do
    [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
  done
  rm -rf "$SANDBOX"
}
trap 'cleanup' EXIT
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
  exec 10<>"$FAKE_BUILD_LOCK_PATH"
  if flock -n 10; then
    echo "target build lock was not held" >&2
    exit 9
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

  pty_command="env HYPERCOLOR_CACHE_DIR=$SANDBOX/job-cache HYPERCOLOR_NO_SCCACHE=1 CARGO_TARGET_DIR=$SANDBOX/job-target PATH=$SANDBOX/bin:$PATH $WRAPPER read-loop"
  pty_status=0
  {
    printf '%s\n' "$pty_command"
    sleep 0.3
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
assert_args default build --target-dir "$ROOT_DIR/target" --workspace
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

printf 'cargo-cache-build tests: PASS\n'
