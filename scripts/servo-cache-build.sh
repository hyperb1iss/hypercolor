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
  TOOLCHAIN_DIR="$CACHE_ROOT/toolchain"
  mkdir -p "$CCACHE_DIR" "$TOOLCHAIN_DIR"

  # mozjs_sys compiles a very large C++ codebase; route CC/CXX through ccache.
  # Use wrapper executables instead of "ccache <tool>" strings because some
  # configure steps expect these variables to be plain executable paths.
  if [ ! -x "$TOOLCHAIN_DIR/cc" ]; then
    cat >"$TOOLCHAIN_DIR/cc" <<'EOF'
#!/usr/bin/env bash
exec ccache "$(command -v cc)" "$@"
EOF
    chmod +x "$TOOLCHAIN_DIR/cc"
  fi

  if [ ! -x "$TOOLCHAIN_DIR/cxx" ]; then
    cat >"$TOOLCHAIN_DIR/cxx" <<'EOF'
#!/usr/bin/env bash
exec ccache "$(command -v c++)" "$@"
EOF
    chmod +x "$TOOLCHAIN_DIR/cxx"
  fi

  export CC="${CC:-$TOOLCHAIN_DIR/cc}"
  export CXX="${CXX:-$TOOLCHAIN_DIR/cxx}"

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
