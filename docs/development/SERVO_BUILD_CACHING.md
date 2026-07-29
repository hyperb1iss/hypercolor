# Servo Build Caching

The `servo` crate pulls in `mozjs_sys`, which compiles a large native C++
codebase. The first build is expensive. Subsequent builds should stay fast when
Cargo uses the workspace target tree and the heavy Mozilla/compiler caches stay
outside the repo.

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

- `CARGO_TARGET_DIR=<repo>/target` (unless already set)
- `MOZBUILD_STATE_PATH=$HOME/.cache/hypercolor/mozbuild` (unless already set)
- `sccache` as `RUSTC_WRAPPER` for whole-tree codegen commands
  (`cargo build`, `test`, `bench`, and anything release/bench-profiled)
  when installed, with a bounded on-disk cache (default `75G`, override
  with `HYPERCOLOR_SCCACHE_SIZE`). sccache and incremental compilation are
  mutually exclusive, so these commands run with `CARGO_INCREMENTAL=0`.
- Cargo incremental compilation for iteration and metadata commands:
  `cargo run` (the edit-run loop; a measured hypercolor-core edit-rebuild
  is ~45s non-incremental vs ~11s incremental), `cargo check`, and
  `clippy` (sccache cannot cache `--emit=metadata` units). The
  iteration-shaped recipes (`just test-crate`, `test-one`, `app`, and the
  Windows `just dev` daemon build) pin incremental via
  `HYPERCOLOR_ITERATE=1`.
- Opt-outs: `HYPERCOLOR_NO_SCCACHE=1` disables sccache for the session;
  `HYPERCOLOR_ITERATE=1` does the same per invocation when you want
  incremental rebuilds in a tight edit loop; a pre-set non-zero
  `CARGO_INCREMENTAL` always wins. Alternating the same profile tree
  between the two modes rebuilds only workspace crates (~50s measured),
  never dependencies.
- `rust-lld` as the linker on `x86_64-pc-windows-msvc`
  (`HYPERCOLOR_NO_FAST_LINK=1` to opt out)
- `clang` + `ld.lld` for faster link steps on `x86_64-unknown-linux-gnu` when available
- C/C++ caching for `cc`- and CMake-driven native deps (mozangle/ANGLE,
  turbojpeg): `ccache` or `sccache` on Unix, `sccache` around `cl.exe` on
  Windows

## Cross-Worktree Topology

Multiple worktrees (and multiple agents) build this repo concurrently. The
sharing layer is the compile cache, not the target dir:

- **Per-worktree `target/`** stays the default. Cargo's target lock is
  coarse; a shared target dir would serialize parallel builds across
  worktrees and thrash on feature-shape differences.
- **Shared, bounded caches** live under `$HOME/.cache/hypercolor`
  (`HYPERCOLOR_CACHE_DIR` to relocate): `sccache/` for compiled units,
  `mozbuild/` for SpiderMonkey build state. A second worktree's cold build
  becomes mostly cache hits without any cross-worktree locking.
- **Incompatible feature shapes get isolated lanes**: `just e2e-build-cpu`
  builds into `target/cpu-smoke` so the `--no-default-features` unification
  never churns the daily tree.
- **`mozjs_sys` uses prebuilt SpiderMonkey archives by default.** It falls
  back to a source build silently, e.g. when a package profile override
  drops `mozjs_sys` below `-O3`; keep the `opt-level = 3` overrides in
  `Cargo.toml` intact.

## Disk Bounds

Target dirs grow without bound as toolchains, lockfiles, and feature shapes
churn — Cargo never garbage-collects them. Bound them with:

```bash
just disk          # per-profile + shared-cache usage report
just gc            # sweep orphaned-toolchain and >14-day artifacts here
just gc-worktrees  # the same sweep across every worktree lane
just gc-deep       # additionally drop incremental state + the cpu-smoke lane
```

`sccache` trims itself to `SCCACHE_CACHE_SIZE`; the cache size only applies
when the server starts, so after changing it run `sccache --stop-server`
first.

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
- `.cache/hypercolor/target` for CI-selected Cargo target shards
- `.cache/hypercolor/mozbuild`
- `.cache/hypercolor/toolchain`
- `.cache/hypercolor/ccache`

The manual `.github/workflows/servo-cache-warm.yml` workflow warms three
compatible shapes when a maintainer deliberately refreshes Servo caches:

| Suite        | Shared Key     | Extra Key    | Purpose                                          |
| ------------ | -------------- | ------------ | ------------------------------------------------ |
| Core Servo   | `servo-core`   | empty        | core Servo check, test, and clippy artifacts     |
| Daemon Servo | `servo-daemon` | empty        | daemon Servo check, test, and clippy artifacts   |
| E2E Servo    | `servo-daemon` | `e2e-dev-v1` | daemon and CLI binaries for the normal E2E stack |

The main CI workflow reuses those same shared keys in the explicit Servo check,
test, and E2E build lanes. Pull requests keep the separate Servo check/test
lanes out of the default path and rely on the normal Servo E2E stack for HTML
renderer coverage. Pushes to `main`, tags, and manual CI dispatches still run
the full Servo check/test gates.

Shared non-Servo Rust lanes deliberately keep Servo out of their dependency
graph so routine crates do not rebuild `servo-script`.

## E2E Policy

CI builds and runs two E2E stacks:

- **Servo:** `just e2e-build`, default daemon features, real HTML effects, and
  `e2e/tests/servo.spec.mjs` telemetry proof.
- **CPU Smoke:** `just e2e-build-cpu`, builtin-driver daemon feature set, and a
  reduced proof that the non-Servo stack still boots.

The Servo lane is the PR integration gate. The CPU smoke lane remains a fallback
shape for builtin-driver coverage and release confidence.

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
