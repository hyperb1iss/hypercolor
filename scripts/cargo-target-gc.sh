#!/usr/bin/env bash
set -euo pipefail

MODE=dry-run
RECLAIM_NOW=0
case "${1:-}" in
  "" | --dry-run) ;;
  --apply) MODE=apply ;;
  --reclaim-now)
    MODE=apply
    RECLAIM_NOW=1
    ;;
  -h | --help)
    printf '%s\n' \
      'Usage: cargo-target-gc.sh [--dry-run|--apply|--reclaim-now]' \
      '' \
      'Prunes inactive Hypercolor Cargo profiles only when aggregate target' \
      'usage exceeds the high-water mark. Dry-run is the default.'
    exit 0
    ;;
  *)
    echo "unknown argument: $1" >&2
    exit 2
    ;;
esac

CACHE_ROOT="${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}"
HIGH_WATER="${HYPERCOLOR_GC_HIGH_WATER_BYTES:-$((300 * 1024 * 1024 * 1024))}"
LOW_WATER="${HYPERCOLOR_GC_LOW_WATER_BYTES:-$((240 * 1024 * 1024 * 1024))}"
MIN_AGE_DAYS="${HYPERCOLOR_GC_MIN_AGE_DAYS:-14}"
[ "$RECLAIM_NOW" -eq 0 ] || MIN_AGE_DAYS=0

for value in "$HIGH_WATER" "$LOW_WATER" "$MIN_AGE_DAYS"; do
  [[ "$value" =~ ^[0-9]+$ ]] || {
    echo "GC thresholds must be non-negative integers" >&2
    exit 2
  }
done
if [ "$LOW_WATER" -ge "$HIGH_WATER" ]; then
  echo "low-water bytes must be smaller than high-water bytes" >&2
  exit 2
fi

mkdir -p "$CACHE_ROOT"
exec 8>"$CACHE_ROOT/target-gc.lock"
if ! flock -n 8; then
  echo "[cargo-gc] another collection is already running"
  exit 0
fi

TARGET_ROOTS=()
TARGET_OWNERS=()
TARGET_DIRTY=()

add_target_root() {
  local path="$1"
  local owner="$2"
  local dirty="$3"
  [ -d "$path" ] || return 0

  local canonical existing
  canonical="$(cd "$path" && pwd -P)"
  case "$canonical" in
    / | /home | /tmp | /usr | /var | "$HOME" | "$HOME/dev")
      echo "[cargo-gc] refusing broad target root: $canonical" >&2
      return 0
      ;;
  esac
  for existing in "${!TARGET_ROOTS[@]}"; do
    if [ "${TARGET_ROOTS[existing]}" = "$canonical" ]; then
      if [ "$dirty" -eq 1 ]; then
        TARGET_DIRTY[existing]=1
      fi
      return 0
    fi
  done
  TARGET_ROOTS+=("$canonical")
  TARGET_OWNERS+=("$owner")
  TARGET_DIRTY+=("$dirty")
}

register_worktree() {
  local worktree="$1"
  local dirty=0
  [ -d "$worktree" ] || return 0
  if [ -n "$(git -C "$worktree" status --porcelain=v1 --untracked-files=normal 2>/dev/null || true)" ]; then
    dirty=1
  fi
  add_target_root "$worktree/target" "$worktree" "$dirty"
  add_target_root "$worktree/crates/hypercolor-ui/target" "$worktree" "$dirty"
  add_target_root "$worktree/crates/hypercolor-internal-ui/target" "$worktree" "$dirty"
  if [ -d "$worktree/oss" ] && [ ! -L "$worktree/oss" ]; then
    add_target_root "$worktree/oss/crates/hypercolor-ui/target" "$worktree" "$dirty"
  fi
}

if [ "${HYPERCOLOR_GC_REPOS+x}" = x ]; then
  repo_list="$HYPERCOLOR_GC_REPOS"
else
  repo_list="$HOME/dev/hypercolor:$HOME/dev/hypercolor.lighting"
