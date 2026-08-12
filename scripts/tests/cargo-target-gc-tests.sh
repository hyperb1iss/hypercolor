#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
GC="$ROOT_DIR/scripts/cargo-target-gc.sh"
SANDBOX="$(mktemp -d)"
trap 'find "$SANDBOX" -depth -delete 2>/dev/null || true' EXIT

run_gc() {
  HYPERCOLOR_CACHE_DIR="$SANDBOX/cache" \
    HYPERCOLOR_GC_REPOS="${HYPERCOLOR_GC_REPOS:-}" \
    HYPERCOLOR_GC_TARGETS="${HYPERCOLOR_GC_TARGETS:-}" \
    HYPERCOLOR_GC_HIGH_WATER_BYTES=1 \
    HYPERCOLOR_GC_LOW_WATER_BYTES=0 \
    HYPERCOLOR_GC_MIN_AGE_DAYS="${HYPERCOLOR_GC_MIN_AGE_DAYS:-14}" \
    "$GC" "$@"
}

make_profile() {
  local profile="$1"
  local age="$2"
  mkdir -p "$profile/deps"
  : >"$profile/.cargo-lock"
  truncate -s 4096 "$profile/deps/artifact"
  touch -d "$age" "$profile" "$profile/.cargo-lock" "$profile/deps"
}

target="$SANDBOX/targets/target"
old_profile="$target/debug"
recent_profile="$target/release"
make_profile "$old_profile" '30 days ago'
make_profile "$recent_profile" '1 day ago'
HYPERCOLOR_GC_TARGETS="$target" run_gc --dry-run >"$SANDBOX/dry-run.log"
grep -Fq "would remove" "$SANDBOX/dry-run.log"
test -d "$old_profile"

HYPERCOLOR_GC_TARGETS="$target" run_gc --apply >"$SANDBOX/apply.log"
test ! -e "$old_profile"
test -d "$recent_profile"

locked_target="$SANDBOX/locked/target"
locked_profile="$locked_target/debug"
make_profile "$locked_profile" '30 days ago'
ready="$SANDBOX/locked.ready"
flock "$locked_profile/.cargo-lock" bash -c "touch \"\$1\"; sleep 5" _ "$ready" &
locker_pid=$!
for _ in {1..50}; do
  [ -e "$ready" ] && break
  sleep 0.02
done
HYPERCOLOR_GC_TARGETS="$locked_target" run_gc --apply >"$SANDBOX/locked.log"
grep -Fq "keeping locked profile" "$SANDBOX/locked.log"
test -d "$locked_profile"
kill "$locker_pid"
wait "$locker_pid" 2>/dev/null || true

repo="$SANDBOX/repo"
mkdir -p "$repo"
git -C "$repo" init -q
printf 'target/\n' >"$repo/.gitignore"
printf 'clean\n' >"$repo/source.txt"
git -C "$repo" add .gitignore source.txt
git -C "$repo" -c user.name=GC -c user.email=gc@example.invalid commit -qm init
dirty_profile="$repo/target/debug"
make_profile "$dirty_profile" '30 days ago'
printf 'dirty\n' >>"$repo/source.txt"
HYPERCOLOR_GC_REPOS="$repo" HYPERCOLOR_GC_MIN_AGE_DAYS=0 run_gc --apply \
  >"$SANDBOX/dirty.log"
grep -Fq "keeping dirty worktree profile" "$SANDBOX/dirty.log"
test -d "$dirty_profile"

printf 'cargo-target-gc tests: PASS\n'
