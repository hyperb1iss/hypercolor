# Servo Build Caching

`libservo` pulls in `mozjs_sys`, which compiles a large native C++ codebase.
The first build is expensive. Subsequent builds should be fast if caches are
stable and reused.

## Local Workflow

Use the wrapper script:

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

The wrapper configures:

- `CARGO_TARGET_DIR=$HOME/.cache/hypercolor/target` (unless already set)
- `MOZBUILD_STATE_PATH=$HOME/.cache/hypercolor/mozbuild` (unless already set)
- `CARGO_INCREMENTAL=1` (unless already set)
- `ccache` integration for `CC`/`CXX`/`AR` when `ccache` exists

## Verify Cache Hits

```bash
ccache -s
```

Look for increasing cache hit counts after the first Servo build.

## CI Caching Guidance

Cache these paths:

- Cargo target dir (`$CARGO_TARGET_DIR`)
- Cargo git checkout + index (`$CARGO_HOME/git`)
- Cargo registry (`$CARGO_HOME/registry`)
- C/C++ cache (`$CCACHE_DIR`)
- Mozilla build state (`$MOZBUILD_STATE_PATH`)

Recommended cache key inputs:

- `Cargo.lock`
- `crates/hypercolor-core/Cargo.toml` (contains pinned `libservo` rev)
- Rust toolchain version
- target triple

If the pinned `libservo` revision and toolchain are unchanged, warm builds
should avoid repeating the costly native compile.
