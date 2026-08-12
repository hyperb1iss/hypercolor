#!/usr/bin/env bash
set -euo pipefail

# Shared Cargo build wrapper.
# Keeps Rust/C/C++ artifacts in stable cache locations and opportunistically
# enables compiler caches so whole-workspace builds warm up instead of starting
# from scratch on every clean target dir.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CALLER_DIR="$PWD"

# Default the bare invocation before command detection so it routes like an
# explicit `cargo build --workspace` instead of falling through to
# incremental mode.
if [ "$#" -eq 0 ]; then
  set -- cargo build --workspace
fi

CARGO_SUBCOMMAND=""
CARGO_SUBCOMMAND_INDEX=0
if [ "$#" -gt 1 ]; then
  case "$(basename "$1")" in
    cargo | cargo.exe)
      i=2
      while [ "$i" -le "$#" ]; do
        arg="${!i}"
        case "$arg" in
          --color | --config | -Z)
            i=$((i + 2))
            continue
            ;;
          -* | +*)
            i=$((i + 1))
            continue
            ;;
          *)
            CARGO_SUBCOMMAND="$arg"
            CARGO_SUBCOMMAND_INDEX="$i"
            break
            ;;
        esac
      done
      ;;
  esac
fi

TAURI_SUBCOMMAND=""
if [ "$CARGO_SUBCOMMAND" = "tauri" ]; then
  tauri_subcommand_index=$((CARGO_SUBCOMMAND_INDEX + 1))
  if [ "$tauri_subcommand_index" -le "$#" ]; then
    TAURI_SUBCOMMAND="${!tauri_subcommand_index}"
  fi
fi

TOP_LEVEL_COMMAND="$(basename "$1")"

TARGET_DIR_ARG=""
TARGET_DIR_IS_EXPLICIT=0
for ((i = 1; i <= $#; i++)); do
  arg="${!i}"
  case "$arg" in
    --)
      if [ "$CARGO_SUBCOMMAND" != "tauri" ] || [ "$TAURI_SUBCOMMAND" != "build" ]; then
        break
      fi
      ;;
    --target-dir)
      next_index=$((i + 1))
      if [ "$next_index" -gt "$#" ]; then
        echo "[cargo-cache] --target-dir requires a path" >&2
        exit 2
      fi
      TARGET_DIR_ARG="${!next_index}"
      TARGET_DIR_IS_EXPLICIT=1
      break
      ;;
    --target-dir=*)
      TARGET_DIR_ARG="${arg#--target-dir=}"
      TARGET_DIR_IS_EXPLICIT=1
      break
      ;;
  esac
done

if [ "$TARGET_DIR_IS_EXPLICIT" -eq 1 ]; then
  TARGET_DIR="$TARGET_DIR_ARG"
else
  TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
fi
mkdir -p "$TARGET_DIR"
TARGET_DIR="$(cd "$TARGET_DIR" && pwd -P)"

# Servo builds spawn hundreds of parallel rustc+sccache clients; source hashing
# trips EMFILE on macOS launchd's default soft limit (256).
current_nofile="$(ulimit -Sn)"
if [ "$current_nofile" != "unlimited" ] && [ "$current_nofile" -lt 65536 ]; then
  ulimit -n 65536 2>/dev/null || true
fi

CACHE_ROOT="${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}"
export MOZBUILD_STATE_PATH="${MOZBUILD_STATE_PATH:-$CACHE_ROOT/mozbuild}"
TOOLCHAIN_DIR="$CACHE_ROOT/toolchain"

mkdir -p "$MOZBUILD_STATE_PATH" "$TOOLCHAIN_DIR"

portable_lock_helper() {
  local source="$SCRIPT_DIR/cargo-cache-lock.rs"
  local version helper_dir helper temp
  version="$(cksum "$source" | awk '{print $1 "-" $2}')"
  helper_dir="$TOOLCHAIN_DIR/portable-lock-$version"
  helper="$helper_dir/cargo-cache-lock"
  if [ ! -x "$helper" ]; then
    mkdir -p "$helper_dir"
    temp="$helper.$$"
    rustc --edition=2024 -O "$source" -o "$temp"
    chmod +x "$temp"
    mv "$temp" "$helper"
  fi
  printf '%s\n' "$helper"
}

