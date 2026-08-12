#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
GC="$ROOT_DIR/scripts/cargo-target-gc.sh"
SANDBOX="$(mktemp -d)"
trap 'find "$SANDBOX" -depth -delete 2>/dev/null || true' EXIT
mkdir -p "$SANDBOX/bin"
cat >"$SANDBOX/bin/cargo-sweep" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$FAKE_CARGO_SWEEP_LOG"
EOF
chmod +x "$SANDBOX/bin/cargo-sweep"

run_gc() {
  HYPERCOLOR_CACHE_DIR="$SANDBOX/cache" \
    HYPERCOLOR_GC_REPOS="${HYPERCOLOR_GC_REPOS:-}" \
    HYPERCOLOR_GC_TARGETS="${HYPERCOLOR_GC_TARGETS:-}" \
    HYPERCOLOR_GC_HIGH_WATER_BYTES="${HYPERCOLOR_GC_HIGH_WATER_BYTES:-1}" \
    HYPERCOLOR_GC_LOW_WATER_BYTES="${HYPERCOLOR_GC_LOW_WATER_BYTES:-0}" \
    HYPERCOLOR_GC_MIN_AGE_DAYS="${HYPERCOLOR_GC_MIN_AGE_DAYS:-14}" \
    HYPERCOLOR_GC_RECLAIM_MIN_AGE_SECONDS="${HYPERCOLOR_GC_RECLAIM_MIN_AGE_SECONDS:-900}" \
    FAKE_CARGO_SWEEP_LOG="$SANDBOX/cargo-sweep.log" \
    PATH="$SANDBOX/bin:$PATH" \
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
grep -Fq "would clear" "$SANDBOX/dry-run.log"
test -d "$old_profile"

old_lock_inode="$(stat -c %i "$old_profile/.cargo-lock")"
HYPERCOLOR_GC_TARGETS="$target" run_gc --apply >"$SANDBOX/apply.log"
test -d "$old_profile"
test "$(stat -c %i "$old_profile/.cargo-lock")" = "$old_lock_inode"
test ! -e "$old_profile/deps/artifact"
test -d "$recent_profile"

future_target="$SANDBOX/future/target"
future_profile="$future_target/debug"
mkdir -p "$future_profile/deps"
: >"$future_profile/.cargo-artifact-lock"
truncate -s 4096 "$future_profile/deps/artifact"
touch -d '30 days ago' "$future_profile" "$future_profile/.cargo-artifact-lock" \
  "$future_profile/deps"
HYPERCOLOR_GC_TARGETS="$future_target" run_gc --apply >"$SANDBOX/future.log"
test -e "$future_profile/.cargo-artifact-lock"
test ! -e "$future_profile/deps/artifact"

grace_target="$SANDBOX/grace/target"
grace_profile="$grace_target/debug"
make_profile "$grace_profile" '5 minutes ago'
HYPERCOLOR_GC_TARGETS="$grace_target" run_gc --reclaim-now >"$SANDBOX/grace.log"
test -e "$grace_profile/deps/artifact"

bounded_target="$SANDBOX/bounded/target"
small_profile="$bounded_target/a-small"
large_profile="$bounded_target/b-large"
make_profile "$small_profile" '40 days ago'
make_profile "$large_profile" '30 days ago'
truncate -s 1048576 "$large_profile/deps/artifact"
small_size="$(du -sb "$small_profile" | awk '{print $1}')"
large_size="$(du -sb "$large_profile" | awk '{print $1}')"
bounded_total=$((small_size + large_size))
bounded_high=$((bounded_total - small_size))
bounded_low=$((bounded_high - 1))
HYPERCOLOR_GC_TARGETS="$bounded_target" \
  HYPERCOLOR_GC_HIGH_WATER_BYTES="$bounded_high" \
  HYPERCOLOR_GC_LOW_WATER_BYTES="$bounded_low" \
  run_gc --apply >"$SANDBOX/bounded.log"
test -d "$small_profile"
test -e "$small_profile/.cargo-lock"
test ! -e "$small_profile/deps/artifact"
test -d "$large_profile"
grep -Fq "keeping oversized profile" "$SANDBOX/bounded.log"
grep -Fq "pressure cleared" "$SANDBOX/bounded.log"

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

reclaim_repo="$SANDBOX/reclaim-repo"
mkdir -p "$reclaim_repo"
git -C "$reclaim_repo" init -q
printf 'target/\n' >"$reclaim_repo/.gitignore"
printf '[package]\nname = "gc-probe"\nversion = "0.1.0"\n' >"$reclaim_repo/Cargo.toml"
printf 'fn main() {}\n' >"$reclaim_repo/main.rs"
git -C "$reclaim_repo" add .gitignore Cargo.toml main.rs
old_commit_date="$(date -d '30 days ago' --iso-8601=seconds)"
GIT_AUTHOR_DATE="$old_commit_date" GIT_COMMITTER_DATE="$old_commit_date" \
  git -C "$reclaim_repo" -c user.name=GC -c user.email=gc@example.invalid commit -qm init
reclaim_profile="$reclaim_repo/target/debug"
make_profile "$reclaim_profile" '30 days ago'
HYPERCOLOR_GC_REPOS="$reclaim_repo" run_gc --reclaim-now >"$SANDBOX/reclaim.log"
grep -Eq "sweep --maxsize [0-9]+MB $reclaim_repo" "$SANDBOX/cargo-sweep.log"
test -e "$reclaim_profile/.cargo-lock"
test ! -e "$reclaim_profile/deps/artifact"

printf 'cargo-target-gc tests: PASS\n'