fi
while IFS= read -r repo; do
  [ -e "$repo/.git" ] || continue
  while IFS= read -r worktree; do
    register_worktree "$worktree"
  done < <(git -C "$repo" worktree list --porcelain | sed -n 's/^worktree //p')
done < <(printf '%s\n' "$repo_list" | tr ':' '\n')

if [ "${HYPERCOLOR_GC_TARGETS+x}" = x ]; then
  while IFS= read -r target; do
    [ -z "$target" ] || add_target_root "$target" "" 0
  done < <(printf '%s\n' "$HYPERCOLOR_GC_TARGETS" | tr ':' '\n')
fi
add_target_root "$CACHE_ROOT/target" "" 0

PROFILE_PATHS=()
PROFILE_ROOTS=()
PROFILE_OWNERS=()
PROFILE_DIRTY=()

add_profile() {
  local profile="$1"
  local root="$2"
  local owner="$3"
  local dirty="$4"
  local canonical existing
  canonical="$(cd "$profile" && pwd -P)"
  case "$canonical" in
    "$root"/*) ;;
    *)
      echo "[cargo-gc] refusing profile outside target root: $canonical" >&2
      return 0
      ;;
  esac
  [ -f "$canonical/.cargo-lock" ] || return 0
  for existing in "${!PROFILE_PATHS[@]}"; do
    if [ "${PROFILE_PATHS[existing]}" = "$canonical" ]; then
      if [ "$dirty" -eq 1 ]; then
        PROFILE_DIRTY[existing]=1
      fi
      return 0
    fi
  done
  PROFILE_PATHS+=("$canonical")
  PROFILE_ROOTS+=("$root")
  PROFILE_OWNERS+=("$owner")
  PROFILE_DIRTY+=("$dirty")
}

for index in "${!TARGET_ROOTS[@]}"; do
  root="${TARGET_ROOTS[index]}"
  while IFS= read -r -d '' lock_file; do
    add_profile "${lock_file%/.cargo-lock}" "$root" \
      "${TARGET_OWNERS[index]}" "${TARGET_DIRTY[index]}"
  done < <(find "$root" -mindepth 2 -maxdepth 3 -type f -name .cargo-lock -print0)
done

profile_activity() {
  local profile="$1"
  local owner="$2"
  local latest=0
  local timestamp
  while IFS= read -r timestamp; do
    [ "$timestamp" -le "$latest" ] || latest="$timestamp"
  done < <(stat -c '%Y' "$profile" "$profile/.cargo-lock" \
    "$profile/deps" "$profile/build" "$profile/incremental" 2>/dev/null || true)
  if [ -n "$owner" ]; then
    timestamp="$(git -C "$owner" show -s --format=%ct HEAD 2>/dev/null || echo 0)"
    [ "$timestamp" -le "$latest" ] || latest="$timestamp"
  fi
  printf '%s\n' "$latest"
}

human_bytes() {
  numfmt --to=iec-i --suffix=B "$1"
}

scan_profiles() {
  CANDIDATE_ROWS=()
  TOTAL_BYTES=0
  local index profile size activity
  for index in "${!PROFILE_PATHS[@]}"; do
    profile="${PROFILE_PATHS[index]}"
    [ -d "$profile" ] || continue
    size="$(du -sb "$profile" 2>/dev/null | awk 'END {print $1}' || true)"
    if ! [[ "$size" =~ ^[0-9]+$ ]]; then
      echo "[cargo-gc] target inventory changed during measurement; deferring collection: $profile" >&2
      exit 0
    fi
    activity="$(profile_activity "$profile" "${PROFILE_OWNERS[index]}")"
    TOTAL_BYTES=$((TOTAL_BYTES + size))
    CANDIDATE_ROWS+=("$activity"$'\t'"$size"$'\t'"$index")
  done
}

scan_profiles

printf '[cargo-gc] %s across %d Cargo profiles, high %s, low %s\n' \
  "$(human_bytes "$TOTAL_BYTES")" "${#PROFILE_PATHS[@]}" \
  "$(human_bytes "$HIGH_WATER")" "$(human_bytes "$LOW_WATER")"
if [ "$TOTAL_BYTES" -le "$HIGH_WATER" ]; then
  echo "[cargo-gc] below the high-water mark; nothing to prune"
  exit 0
fi

TARGET_LOCK_FDS=()

release_target_locks() {
  local fd
  for fd in "${TARGET_LOCK_FDS[@]}"; do
    eval "exec ${fd}>&-"
  done
  TARGET_LOCK_FDS=()
}

acquire_target_locks() {
  local root="$1"
  local index profile sentinel fd
  TARGET_LOCK_FDS=()
  for index in "${!PROFILE_PATHS[@]}"; do
    [ "${PROFILE_ROOTS[index]}" = "$root" ] || continue
    profile="${PROFILE_PATHS[index]}"
    for sentinel in \
      "$profile/.cargo-lock" \
      "$profile/.cargo-build-lock" \
      "$profile/.cargo-artifact-lock"; do
      [ -e "$sentinel" ] || continue
      exec {fd}<>"$sentinel"
      if ! flock -n "$fd"; then
        eval "exec ${fd}>&-"
        release_target_locks
        return 1
      fi
      TARGET_LOCK_FDS+=("$fd")
    done
  done
}

CARGO_SWEEP_BIN="$(command -v cargo-sweep || true)"
if [ -z "$CARGO_SWEEP_BIN" ] && [ -x "$HOME/.cargo/bin/cargo-sweep" ]; then
  CARGO_SWEEP_BIN="$HOME/.cargo/bin/cargo-sweep"
fi
if [ -n "$CARGO_SWEEP_BIN" ]; then
  for index in "${!TARGET_ROOTS[@]}"; do
    root="${TARGET_ROOTS[index]}"
    owner="${TARGET_OWNERS[index]}"
    [ "${TARGET_DIRTY[index]}" -eq 0 ] || continue
    [ "$(basename "$root")" = target ] || continue
    [ -n "$owner" ] && [ -f "$owner/Cargo.toml" ] || continue
    if [ -n "$owner" ] \
      && [ -n "$(git -C "$owner" status --porcelain=v1 --untracked-files=normal 2>/dev/null || true)" ]; then
      continue
    fi
    if ! acquire_target_locks "$root"; then
      echo "[cargo-gc] keeping target with a locked profile: $root"
      continue
    fi
    if [ "$RECLAIM_NOW" -eq 1 ]; then
      scan_profiles
      needed=$((TOTAL_BYTES - LOW_WATER))
      if [ "$needed" -le 0 ]; then
        release_target_locks
        break
      fi
      target_bytes="$(du -sb "$root" 2>/dev/null | awk 'END {print $1}' || true)"
      if ! [[ "$target_bytes" =~ ^[0-9]+$ ]]; then
        echo "[cargo-gc] target changed during sizing; preserving it: $root"
        release_target_locks
        continue
      fi
      target_cap=$((target_bytes - needed))
      [ "$target_cap" -ge 1048576 ] || target_cap=1048576
      target_cap_mib=$(( (target_cap + 1048575) / 1048576 ))
      sweep_args=(--maxsize "${target_cap_mib}MB")
    else
      sweep_args=(--time "$MIN_AGE_DAYS")
      [ "$MODE" = apply ] || sweep_args=(--dry-run "${sweep_args[@]}")
    fi
    "$CARGO_SWEEP_BIN" sweep "${sweep_args[@]}" "$(dirname "$root")"
    release_target_locks
  done
  if [ "$MODE" = apply ]; then
    scan_profiles
    printf '[cargo-gc] post-sweep usage: %s\n' "$(human_bytes "$TOTAL_BYTES")"
    if [ "$TOTAL_BYTES" -le "$LOW_WATER" ]; then
      echo "[cargo-gc] artifact sweep reached the low-water mark; whole profiles were preserved"
      exit 0
    fi
  fi
else
  echo "[cargo-gc] cargo-sweep unavailable; active profiles receive no artifact-level pruning"
fi

release_profile_locks() {
  local fd
  for fd in "${PROFILE_LOCK_FDS[@]}"; do
    eval "exec ${fd}>&-"
  done
  PROFILE_LOCK_FDS=()
}

acquire_profile_locks() {
  local profile="$1"
  local root="$2"
  local sentinel fd
  PROFILE_LOCK_FDS=()
  for sentinel in \
    "$profile/.cargo-lock" \
    "$profile/.cargo-build-lock" \
    "$profile/.cargo-artifact-lock" \
    "$root/.cargo-build-lock" \
    "$root/.cargo-artifact-lock"; do
    [ -e "$sentinel" ] || continue
    exec {fd}<>"$sentinel"
    if ! flock -n "$fd"; then
      eval "exec ${fd}>&-"
      release_profile_locks
      return 1
    fi
    PROFILE_LOCK_FDS+=("$fd")
  done
}

cutoff=$(( $(date +%s) - MIN_AGE_DAYS * 86400 ))
remaining="$TOTAL_BYTES"
removed=0
while IFS=$'\t' read -r activity size index; do
  [ -n "$index" ] || continue
  [ "$remaining" -gt "$LOW_WATER" ] || break
  profile="${PROFILE_PATHS[index]}"
  root="${PROFILE_ROOTS[index]}"
  owner="${PROFILE_OWNERS[index]}"
  needed=$((remaining - LOW_WATER))

  if [ "${PROFILE_DIRTY[index]}" -eq 1 ]; then
    echo "[cargo-gc] keeping dirty worktree profile: $profile"
    continue
  fi
  if [ "$activity" -gt "$cutoff" ]; then
    continue
  fi
  if [ "$remaining" -le "$HIGH_WATER" ] && [ "$size" -gt "$needed" ]; then
    printf '[cargo-gc] keeping oversized profile %s above the low-water target: %s\n' \
      "$(human_bytes "$size")" "$profile"
    continue
  fi
  if [ -n "$owner" ] \
    && [ -n "$(git -C "$owner" status --porcelain=v1 --untracked-files=normal 2>/dev/null || true)" ]; then
    echo "[cargo-gc] keeping newly dirty worktree profile: $profile"
    continue
  fi
  if ! acquire_profile_locks "$profile" "$root"; then
    echo "[cargo-gc] keeping locked profile: $profile"
    continue
  fi
  activity="$(profile_activity "$profile" "$owner")"
  if [ "$activity" -gt "$cutoff" ]; then
    release_profile_locks
    continue
  fi

  if [ "$MODE" = dry-run ]; then
    printf '[cargo-gc] would remove %s: %s\n' "$(human_bytes "$size")" "$profile"
  else
    printf '[cargo-gc] removing %s: %s\n' "$(human_bytes "$size")" "$profile"
    find "$profile" -xdev -depth -delete
  fi
  release_profile_locks
  remaining=$((remaining - size))
  removed=$((removed + size))
done < <(printf '%s\n' "${CANDIDATE_ROWS[@]:-}" | sort -n -k1,1)

if [ "$remaining" -le "$LOW_WATER" ]; then
  verb=eligible
  [ "$MODE" = dry-run ] || verb=removed
  printf '[cargo-gc] %s %s, projected usage %s\n' \
    "$verb" "$(human_bytes "$removed")" "$(human_bytes "$remaining")"
elif [ "$remaining" -le "$HIGH_WATER" ]; then
  printf '[cargo-gc] pressure cleared at %s; oversized, active, locked, dirty, or recent profiles were preserved\n' \
    "$(human_bytes "$remaining")"
else
  printf '[cargo-gc] pressure remains at %s; active, locked, dirty, or recent profiles were preserved\n' \
    "$(human_bytes "$remaining")"
fi