TARGET_BUILD_LOCK_PID=""
TARGET_BUILD_LOCK_CHANNEL_DIR=""
SCCACHE_CONFIG_LOCK_KIND=""
SCCACHE_CONFIG_LOCK_PID=""
SCCACHE_CONFIG_LOCK_CHANNEL_DIR=""

acquire_target_build_lock() {
  local helper release_fifo ready_fifo ready
  helper="$(portable_lock_helper)"
  TARGET_BUILD_LOCK_CHANNEL_DIR="$(mktemp -d "$CACHE_ROOT/target-lock.XXXXXX")"
  release_fifo="$TARGET_BUILD_LOCK_CHANNEL_DIR/release"
  ready_fifo="$TARGET_BUILD_LOCK_CHANNEL_DIR/ready"
  mkfifo "$release_fifo" "$ready_fifo"
  "$helper" shared "$TARGET_DIR/.cargo-build-lock" \
    <"$release_fifo" >"$ready_fifo" &
  TARGET_BUILD_LOCK_PID="$!"
  exec 6>"$release_fifo"
  exec 8<"$ready_fifo"
  if ! IFS= read -r ready <&8 || [ "$ready" != "locked" ]; then
    exec 6>&-
    exec 8<&-
    wait "$TARGET_BUILD_LOCK_PID" || true
    rm -f "$release_fifo" "$ready_fifo"
    rmdir "$TARGET_BUILD_LOCK_CHANNEL_DIR" 2>/dev/null || true
    return 1
  fi
}

release_target_build_lock() {
  [ -n "$TARGET_BUILD_LOCK_PID" ] || return 0
  printf 'release\n' >&6 || true
  exec 6>&-
  exec 8<&-
  wait "$TARGET_BUILD_LOCK_PID" || true
  rm -f \
    "$TARGET_BUILD_LOCK_CHANNEL_DIR/release" \
    "$TARGET_BUILD_LOCK_CHANNEL_DIR/ready"
  rmdir "$TARGET_BUILD_LOCK_CHANNEL_DIR" 2>/dev/null || true
  TARGET_BUILD_LOCK_PID=""
  TARGET_BUILD_LOCK_CHANNEL_DIR=""
}

acquire_sccache_config_lock() {
  local lock_file="$1"
  local helper ready release_fifo ready_fifo
  if command -v flock >/dev/null 2>&1 \
    && [ "${HYPERCOLOR_FORCE_PORTABLE_LOCK:-0}" != "1" ]; then
    exec 9>"$lock_file"
    flock 9
    SCCACHE_CONFIG_LOCK_KIND="flock"
    return 0
  fi

  helper="$(portable_lock_helper)"
  SCCACHE_CONFIG_LOCK_CHANNEL_DIR="$(mktemp -d "$CACHE_ROOT/sccache-lock.XXXXXX")"
  release_fifo="$SCCACHE_CONFIG_LOCK_CHANNEL_DIR/release"
  ready_fifo="$SCCACHE_CONFIG_LOCK_CHANNEL_DIR/ready"
  mkfifo "$release_fifo" "$ready_fifo"
  "$helper" exclusive "$lock_file" <"$release_fifo" >"$ready_fifo" &
  SCCACHE_CONFIG_LOCK_PID="$!"
  exec 5>"$release_fifo"
  exec 4<"$ready_fifo"
  if ! IFS= read -r ready <&4 || [ "$ready" != "locked" ]; then
    exec 5>&-
    exec 4<&-
    wait "$SCCACHE_CONFIG_LOCK_PID" || true
    rm -f "$release_fifo" "$ready_fifo"
    rmdir "$SCCACHE_CONFIG_LOCK_CHANNEL_DIR" 2>/dev/null || true
    return 1
  fi
  SCCACHE_CONFIG_LOCK_KIND="helper"
}

