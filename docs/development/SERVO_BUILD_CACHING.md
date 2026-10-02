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

- an isolated target directory passed through Cargo's command line or config
  layer, never exported into rustc's cache key
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
  `mozbuild/` for SpiderMonkey build state. Clean target directories under
  the same checkout reuse Rust objects. C and C++ objects also reuse across
  checkout roots. Released sccache versions keep Rust artifacts sensitive to
  their checkout path, so a new worktree still compiles its Rust graph once.
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
just gc            # preview eligible profiles across every worktree
just gc-apply      # prune eligible profiles until pressure clears
just gc-reclaim    # clear pressure now, preserving dirty and locked profiles
just gc-install    # install and enable the daily user timer
just gc-status     # show the timer schedule and previous result
```

Collection starts only when aggregate Cargo profiles exceed 300 GiB. It
uses cargo-sweep under every available Cargo profile lock, then removes the
oldest whole profiles until usage reaches 240 GiB. The daily path preserves
profiles younger than 14 days plus every dirty or Cargo-locked worktree.
The explicit `just gc-reclaim` command shortens the age floor to 15 minutes,
which protects multi-command build workflows between Cargo invocations. Set
`HYPERCOLOR_GC_HIGH_WATER_BYTES`, `HYPERCOLOR_GC_LOW_WATER_BYTES`, and
`HYPERCOLOR_GC_MIN_AGE_DAYS` to tune the daily bounds. Set
`HYPERCOLOR_GC_RECLAIM_MIN_AGE_SECONDS` to tune the explicit reclaim grace.

The default dev profile keeps readable backtraces with line tables and trims
third-party symbols. Use `just debug-build` when a debugger needs full locals
and type information.

`sccache` trims itself to `SCCACHE_CACHE_SIZE`. The wrapper owns a dedicated
Hypercolor server and restarts it safely when its size or checkout map changes.

## Verify Cache Hits

```bash
ccache -s
just disk
```

Look for increasing cache hit counts after the first Servo build.

## CI Cache Topology

The reusable action `.github/actions/rust-build-cache` gives each Rust job two
cache layers:

- **GitHub Actions cache:** the Cargo registry and git checkouts, the
  CI-selected Cargo target shard under `.cache/hypercolor/target`, and any
  extra directories a lane declares (`.cache/hypercolor/mozbuild`, the macOS
  `ccache` directory). Each lane passes its own `shared-key` and shards its
  target dir to match (`shared-key: servo` builds into
  `.cache/hypercolor/target/servo`), so lanes with incompatible feature shapes
  never share an entry.
- **sccache:** the action installs the pinned sccache release, starts a server
  for the job, and prints its statistics in the job summary. On Linux and
  macOS the bash build wrapper restarts that server once, on the first build
  or test, to apply checkout path normalization.
  `CARGO_INCREMENTAL=0` is set workflow-wide because sccache refuses
  incremental compiles.

**Only the default branch writes.** The action's `save-if` input defaults to
`auto`, which resolves to true only for pushes, dispatches, and schedules on
`refs/heads/main`. Actions cache storage is bounded: GitHub evicts
least-recently-used entries, and once the repository reaches its configured
storage budget the cache turns read-only ("Cache reservation failed: You have
reached your configured budget"), so new keys stop saving at all. PR and tag
runs that each saved their own copy would push the warm Servo entry out and
hand the next job a cold native build. PR and tag lanes restore without
competing; `save-if: "false"` opts a lane out of saving even on main.

### Shared compiler cache in R2

sccache stores its entries in the Cloudflare R2 bucket named by the
`SCCACHE_R2_BUCKET` repository variable, outside the Actions cache budget.
Each of the thirteen jobs that use the cache action passes the bucket, the
`SCCACHE_R2_ENDPOINT` variable, and the `SCCACHE_R2_ACCESS_KEY_ID` and
`SCCACHE_R2_SECRET_ACCESS_KEY` secrets as the action's `r2-*` inputs. Jobs
that never compile see none of them. The action then:

- writes (`SCCACHE_S3_RW_MODE=READ_WRITE`) only where the Actions cache writes,
  and reads everywhere else, so pull requests and tags reuse what `main`
  compiled without adding entries;
- keys entries under `sccache/<os>-<arch>`, so each runner platform keeps its
  own namespace;
- sends a signed HEAD request before enabling R2. A refused or unreachable
  bucket logs a warning and leaves the job on the local disk cache, and so
  does a server restart whose storage check fails mid-job, because sccache
  otherwise refuses to start and would fail the build over an optional
  accelerator.

Fork pull requests receive no secrets and compile with the local disk cache.
The `Compiler cache:` line in the configure step's log and the
`Cache location` row of the job summary's statistics show which backend a job
used. The statistics are authoritative: a mid-job fallback, or a write check
that downgrades sccache to read-only, leaves the configure line stale, and the
first `main` run should show `Cache writes` above zero. A read-only run counts
every write it skips as a `Cache write errors` entry, so in pull requests and
tags that row matching `Cache misses` is expected, not a fault.

The bucket deletes objects 30 days after upload. After an entry expires, the
next `main` build that needs it compiles once and writes it again; pull request
and tag runs recompile without writing.

Two keys keep untrusted code out of the cache that release builds trust:

- The repository secrets hold a key with **object read only** on the bucket.
  Every job that compiles gets it, including pull requests, so code running
  in an unmerged branch (a dependency's build script, a test) can read the
  cache but never write to it.
- The `sccache-writer` environment holds the **read-write** key under the same
  secret names. Each cache job names that environment only when it runs on
  `refs/heads/main` (`deployment: false`, so no deployment records), and the
  environment's branch policy allows only `main`, so a branch that edits the
  workflow to name it is refused before the job starts.

Both keys reach only that one bucket. The proprietary repository uses its own
bucket and keys, so public CI can never write an entry a proprietary build
consumes. The 1Password items "Cloudflare R2 sccache:
hypercolor-oss-sccache-ci" and its `-lighting-` counterpart in the Hyperbliss
vault hold the credentials and the rotation steps.

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
4. Inspect the action's `Restore build artifacts and compiler cache` step for a
   key miss, and the `Compiler cache:` line of its configure step for whether
   the job reached R2.
5. Check the repo's Actions cache list for eviction, and the save step's log
   for "your cache is now read only". The Servo entry is large, so a burst of
   other saved entries can push it out even when every key is correct, and a
   cache at its storage budget saves nothing new. A key that was fine
   yesterday and misses today with no input change is the signature.

If the pinned Servo version and toolchain are unchanged, warm builds should
avoid repeating the costly native compile.
