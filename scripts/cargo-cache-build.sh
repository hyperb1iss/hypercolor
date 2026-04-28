#!/usr/bin/env bash
set -euo pipefail

# Shared Cargo build wrapper.
# Keeps Rust/C/C++ artifacts in stable cache locations and opportunistically
# enables compiler caches so whole-workspace builds warm up instead of starting
# from scratch on every clean target dir.

# Servo builds spawn hundreds of parallel rustc+sccache clients; source hashing
# trips EMFILE on macOS launchd's default soft limit (256).
current_nofile="$(ulimit -Sn)"
if [ "$current_nofile" != "unlimited" ] && [ "$current_nofile" -lt 65536 ]; then
  ulimit -n 65536 2>/dev/null || true
fi

CACHE_ROOT="${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$CACHE_ROOT/target}"
export MOZBUILD_STATE_PATH="${MOZBUILD_STATE_PATH:-$CACHE_ROOT/mozbuild}"
TOOLCHAIN_DIR="$CACHE_ROOT/toolchain"

mkdir -p "$CARGO_TARGET_DIR" "$MOZBUILD_STATE_PATH" "$TOOLCHAIN_DIR"

prune_stale_turbojpeg_cmake_cache() {
  [ -d "$CARGO_TARGET_DIR" ] || return 0

  local cache_path stale_root
  while IFS= read -r -d '' cache_path; do
    if grep -Eq '^(CMAKE_INSTALL_PREFIX:PATH=/opt/libjpeg-turbo|ENABLE_SHARED:BOOL=ON|REQUIRE_SIMD:BOOL=OFF)$' "$cache_path"; then
      stale_root="${cache_path%/out/build/CMakeCache.txt}"
      echo "[cargo-cache] pruning stale turbojpeg CMake cache: $stale_root"
      rm -rf "$stale_root"
    fi
  done < <(find "$CARGO_TARGET_DIR" -path '*/build/turbojpeg-sys-*/out/build/CMakeCache.txt' -print0)
}

prune_stale_turbojpeg_cmake_cache

HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
SCCACHE_BIN="$(command -v sccache || true)"
CCACHE_BIN="$(command -v ccache || true)"
ENABLE_RUST_SCCACHE=1

for ((i = 1; i <= $#; i++)); do
  arg="${!i}"
  case "$arg" in
    --release)
      ENABLE_RUST_SCCACHE=1
      ;;
    --profile)
      next_index=$((i + 1))
      if [ "$next_index" -le "$#" ]; then
        next_arg="${!next_index}"
        if [ "$next_arg" != "release" ] && [ "$next_arg" != "bench" ]; then
          ENABLE_RUST_SCCACHE=0
        fi
      fi
      ;;
    --profile=*)
      profile_name="${arg#--profile=}"
      if [ "$profile_name" != "release" ] && [ "$profile_name" != "bench" ]; then
        ENABLE_RUST_SCCACHE=0
      fi
      ;;
  esac
done

if [ "$#" -gt 0 ] && ! printf '%s\n' "$*" | grep -Eq -- '(^| )--release($| )|(^| )--profile(=| )(release|bench)($| )'; then
  ENABLE_RUST_SCCACHE=0
fi

if [ -n "$SCCACHE_BIN" ] && [ "$ENABLE_RUST_SCCACHE" -eq 1 ]; then
  export SCCACHE_DIR="${SCCACHE_DIR:-$CACHE_ROOT/sccache}"
  mkdir -p "$SCCACHE_DIR"
  export RUSTC_WRAPPER="${RUSTC_WRAPPER:-$SCCACHE_BIN}"
  # sccache rejects incremental compilation entirely, so prefer the
  # compiler cache and force a compatible Cargo setting.
  unset CARGO_INCREMENTAL || true
  export CARGO_BUILD_INCREMENTAL="false"
  export CARGO_PROFILE_DEV_INCREMENTAL="false"
  export CARGO_PROFILE_RELEASE_INCREMENTAL="false"
  export CARGO_PROFILE_TEST_INCREMENTAL="false"
  export CARGO_PROFILE_BENCH_INCREMENTAL="false"
  export CARGO_PROFILE_PREVIEW_INCREMENTAL="false"
  echo "[cargo-cache] sccache enabled for Rust compilation"
  echo "[cargo-cache] SCCACHE_DIR=$SCCACHE_DIR"
  echo "[cargo-cache] incremental compilation disabled for sccache compatibility"
else
  export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"
  if [ -n "$SCCACHE_BIN" ]; then
    echo "[cargo-cache] skipping rust sccache for dev/preview-style build; using Cargo incremental instead"
  else
    echo "[cargo-cache] sccache not found; Rust compilation will use Cargo incremental only"
  fi
  echo "[cargo-cache] CARGO_INCREMENTAL=$CARGO_INCREMENTAL"
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

  export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER:-$TOOLCHAIN_DIR/clang-lld}"
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

echo "[cargo-cache] CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
echo "[cargo-cache] MOZBUILD_STATE_PATH=$MOZBUILD_STATE_PATH"

if [ "$#" -eq 0 ]; then
  set -- cargo build --workspace
fi

echo "[cargo-cache] running: $*"
exec "$@"