release_sccache_config_lock() {
  case "$SCCACHE_CONFIG_LOCK_KIND" in
    flock)
      exec 9>&-
      ;;
    helper)
      printf 'release\n' >&5 || true
      exec 5>&-
      exec 4<&-
      wait "$SCCACHE_CONFIG_LOCK_PID" || true
      rm -f \
        "$SCCACHE_CONFIG_LOCK_CHANNEL_DIR/release" \
        "$SCCACHE_CONFIG_LOCK_CHANNEL_DIR/ready"
      rmdir "$SCCACHE_CONFIG_LOCK_CHANNEL_DIR" 2>/dev/null || true
      ;;
  esac
  SCCACHE_CONFIG_LOCK_KIND=""
  SCCACHE_CONFIG_LOCK_PID=""
  SCCACHE_CONFIG_LOCK_CHANNEL_DIR=""
}

cleanup_locks() {
  release_sccache_config_lock
  release_target_build_lock
}

prune_stale_turbojpeg_cmake_cache() {
  [ -d "$TARGET_DIR" ] || return 0

  local cache_path stale_root
  while IFS= read -r -d '' cache_path; do
    if grep -Eq '^(CMAKE_INSTALL_PREFIX:PATH=/opt/libjpeg-turbo|ENABLE_SHARED:BOOL=ON|REQUIRE_SIMD:BOOL=OFF)$' "$cache_path"; then
      stale_root="${cache_path%/out/build/CMakeCache.txt}"
      echo "[cargo-cache] pruning stale turbojpeg CMake cache: $stale_root"
      rm -rf "$stale_root"
    fi
  done < <(find "$TARGET_DIR" -path '*/build/turbojpeg-sys-*/out/build/CMakeCache.txt' -print0)
}

HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"

if ld.lld --help 2>/dev/null | grep -Fq -- '--compress-debug-sections=[none,zlib,zstd]'; then
  case "$HOST_TRIPLE" in
    x86_64-unknown-linux-gnu)
      export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS:--C link-arg=-fuse-ld=lld -C link-arg=-Wl,--compress-debug-sections=zstd}"
      ;;
    aarch64-unknown-linux-gnu)
      export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS:--C link-arg=-fuse-ld=lld -C link-arg=-Wl,--compress-debug-sections=zstd}"
      ;;
  esac
fi

if [ "$HOST_TRIPLE" = "x86_64-pc-windows-msvc" ]; then
  export CARGO_TARGET_DIR="$TARGET_DIR"
  PS_WRAPPER="$SCRIPT_DIR/cargo-cache-build.ps1"
  if command -v cygpath >/dev/null 2>&1; then
    PS_WRAPPER="$(cygpath -w "$PS_WRAPPER")"
  fi
  exec powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$PS_WRAPPER" "$@"
fi

acquire_target_build_lock
trap 'cleanup_locks' EXIT
prune_stale_turbojpeg_cmake_cache

SCCACHE_BIN="$(command -v sccache || true)"
CCACHE_BIN="$(command -v ccache || true)"
DISABLE_SCCACHE="${HYPERCOLOR_NO_SCCACHE:-0}"

RELEASE_LIKE_PROFILE=0
for ((i = 1; i <= $#; i++)); do
  arg="${!i}"
  case "$arg" in
    --release)
      RELEASE_LIKE_PROFILE=1
      ;;
    --profile)
      next_index=$((i + 1))
      if [ "$next_index" -le "$#" ]; then
        next_arg="${!next_index}"
        if [ "$next_arg" = "release" ] || [ "$next_arg" = "bench" ]; then
          RELEASE_LIKE_PROFILE=1
        fi
      fi
      ;;
    --profile=release | --profile=bench)
      RELEASE_LIKE_PROFILE=1
      ;;
  esac
done

# sccache is the clean-target sharing layer: every worktree keeps its own
# target dir (so parallel agent builds never contend on Cargo's target lock),
# while reusable compiles hit one bounded cache under the shared cache root.
# Released sccache versions keep Rust artifacts checkout-sensitive, but
# SCCACHE_BASEDIRS still normalizes C and C++ compiles across worktrees.
# sccache and incremental compilation are mutually exclusive (sccache 0.17
# hard-errors on either the env var or -Cincremental), so the wrapper picks
# per command: codegen-heavy tree ops go through sccache; metadata-only ops
# (check/clippy) keep incremental because sccache cannot cache
# --emit=metadata units at all.
USES_TARGET_DIR=0
case "$CARGO_SUBCOMMAND" in
  build | check | test | bench | clippy | doc | run | clean | nextest)
    USES_TARGET_DIR=1
    ;;
