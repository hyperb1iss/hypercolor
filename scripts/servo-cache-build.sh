#!/usr/bin/env bash
set -euo pipefail

# Hypercolor Servo build wrapper.
# Keeps Cargo artifacts and C/C++ object caches in stable locations so
# pinned libservo + mozjs builds are reused across runs.

CACHE_ROOT="${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$CACHE_ROOT/target}"
export MOZBUILD_STATE_PATH="${MOZBUILD_STATE_PATH:-$CACHE_ROOT/mozbuild}"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"

mkdir -p "$CARGO_TARGET_DIR" "$MOZBUILD_STATE_PATH"

if command -v ccache >/dev/null 2>&1; then
  export CCACHE_DIR="${CCACHE_DIR:-$CACHE_ROOT/ccache}"
  mkdir -p "$CCACHE_DIR"

  # mozjs_sys compiles a very large C++ codebase; route compilers through ccache.
  export CC="${CC:-ccache cc}"
  export CXX="${CXX:-ccache c++}"
  export AR="${AR:-ccache ar}"

  echo "[servo-cache] ccache enabled"
  echo "[servo-cache] CCACHE_DIR=$CCACHE_DIR"
else
  echo "[servo-cache] ccache not found; continuing without C/C++ compiler cache"
fi

echo "[servo-cache] CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
echo "[servo-cache] MOZBUILD_STATE_PATH=$MOZBUILD_STATE_PATH"

if [ "$#" -eq 0 ]; then
  set -- cargo test -p hypercolor-core --features servo --all-targets
fi

echo "[servo-cache] running: $*"
exec "$@"
