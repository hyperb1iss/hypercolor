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
  iteration-shaped recipes (`just test-crate`, `test-one`, the Unix `app`
  build, and the Windows `just dev` daemon build) pin incremental via
  `HYPERCOLOR_ITERATE=1`; the Windows `app` recipe uses `cargo run` and
  lands there by subcommand.
- Opt-outs: `HYPERCOLOR_NO_SCCACHE=1` disables sccache for the session;
  `HYPERCOLOR_ITERATE=1` does the same per invocation when you want
  incremental rebuilds in a tight edit loop; a pre-set non-zero
  `CARGO_INCREMENTAL` always wins. Alternating the same profile tree
  between the two modes rebuilds only workspace crates (~50s measured),
  never dependencies.
- `rust-lld` as the linker on `x86_64-pc-windows-msvc` for non-release
  builds (`HYPERCOLOR_NO_FAST_LINK=1` to opt out); release-like builds
  keep `link.exe` so shipped artifacts all come off the same linker
- `clang` + `ld.lld` for faster link steps on `x86_64-unknown-linux-gnu` when available
- C/C++ caching for `cc`- and CMake-driven native deps (mozangle/ANGLE,
  turbojpeg): `ccache` or `sccache` on Unix (both modes), `sccache` around
  `cl.exe` on Windows (sccache mode only)

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
churn; Cargo never garbage-collects them. Bound them with:

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

- `Swatinem/rust-cache` for Cargo and extra cache directories
- `CARGO_INCREMENTAL=0` (set workflow-wide)
- `.cache/hypercolor/target` for CI-selected Cargo target shards
- `.cache/hypercolor/mozbuild`
- `.cache/hypercolor/toolchain`
- `.cache/hypercolor/ccache`

Each lane passes its own `shared-key` and shards its target dir to match
(`shared-key: servo` builds into `.cache/hypercolor/target/servo`), so lanes
with incompatible feature shapes never share an entry.

**Only the default branch saves.** The action's `save-if` input defaults to
`auto`, which resolves to true only on `refs/heads/main`. The repo has a 10GB
Actions cache quota and GitHub evicts least-recently-used entries to stay under
it, so PR and tag runs that each saved their own copy would push the warm Servo
entry out and hand the next job a cold native build. PR and tag lanes restore
without competing; `save-if: "false"` opts a lane out of saving even on main.

CI does not run sccache today: hosted runners do not preinstall it and the
workflows do not set it up. The wrappers honor `HYPERCOLOR_FORCE_SCCACHE=1`
and a pre-set `CARGO_INCREMENTAL=0`, so a future CI sccache lane only needs
to install the binary and set the flag.

The manual `.github/workflows/servo-cache-warm.yml` workflow warms the
`servo` shared cache key when a maintainer deliberately refreshes Servo
caches; the main CI workflow reuses that key in its Servo check, test, and
E2E build lanes. Pull requests keep the separate Servo check/test
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

1. Check whether `servo-cache-warm.yml` is green on `main`. Dispatching the
   warmer on a feature branch restores but does not save, so only a `main` run
   populates the entry the PR lanes read.
2. Confirm the PR lane uses the same `shared-key`, `key`, and target directory
   shape as the warmer.
3. Confirm `Cargo.lock`, `rust-toolchain.toml`, and Servo feature sets did not
   change.
4. Inspect `Swatinem/rust-cache` restore logs for a key miss.
5. Check the repo's Actions cache list for eviction. The Servo entry is large
   and the quota is 10GB, so a burst of other saved entries can push it out
   even when every key is correct. A key that was fine yesterday and misses
   today with no input change is the signature.

If the pinned Servo version and toolchain are unchanged, warm builds should
avoid repeating the costly native compile.