esac
if [ "$CARGO_SUBCOMMAND" = "tauri" ] && [ "$TAURI_SUBCOMMAND" = "build" ]; then
  USES_TARGET_DIR=1
fi

# Cargo hashes an exported CARGO_TARGET_DIR into compiler invocations, which
# prevents a clean target from reusing otherwise identical Rust objects.
# Passing the location through the subcommand flag preserves target isolation
# without poisoning sccache's compiler key.
if [ "$USES_TARGET_DIR" -eq 1 ]; then
  if [ "$TARGET_DIR_IS_EXPLICIT" -eq 0 ]; then
    cargo_args=("$@")
    if [ "$CARGO_SUBCOMMAND" = "tauri" ]; then
      has_forwarding_delimiter=0
      for arg in "${cargo_args[@]:CARGO_SUBCOMMAND_INDEX}"; do
        if [ "$arg" = "--" ]; then
          has_forwarding_delimiter=1
          break
        fi
      done
      if [ "$has_forwarding_delimiter" -eq 1 ]; then
        set -- "${cargo_args[@]}" --target-dir "$TARGET_DIR"
      else
        set -- "${cargo_args[@]}" -- --target-dir "$TARGET_DIR"
      fi
    else
      set -- \
        "${cargo_args[@]:0:CARGO_SUBCOMMAND_INDEX}" \
        --target-dir "$TARGET_DIR" \
        "${cargo_args[@]:CARGO_SUBCOMMAND_INDEX}"
    fi
  fi
  unset CARGO_TARGET_DIR
fi

if [ "$TOP_LEVEL_COMMAND" = "trunk" ] || [ "$TOP_LEVEL_COMMAND" = "trunk.exe" ]; then
  real_cargo="$(command -v cargo || true)"
  if [ -z "$real_cargo" ]; then
    echo "[cargo-cache] cargo not found for Trunk build" >&2
    exit 127
  fi
  shim_source="$SCRIPT_DIR/cargo-cache-cargo-shim.sh"
  shim_version="$(cksum "$shim_source" | awk '{print $1 "-" $2}')"
  cargo_shim_dir="$TARGET_DIR/.hypercolor-toolchain/cargo-shim-$shim_version"
  mkdir -p "$cargo_shim_dir"
  if [ ! -e "$cargo_shim_dir/cargo" ]; then
    shim_temp="$cargo_shim_dir/cargo.$$"
    cp "$shim_source" "$shim_temp"
    chmod +x "$shim_temp"
    mv "$shim_temp" "$cargo_shim_dir/cargo"
  fi
  export HYPERCOLOR_REAL_CARGO="$real_cargo"
  export HYPERCOLOR_NESTED_CARGO_TARGET_DIR="$TARGET_DIR"
  export PATH="$cargo_shim_dir:$PATH"
  unset CARGO_TARGET_DIR
fi

FORCE_SCCACHE="${HYPERCOLOR_FORCE_SCCACHE:-0}"
ITERATE="${HYPERCOLOR_ITERATE:-0}"
# `run` stays incremental: it is the edit-run iteration loop, and a measured
# edit-rebuild of hypercolor-core is ~45s non-incremental vs ~11s
# incremental. Whole-tree ops win with sccache instead.
WANTS_SCCACHE=0
case "$CARGO_SUBCOMMAND" in
  build | test | bench | nextest) WANTS_SCCACHE=1 ;;
esac
if [ "$RELEASE_LIKE_PROFILE" -eq 1 ] || [ "$FORCE_SCCACHE" = "1" ] || [ "$FORCE_SCCACHE" = "true" ]; then
  WANTS_SCCACHE=1
fi
if [ "$DISABLE_SCCACHE" = "1" ] || [ "$DISABLE_SCCACHE" = "true" ] \
  || [ "$ITERATE" = "1" ] || [ "$ITERATE" = "true" ] \
  || { [ -n "${CARGO_INCREMENTAL:-}" ] && [ "$CARGO_INCREMENTAL" != "0" ]; }; then
  WANTS_SCCACHE=0
