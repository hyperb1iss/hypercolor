#!/usr/bin/env bash
set -euo pipefail

# Shared Cargo build wrapper.
# Keeps Rust/C/C++ artifacts in stable cache locations and opportunistically
# enables compiler caches so whole-workspace builds warm up instead of starting
# from scratch on every clean target dir.

CACHE_ROOT="${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$CACHE_ROOT/target}"
export MOZBUILD_STATE_PATH="${MOZBUILD_STATE_PATH:-$CACHE_ROOT/mozbuild}"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"
TOOLCHAIN_DIR="$CACHE_ROOT/toolchain"

mkdir -p "$CARGO_TARGET_DIR" "$MOZBUILD_STATE_PATH" "$TOOLCHAIN_DIR"

HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
SCCACHE_BIN="$(command -v sccache || true)"
CCACHE_BIN="$(command -v ccache || true)"

if [ -n "$SCCACHE_BIN" ]; then
  export SCCACHE_DIR="${SCCACHE_DIR:-$CACHE_ROOT/sccache}"
  mkdir -p "$SCCACHE_DIR"
  export RUSTC_WRAPPER="${RUSTC_WRAPPER:-$SCCACHE_BIN}"
  echo "[cargo-cache] sccache enabled for Rust compilation"
  echo "[cargo-cache] SCCACHE_DIR=$SCCACHE_DIR"
else
  echo "[cargo-cache] sccache not found; Rust compilation will use Cargo incremental only"
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

echo "[cargo-cache] CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
echo "[cargo-cache] MOZBUILD_STATE_PATH=$MOZBUILD_STATE_PATH"

if [ "$#" -eq 0 ]; then
  set -- cargo build --workspace
fi

echo "[cargo-cache] running: $*"
exec "$@"
