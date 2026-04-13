# Servo Build Caching

The `servo` crate pulls in `mozjs_sys`, which compiles a large native C++
codebase. The first build is expensive. Subsequent builds should be fast if
caches are stable and reused.

## Local Workflow

Use the Servo wrapper script:

```bash
./scripts/servo-cache-build.sh
```

Default command:

```bash
cargo test -p hypercolor-core --features servo --all-targets
```

Override with any command:

```bash
./scripts/servo-cache-build.sh cargo clippy -p hypercolor-core --features servo --all-targets -- -D warnings
```

For general workspace builds, use the shared wrapper:

```bash
./scripts/cargo-cache-build.sh cargo build --workspace
```

Run the daemon with Servo-enabled HTML rendering:

```bash
./scripts/run-preview-servo.sh
```

This command wraps:

```bash
cargo run -p hypercolor-daemon --features servo -- --bind 127.0.0.1:9420
```

The shared wrapper configures:

- `CARGO_TARGET_DIR=$HOME/.cache/hypercolor/target` (unless already set)
- `MOZBUILD_STATE_PATH=$HOME/.cache/hypercolor/mozbuild` (unless already set)
- `CARGO_INCREMENTAL=1` (unless already set)
- `sccache` as `RUSTC_WRAPPER` when available
- `clang` + `ld.lld` for faster link steps on `x86_64-unknown-linux-gnu` when available
- `ccache` for `CC`/`CXX` when installed, otherwise `sccache` if available

## Verify Cache Hits

```bash
ccache -s
sccache --show-stats
```

Look for increasing cache hit counts after the first Servo build.

## CI Caching Guidance

Cache these paths:

- Cargo target dir (`$CARGO_TARGET_DIR`)
- Cargo git checkout + index (`$CARGO_HOME/git`)
- Cargo registry (`$CARGO_HOME/registry`)
- Rust compiler cache (`$SCCACHE_DIR`)
- C/C++ cache (`$CCACHE_DIR`)
- Mozilla build state (`$MOZBUILD_STATE_PATH`)

Recommended cache key inputs:

- `Cargo.lock`
- `crates/hypercolor-core/Cargo.toml` (contains the pinned `servo` version)
- Rust toolchain version
- target triple

If the pinned `servo` version and toolchain are unchanged, warm builds
should avoid repeating the costly native compile.