fi

add_sccache_basedir() {
  local candidate="$1"
  [ -d "$candidate" ] || return 0

  local logical physical existing
  logical="$(cd "$candidate" && pwd -L)"
  physical="$(cd "$candidate" && pwd -P)"
  for candidate in "$logical" "$physical"; do
    for existing in "${SCCACHE_BASEDIR_LIST[@]:-}"; do
      [ "$existing" = "$candidate" ] && continue 2
    done
    SCCACHE_BASEDIR_LIST+=("$candidate")
  done
}

collect_sccache_basedirs() {
  SCCACHE_BASEDIR_LIST=()
  add_sccache_basedir "$ROOT_DIR"

  local caller_root repo worktree configured
  caller_root="$(git -C "$CALLER_DIR" rev-parse --show-toplevel 2>/dev/null || true)"
  [ -z "$caller_root" ] || add_sccache_basedir "$caller_root"

  for repo in "$HOME/dev/hypercolor" "$HOME/dev/hypercolor.lighting"; do
    [ -e "$repo/.git" ] || continue
    while IFS= read -r worktree; do
      [ -d "$worktree/oss" ] && add_sccache_basedir "$worktree/oss"
      add_sccache_basedir "$worktree"
    done < <(git -C "$repo" worktree list --porcelain | sed -n 's/^worktree //p')
  done

  if [ -n "${SCCACHE_BASEDIRS:-}" ]; then
    while IFS= read -r configured; do
      [ -z "$configured" ] || add_sccache_basedir "$configured"
    done < <(printf '%s\n' "$SCCACHE_BASEDIRS" | tr ':' '\n')
  fi

  local i j swap
  for ((i = 0; i < ${#SCCACHE_BASEDIR_LIST[@]}; i++)); do
    for ((j = i + 1; j < ${#SCCACHE_BASEDIR_LIST[@]}; j++)); do
      if [ "${#SCCACHE_BASEDIR_LIST[j]}" -gt "${#SCCACHE_BASEDIR_LIST[i]}" ] \
        || { [ "${#SCCACHE_BASEDIR_LIST[j]}" -eq "${#SCCACHE_BASEDIR_LIST[i]}" ] \
          && [[ "${SCCACHE_BASEDIR_LIST[j]}" < "${SCCACHE_BASEDIR_LIST[i]}" ]]; }; then
        swap="${SCCACHE_BASEDIR_LIST[i]}"
        SCCACHE_BASEDIR_LIST[i]="${SCCACHE_BASEDIR_LIST[j]}"
        SCCACHE_BASEDIR_LIST[j]="$swap"
      fi
    done
  done

  local joined
  joined="$(IFS=:; printf '%s' "${SCCACHE_BASEDIR_LIST[*]}")"
  export SCCACHE_BASEDIRS="$joined"
}

hypercolor_sccache_clients_are_active() {
  local pid
  while IFS= read -r pid; do
    [ -r "/proc/$pid/environ" ] || return 0
    if tr '\0' '\n' <"/proc/$pid/environ" \
      | grep -Fqx "SCCACHE_SERVER_UDS=$SCCACHE_SERVER_UDS"; then
      return 0
    fi
  done < <(pgrep -u "$(id -u)" -f '[s]ccache .+' 2>/dev/null || true)
  return 1
}

refresh_sccache_server_config() {
  local state_file="$CACHE_ROOT/sccache-server-config"
  local lock_file="$CACHE_ROOT/sccache-config.lock"
  local desired
  local current=""
  desired="$("$SCCACHE_BIN" --version)|$SCCACHE_CACHE_SIZE|$SCCACHE_BASEDIRS"
  [ ! -f "$state_file" ] || current="$(<"$state_file")"
  [ "$current" = "$desired" ] && return 0

  acquire_sccache_config_lock "$lock_file"

  [ ! -f "$state_file" ] || current="$(<"$state_file")"
  if [ "$current" = "$desired" ]; then
    release_sccache_config_lock
    return 0
  fi
  if hypercolor_sccache_clients_are_active; then
    echo "[cargo-cache] active Hypercolor compiles deferred sccache normalization refresh"
    release_sccache_config_lock
    return 0
  fi

  "$SCCACHE_BIN" --stop-server >/dev/null 2>&1 || true
  if ! "$SCCACHE_BIN" --start-server 4>&- 5>&- 6>&- 8>&- 9>&- >/dev/null; then
    release_sccache_config_lock
    return 1
  fi
  printf '%s\n' "$desired" >"$state_file.tmp.$$"
  mv "$state_file.tmp.$$" "$state_file"
  release_sccache_config_lock
  echo "[cargo-cache] refreshed checkout path normalization"
}

if [ -n "$SCCACHE_BIN" ] && { [ "$WANTS_SCCACHE" -eq 1 ] || [ -z "$CCACHE_BIN" ]; }; then
  export SCCACHE_DIR="${SCCACHE_DIR:-$CACHE_ROOT/sccache}"
  mkdir -p "$SCCACHE_DIR"
  export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-${HYPERCOLOR_SCCACHE_SIZE:-75G}}"
  export SCCACHE_SERVER_UDS="${SCCACHE_SERVER_UDS:-$CACHE_ROOT/sccache.sock}"
  collect_sccache_basedirs
  refresh_sccache_server_config
fi

if [ -n "$SCCACHE_BIN" ] && [ "$WANTS_SCCACHE" -eq 1 ]; then
  export RUSTC_WRAPPER="${RUSTC_WRAPPER:-$SCCACHE_BIN}"
  export CARGO_INCREMENTAL="0"
  echo "[cargo-cache] sccache mode: Rust cached, incremental off (cap $SCCACHE_CACHE_SIZE)"
  echo "[cargo-cache] SCCACHE_DIR=$SCCACHE_DIR (HYPERCOLOR_NO_SCCACHE=1 or HYPERCOLOR_ITERATE=1 to disable)"
else
  # An ambient sccache RUSTC_WRAPPER combined with incremental hard-fails
  # every compile; drop it rather than let the build die.
  case "$(basename "${RUSTC_WRAPPER:-}")" in
    sccache*)
      unset RUSTC_WRAPPER
      echo "[cargo-cache] unset ambient sccache RUSTC_WRAPPER for incremental mode"
      ;;
  esac
  export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"
  if [ -z "$SCCACHE_BIN" ]; then
    echo "[cargo-cache] sccache not found; run 'cargo install --locked sccache' for shared build caching"
  fi
  echo "[cargo-cache] incremental mode: CARGO_INCREMENTAL=$CARGO_INCREMENTAL"
fi

if [ "$HOST_TRIPLE" = "x86_64-unknown-linux-gnu" ] \
  && command -v clang >/dev/null 2>&1 \
  && command -v ld.lld >/dev/null 2>&1; then
  if [ ! -x "$TOOLCHAIN_DIR/clang-lld" ]; then
    cat >"$TOOLCHAIN_DIR/clang-lld" <<'EOF'
#!/usr/bin/env bash
exec clang -fuse-ld=lld "$@"
EOF
    chmod +x "$TOOLCHAIN_DIR/clang-lld"
  fi

  # lld itself is not this gate's doing: .cargo/config.toml passes
  # -fuse-ld=lld for both Linux targets, so the default cc driver already
  # links with it. All this changes is the driver, gcc to clang, which is
  # why it stays scoped to the host it was measured on.
  case "$HOST_TRIPLE" in
    x86_64-unknown-linux-gnu)
      export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER:-$TOOLCHAIN_DIR/clang-lld}"
      ;;
  esac
  echo "[cargo-cache] using clang + lld for faster link steps"
fi

COMPILER_CACHE_BIN=""
COMPILER_CACHE_NAME=""

if [ -n "$CCACHE_BIN" ]; then
  COMPILER_CACHE_BIN="$CCACHE_BIN"
  COMPILER_CACHE_NAME="ccache"
  export CCACHE_DIR="${CCACHE_DIR:-$CACHE_ROOT/ccache}"
  mkdir -p "$CCACHE_DIR"
  echo "[cargo-cache] ccache enabled for C/C++ compilation"
  echo "[cargo-cache] CCACHE_DIR=$CCACHE_DIR"
elif [ -n "$SCCACHE_BIN" ]; then
  COMPILER_CACHE_BIN="$SCCACHE_BIN"
  COMPILER_CACHE_NAME="sccache"
  echo "[cargo-cache] sccache enabled for C/C++ compilation"
else
  echo "[cargo-cache] no C/C++ compiler cache found; continuing without one"
fi

if [ -n "$COMPILER_CACHE_BIN" ]; then
  if [ ! -x "$TOOLCHAIN_DIR/cc" ]; then
    cat >"$TOOLCHAIN_DIR/cc" <<EOF
#!/usr/bin/env bash
exec "$COMPILER_CACHE_BIN" "\$(command -v cc)" "\$@"
EOF
    chmod +x "$TOOLCHAIN_DIR/cc"
  fi

  if [ ! -x "$TOOLCHAIN_DIR/cxx" ]; then
    cat >"$TOOLCHAIN_DIR/cxx" <<EOF
#!/usr/bin/env bash
exec "$COMPILER_CACHE_BIN" "\$(command -v c++)" "\$@"
EOF
    chmod +x "$TOOLCHAIN_DIR/cxx"
  fi

  export CC="${CC:-$TOOLCHAIN_DIR/cc}"
  export CXX="${CXX:-$TOOLCHAIN_DIR/cxx}"
  echo "[cargo-cache] CC/CXX routed through $COMPILER_CACHE_NAME wrappers"
fi

# Patch glslopt's bundled C11 thread emulation to be compatible with newer glibc.
# glibc 2.39+ with _GNU_SOURCE exposes once_flag/call_once in <stdlib.h>,
# conflicting with threads_posix.h's pthread-based typedefs.
for tp in "$HOME"/.cargo/registry/src/*/glslopt-*/glsl-optimizer/include/c11/threads_posix.h; do
  [ -f "$tp" ] || continue
  if ! grep -q '__once_flag_defined' "$tp"; then
    python3 -c "
import pathlib, sys
p = pathlib.Path(sys.argv[1])
src = p.read_text()
src = src.replace(
    'typedef pthread_once_t  once_flag;',
    '#ifndef __once_flag_defined\ntypedef pthread_once_t  once_flag;\n#endif'
)
src = src.replace(
    'static inline void\ncall_once(once_flag *flag, void (*func)(void))\n{\n    pthread_once(flag, func);\n}',
    '#ifndef __once_flag_defined\nstatic inline void\ncall_once(once_flag *flag, void (*func)(void))\n{\n    pthread_once(flag, func);\n}\n#endif'
)
p.write_text(src)
" "$tp"
    echo "[cargo-cache] patched glslopt threads_posix.h for glibc C23 compat"
  fi
done

# Patch mozjs_sys's linker detection for Xcode 26+.
# Apple's linker changed its PROJECT identifier from "dyld" to "ld-<version>"
# (e.g. "PROGRAM:ld PROJECT:ld-1266.8"), which mozjs's toolchain.configure
# doesn't recognize, causing "Failed to find an adequate linker".
for tc in "$HOME"/.cargo/registry/src/*/mozjs_sys-*/mozjs/build/moz.configure/toolchain.configure; do
  [ -f "$tc" ] || continue
  if ! grep -q 'PROGRAM:ld PROJECT:ld' "$tc"; then
    sed -i.bak 's|"PROGRAM:ld  PROJECT:dyld" in stderr|"PROGRAM:ld  PROJECT:dyld" in stderr or "PROGRAM:ld PROJECT:ld" in stderr|' "$tc"
    rm -f "$tc.bak"
    echo "[cargo-cache] patched mozjs toolchain.configure for Xcode 26 linker detection"
  fi
done

echo "[cargo-cache] target directory=$TARGET_DIR"
echo "[cargo-cache] MOZBUILD_STATE_PATH=$MOZBUILD_STATE_PATH"

echo "[cargo-cache] running: $*"
release_sccache_config_lock
lock_helper="$(portable_lock_helper)"
exec "$lock_helper" run-shared \
  "$TARGET_DIR/.cargo-build-lock" "$TARGET_DIR/.cargo-build-pgid.$$" \
  "$TARGET_BUILD_LOCK_PID" "$@" 4>&- 5>&- 7>&- 9>&-
