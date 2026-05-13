# Servo Build Caching

The `servo` crate pulls in `mozjs_sys`, which compiles a large native C++
codebase. The first build is expensive. Subsequent builds should be fast when
Cargo, Mozilla build state, and compiler caches reuse the same target layout.

Servo is the normal HTML-effect rendering path. CI must keep a real Servo E2E
lane; the CPU-only E2E lane is a smoke fallback for the builtin renderer shape,
not a substitute for Servo coverage.

## Local Workflow

Use the shared Cargo cache wrapper for most commands:

```bash
./scripts/cargo-cache-build.sh cargo build --workspace
```

The older Servo wrapper remains as a convenience entrypoint:

```bash
./scripts/servo-cache-build.sh
```

With no arguments, it runs:

```bash
cargo test -p hypercolor-core --features servo --all-targets
```

Override it with any command:

```bash
./scripts/servo-cache-build.sh cargo clippy -p hypercolor-core --features servo --all-targets -- -D warnings
```

Run the daemon with Servo-enabled HTML rendering:

```bash
just daemon-servo
```

Build the normal Servo E2E stack without running browsers or starting the
daemon:

```bash
just e2e-build
```

The CPU smoke stack is available separately:

```bash
just e2e-build-cpu
```

The shared wrapper configures:

- `CARGO_TARGET_DIR=$HOME/.cache/hypercolor/target` (unless already set)
- `MOZBUILD_STATE_PATH=$HOME/.cache/hypercolor/mozbuild` (unless already set)
- Cargo incremental compilation for local dev and preview-style builds
- `sccache` as `RUSTC_WRAPPER` for release/bench builds, or whenever
  `HYPERCOLOR_FORCE_SCCACHE=1`
- `clang` + `ld.lld` for faster link steps on `x86_64-unknown-linux-gnu` when available
- `ccache` for `CC`/`CXX` when installed, otherwise `sccache` if available

## Verify Cache Hits

```bash
ccache -s
sccache --show-stats
```

Look for increasing cache hit counts after the first Servo build.

## CI Cache Topology

The reusable action `.github/actions/rust-build-cache` configures GitHub
Actions builds with:

- `mozilla-actions/sccache-action` using GitHub's sccache backend
- `HYPERCOLOR_FORCE_SCCACHE=1`
- `CARGO_INCREMENTAL=0`
- `Swatinem/rust-cache` for Cargo and extra cache directories
- `.cache/hypercolor/mozbuild`
- `.cache/hypercolor/toolchain`
- `.cache/hypercolor/ccache`

The scheduled/manual `.github/workflows/servo-cache-warm.yml` workflow warms
three compatible shapes:

| Suite | Shared Key | Extra Key | Purpose |
| ----- | ---------- | --------- | ------- |
| Core Servo | `servo-core` | empty | core Servo check, test, and clippy artifacts |
| Daemon Servo | `servo-daemon` | empty | daemon Servo check, test, and clippy artifacts |
| E2E Servo | `servo-daemon` | `e2e-dev-v1` | daemon and CLI binaries for the normal E2E stack |

The main CI workflow reuses those same shared keys in the explicit Servo check,
test, and E2E build lanes. Shared non-Servo Rust lanes deliberately keep Servo
out of their dependency graph so routine crates do not rebuild `servo-script`.

## E2E Policy

CI builds and runs two E2E stacks:

- **Servo:** `just e2e-build`, default daemon features, real HTML effects, and
  `e2e/tests/servo.spec.mjs` telemetry proof.
- **CPU Smoke:** `just e2e-build-cpu`, builtin-driver daemon feature set, and a
  reduced proof that the non-Servo stack still boots.

The Servo lane is the launch gate. Do not remove it to save time. If it gets
slow, warm or repair the cache instead.

## Cache Miss Checklist

When CI starts compiling Servo from scratch:

1. Check whether `servo-cache-warm.yml` is green on the same branch or `main`.
2. Confirm the PR lane uses the same `shared-key`, `key`, and target directory
   shape as the warmer.
3. Confirm `Cargo.lock`, `rust-toolchain.toml`, and Servo feature sets did not
   change.
4. Inspect `Swatinem/rust-cache` restore logs for a key miss.
5. Inspect `sccache --show-stats` when a job exposes stats.

If the pinned Servo version and toolchain are unchanged, warm builds should
avoid repeating the costly native compile.
