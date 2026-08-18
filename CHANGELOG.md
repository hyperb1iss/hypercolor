# Changelog

All notable changes to Hypercolor will be documented here.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.2] - 2026-08-15

Global output power becomes a first-class daemon concept, bundled and saved presets merge into a single effect-scoped stack, and a trusted in-process API bridge lands for local callers. Persistence now reports durability explicitly, and the build system gains a cache-aware wrapper plus a pressure-triggered target GC.

### Added

- ✨ Add global output power endpoints `GET /api/v1/output/power` and `PUT /api/v1/output/power`, backed by `OutputPowerMode` (`running`, `paused`) and `OutputPowerStatus` (`running`, `paused`, `stopped`); pausing holds outputs at their off frame while **preserving live effect state** (978096e)
- ✨ Add a unified per-effect preset stack: `GET /api/v1/effects/{id}/presets` returns bundled and saved presets as `EffectPresetSummary` with `EffectPresetOrigin` and an `editable` flag, and `POST /api/v1/effects/{id}/presets/{preset_id}/apply` applies one with an optional `render_group` (3c52120, ac2a201)
- ✨ Add `TrustedLocalApi` and `TrustedLocalWebSocket` in `crates/hypercolor-daemon/src/api/local.rs`, letting in-process callers run daemon requests at `AccessTier::Control` without a network hop or API key; paths are validated against `/api/v1/*` and absolute URLs, authority headers, and traversal are rejected (e057ce3)
- ✨ Add stable identifiers for bundled presets via `PresetId::stable(name)`, plus `active_preset_modified` on `ActiveEffectResponse` and an optional `active_preset_id` in the WebSocket `hello` payload (ea42c20)
- ✨ Add Python client methods `get_output_power()`, `set_output_power(paused)`, `get_effect_presets(effect_id)`, and `apply_effect_preset(...)` on async and sync clients, with a new `OutputPowerState` model and regenerated `_generated` bindings (ac2a201, d642543, f471fa1)
- 👷 Add `scripts/cargo-target-gc.sh` with the `hypercolor-cargo-target-gc` systemd user service and timer for pressure-triggered target reclamation, dry-run by default and always preserving dirty worktrees (4b3c4ee)
- 👷 Add `scripts/cargo-cache-lock.rs` build-lock supervisor with signal forwarding and TTY handoff, plus `just` recipes `debug-build`, `build-wrapper-test`, `cargo-gc-test`, and the `gc*` family (64e14a6, 9d7bdb1)
- ✅ Add `crates/hypercolor-daemon/tests/trusted_local_api_tests.rs`, `crates/hypercolor-types/tests/output_tests.rs`, and shell test suites under `scripts/tests/` for the cache wrapper and target GC (e057ce3, aeeefc0)

### Changed

- 🔄 Guard session power transitions with a `transition_generation` counter so in-flight fades and reconnect scans abandon themselves when superseded, making replayed or late updates idempotent (978096e)
- 🔄 Make preset identity server-authoritative: the UI consumes the unified stack and `resolve_legacy_preset_id()` migrates older `bundled:*` ids (7e2d927, 9a57d8d)
- 🔄 Return `AtomicWriteCommitResult` from `persistence.rs` with `FailedBeforeReplacement`, `ReplacementVisibleButNotDurable`, `DurableWritten`, and `Superseded`, so a parent-directory fsync failure after a successful rename no longer collapses into success (05f98f6, 78b7df7)
- ♻️ Rework Cargo debug profiles: dev and preview dependencies drop to `debug = false`, workspace crates move to `line-tables-only`, and a new `debugging` profile (`inherits = "dev"`, `debug = "full"`) is the only way to get full dependency symbols; per-package `debug = 0` overrides were removed so the wildcard wins (36e8e30, 0ea4e63)
- ⚡️ Route Trunk and Cargo through a shim that injects `build.target-dir` instead of exporting `CARGO_TARGET_DIR`, keeping sccache hits stable across isolated targets, and normalize checkout paths with `SCCACHE_BASEDIRS` (64e14a6, 9d7bdb1, c8689c3)
- 🔧 Skip the `compat`, `sdk`, and `ui-test` matrices on docs-only merges and share one absolute target dir between the main build and the Tauri bundle step (3921d75, d1559f8)
- 📝 Rebuild the docs site on the app's brand assets and motion tokens, with class-based code themes (`github-light`, `one-dark-pro`), a margin-marker heading anchor template, and styled leads, pagination, cards, and task lists (455c3ed, 5a02ca5, 9ff82d1, 77bd8a7)
- 🔄 Bump SDK and E2E dependencies: `typescript` `^6.0.3` → `^7.0.2`, `@clack/prompts` `^1.6.0` → `^1.7.0`, `@biomejs/biome` `2.5.2` → `2.5.6`, `@playwright/test` `1.61.1` → `1.62.0`, `ws` `8.21.0` → `8.21.1` (5befbcc, 5566ac9)

### Fixed

- 🐛 Keep RAF-driven Servo capture clocks moving on quiet pages by setting `emit_frame_timing` and dispatching `LightScriptFrameUpdate::TimingScript` in `frame_queue.rs` (238f53f)
- 🐛 Wait for CPU reducer worker threads to initialize before probing screen-capture fanout allocation (df2b1da)
- 🐛 Accept media uploads above Axum's default multipart body limit and return a proper `413` via `multipart_error_response()`, sizing the route limit from the library's 2 GiB cap plus bounded framing overhead (1d66f43)
- 🐛 Acknowledge favorite and config sync updates only after persistence commits, return the exact installed snapshot, and keep deleted tombstones across favorite revisions (e75e33a, 69dd8d7)
- 🐛 Report an `OutputCadence` derived from target FPS with `with_max_frame_silence(KEEPALIVE_INTERVAL)` in the WLED backend (978096e)
- 🐛 Retry Windows delete-pending file locks in WLED tests (a9f8c94)
- 🐛 Fall back to a styled monogram in `vendors.rs` when a device vendor mark fails to load instead of rendering a broken image (43f23f9)
- 🐛 Reject duplicate bundled preset identities at load and normalize preset control whitespace without regex escape hazards in `normalizePresetKey()` (d26cbe8, d96e1fe, 25067d6)
- 🐛 Enforce a reclaim grace period before artifact sweeps and avoid oversized Cargo GC fallback deletions (54e07bd, 4317a44, 2b71a61)
- 🐛 Make docs search work: load the Elasticlunr library and index separately, recover from transient index failures, derive section labels from the URL path, and restore focus on close (f253b40, 2079da7)
- 🐛 Give mobile a working navigation surface via the `nav-hamburger` toggle and stop wide tables and the nav from overflowing (cf77e7d, 241d2f1)
- 🐛 Isolate portable lock helper compilation and place the nextest target directory after its subcommand (74ef48b, cc7b9e0)

### Removed

- 🔥 Remove client-side preset matching (`crates/hypercolor-ui/src/components/preset_matching.rs` and its tests); preset identity now comes from the daemon (7e2d927)

### Breaking Changes

- **Pause is no longer a stop.** `POST /api/v1/effects/pause` preserves live state and holds outputs at the off frame. Clients relying on pause tearing down the active effect must call the stop path explicitly or read `OutputPowerStatus::Stopped`
- **Pause/resume response shapes changed** to explicit `PauseEffectResponse` and `ResumeEffectResponse`; pause adds `off_output_behavior` and `off_output_color`. `/api/v1/effects/pause` and `/api/v1/effects/resume` remain as compatibility endpoints, with `/api/v1/output/power` as the canonical surface
- **Python `pause_rendering()` and `resume_rendering()` return `OutputPowerState`** instead of `MutationResult`
- **Tray menu ids split**: `PAUSE_RESUME` became `PAUSE_OUTPUT` and `RESUME_OUTPUT`, and the command changed from `TogglePause` to `SetPaused(bool)`
- **`PresetTemplate` gains a required `id` field.** Rust consumers must supply an id (`hypercolor_types::library::PresetId::stable(name)`); the generated Python `preset_template` model changed shape
- **Duplicate bundled preset identities are rejected.** `crates/hypercolor-core/src/effect/loader.rs` raises `duplicate bundled preset id: ...` and `install_effect()` wraps it in `ApiError::internal(...)`, so `POST /api/v1/effects/install` returns `500`

Upgrade notes:

- Point pause/resume integrations at `PUT /api/v1/output/power` with `{"state": "paused"}` or `{"state": "running"}` and read status from `GET /api/v1/output/power`
- Regenerate OpenAPI-derived clients: the schema adds the `output` tag, effect preset models, and `active_preset_modified`
- Migrate stored `bundled:*` preset references to the stable identifiers returned by `GET /api/v1/effects/{id}/presets`
- Give each HTML effect `preset` a `preset-id` and verify ids stay unique after whitespace normalization; `validate` flags collisions before upload
- Handle `ReplacementVisibleButNotDurable` separately from `DurableWritten` when inspecting atomic write results
- On Linux, run `just gc-install` for the daily target GC timer (dry-run by default; `just gc-apply` to reclaim)
- Build with `cargo build --profile debugging` when full dependency debug symbols are needed; dev and preview builds no longer carry them

### Metrics

- Total Commits: 77
- Files Changed: 257
- Insertions: +13,490
- Deletions: -1,935
<!-- -------------------------------------------------------------- -->

## [0.3.2] - 2026-08-15

This release unifies bundled and saved presets into a single effect-scoped stack, adds a non-destructive global output power contract that preserves live effect state across pause, and introduces a trusted in-process API bridge for daemon extensions. Build caching and the documentation site both got substantial repair work.

### Added

- Add **effect-scoped preset stack**: `GET /api/v1/effects/{id}/presets` and `POST /api/v1/effects/{id}/presets/{preset_id}/apply` return and apply bundled and saved presets through one surface, backed by new `EffectPresetSummary`, `EffectPresetOrigin`, `EffectPresetListResponse` and `ApplyEffectPresetRequest` types (3c52120)
- Add **stable identifiers for bundled presets**: authored `id` attributes survive HTML metadata parsing and older effects get deterministic UUIDs via `PresetId::stable`, exposed through shared Rust types, TUI decoding and SDK tooling (ea42c20)
- Add **global output power endpoints** `GET`/`PUT /api/v1/output/power` with `OutputPowerMode` (`Running`, `Paused`) and `OutputPowerStatus` (`Running`, `Paused`, `Stopped`); pausing freezes the render loop and holds outputs at their off frame without discarding effect state (978096e)
- Add **trusted local execution bridge** (`crates/hypercolor-daemon/src/api/local.rs`): `TrustedLocalApi::execute` and `open_websocket` let already-authenticated in-process extensions run the same handlers as a local client, confined to `/api/v1/*` and `/api/v1/ws` (e057ce3)
- Add Python client methods `get_effect_presets()`, `apply_effect_preset()`, `get_output_power()`, `set_output_power()`, `pause_rendering()` and `resume_rendering()` on both the async and sync clients (ac2a201, d642543)
- Add **Cargo target garbage collection**: `scripts/cargo-target-gc.sh` with pressure hysteresis, age gating and lock-aware coordination, plus `hypercolor-cargo-target-gc.service`/`.timer` and `scripts/install-cargo-target-gc.sh` (4b3c4ee)
- Add `scripts/cargo-cache-lock.rs`, a compiled cross-platform build lock helper with process-group and signal handling (c8689c3)
- Add `scripts/tests/cargo-target-gc-tests.sh` and `scripts/tests/cargo-cache-build-tests.sh`, wired into the `verify` recipe on Unix

### Changed

- Route the UI's preset panel through the unified stack: `fetch_effect_presets` and `apply_effect_preset` in `crates/hypercolor-ui/src/api/effects.rs` replace the old bundled-preset fetch plus the separate library apply call (7e2d927)
- Point UI pause and resume at `PUT /api/v1/output/power` (`{"state": "paused"}` / `{"state": "running"}`) instead of the effect pause routes
- Add `active_preset_id` to the WebSocket `hello` state payload and `active_preset_modified` to `ActiveEffectResponse`, so clients can tell a clean preset from an edited one
- Rebrand the docs site on the app's assets and motion tokens: trinity mark favicon, touch icon and og:image, and all 67 motion declarations moved to the canonical `--ease-silk` / `--duration-normal` tokens (455c3ed)
- Theme docs code blocks with dual `github-light` / `one-dark-pro` stylesheets that swap with the site theme instead of hardcoded inline One Dark (5a02ca5)
- Replace Zola's default heading anchor emoji with a styled `#` gutter marker revealed on hover and focus, hidden below the 768px breakpoint (9ff82d1)
- Route Trunk and Tauri compiles through `scripts/cargo-cache-build.sh` with a shared target root and persistent per-repo sccache server, so isolated targets keep cache hits (64e14a6, 9d7bdb1, 71c62d4)
- Anchor the native-app CI job to `${{ github.workspace }}/target` so the Tauri bundle step reuses the build step's artifacts (d1559f8)
- Skip UI, SDK and compatibility matrix rebuilds on docs-only merges (3921d75)
- Split the Cargo profile set so dependencies drop debug info while Hypercolor crates keep line tables, with an explicit `debugging` profile for full symbols (36e8e30, 0ea4e63)
- Bump the `sdk-bun` dependency group (4 updates) and the `e2e-npm` group (2 updates) via Dependabot (#130, #131)

### Fixed

- Make native sync materialization durable: favorite and config updates acknowledge only after persistence commits, the exact installed snapshot is returned, and deleted tombstones survive favorite revisions (05f98f6, 78b7df7)
- Sync the parent directory for relative config paths after atomic replacement and retain predecessor state for uncertain favorite replays (e75e33a)
- Accept media uploads above Axum's 2 MiB default multipart limit by sizing the route limit from the library's 2 GiB hard cap plus bounded framing overhead (1d66f43)
- Keep RAF-driven Servo capture clocks moving by publishing compact frame timing for every admitted frame, so quiet capture pages still advance daemon time (238f53f)
- Reject duplicate bundled preset identities across SDK validation, daemon upload, HTML loading and the native catalog so effect-scoped resolution stays unambiguous (d26cbe8)
- Normalize preset control whitespace by treating U+001C–U+001F as separators before collapsing, without regex escapes (d96e1fe, 25067d6)
- Restore device vendor marks by sizing embedded SVG roots to their container and falling back to a vendor monogram when a raster mark fails (43f23f9)
- Retry Windows delete-pending lock failures in WLED tests instead of treating them as hard permission errors (a9f8c94)
- Make docs site search work: bind the search button, drive modal visibility from the `hidden` attribute, fix overlay positioning and CSS class mismatches, and actually load `elasticlunr.min.js` (f253b40)
- Bind the mobile hamburger to `nav-hamburger`, add top-level section links to the drawer, and extend it to the home page (cf77e7d)
- Stop wide tables and the nav bar from overflowing the page at 1280px (241d2f1)
- Render the Agents/MCP mermaid diagram and give all 118 docs images intrinsic width and height to prevent lazy-load reflow (31f9484)
- Enforce the reclaim grace period before artifact sweeps and avoid oversized Cargo GC fallback deletions (54e07bd, 4317a44, 2b71a61)
- Place the nextest target directory after its subcommand in the build wrapper (cc7b9e0)
- Satisfy Rust 1.95 daemon lints (33782c0)

### Removed

- Remove `crates/hypercolor-ui/src/components/preset_matching.rs` and its test module; preset provenance now comes from `EffectPresetOrigin` on the server response instead of client-side heuristic matching
- Remove the UI's local `apply_preset` wrapper from `crates/hypercolor-ui/src/api/library.rs` and `fetch_bundled_presets` from `crates/hypercolor-ui/src/api/effects.rs`

### Breaking Changes

- **Effect-scoped preset routes are the new canonical path.** `GET /api/v1/effects/{id}/presets` and `POST /api/v1/effects/{id}/presets/{preset_id}/apply` are additive; the existing REST surface is still served at HEAD, including `GET`/`POST /api/v1/library/presets`, `GET`/`PUT`/`DELETE /api/v1/library/presets/{id}`, `POST /api/v1/library/presets/{id}/apply`, and `POST /api/v1/effects/pause`|`resume` (the latter two retained as compatibility responses). No REST caller breaks.
  - The actual break is client-side and internal to this repo: the UI dropped its `library::apply_preset` wrapper and `effects::fetch_bundled_presets`, and now calls the effect-scoped routes. Any fork carrying UI patches against those functions must migrate.
  - Upgrade note: new integrations should use the effect-scoped routes. Python SDK users get `get_effect_presets()` and `apply_effect_preset()` on both `HypercolorClient` and `SyncHypercolorClient`; the older `get_presets()` / `apply_preset()` methods remain.
- **`PresetTemplate` gains a required `id` field.** Rust consumers constructing `PresetTemplate` literals must supply an id (use `hypercolor_types::library::PresetId::stable(name)` for the derived value), and the generated Python `preset_template` model changed shape. Regenerate SDK clients against the updated OpenAPI document.
- **WebSocket `hello` payload gains `active_preset_id`.** `HelloState` in `crates/hypercolor-daemon/src/api/ws/protocol.rs` adds an optional `active_preset_id` string; clients that reject unknown fields on the hello frame need updating. Note that `StateUpdate::Snapshot` is a tray-app internal enum variant in `crates/hypercolor-app/src/state.rs` used to rebuild local state, not a field on the wire.

### Metrics

- Total Commits: 71
- Files Changed: 252
- Insertions: +13,310
- Deletions: -1,917
<!-- -------------------------------------------------------------- -->

## [0.3.1] - 2026-08-11

A cross-platform input and capture release. Host keyboard and mouse capture lands behind an explicit consent gate on Linux (evdev) and Windows (Raw Input), Windows gains a Desktop Duplication screen-capture crate, and the screen pipeline is rebuilt around exact publication plans with transactional capacity admission. The web UI collapses to Studio-only navigation with a mobile-responsive shell, and daemon persistence becomes transactional and durable on Windows.

### Added

- Add the **interactive input pipeline** (spec 71): typed input data model, host `[input]` consent gate, WebSocket privacy rules, evdev host capture, and shipped udev uaccess rules (`0a6cbf1a`, `210563bb`, `29704a3e`)
- Add `hypercolor-windows-input`, a Raw Input interop crate, plus a Windows host input source and one shared key inventory wired into the daemon with honest degradation reporting (`ab88979f`, `e7f4524e`, `410ee5a7`)
- Add `hypercolor-windows-capture`, a Desktop Duplication capture crate with D3D11 GPU reduction, descriptor-keyed readback, and wgpu surface bridging (`43c4d38a`, `3522852e`, `f5b7710a`)
- Add monitor enumeration over the API at `GET /api/v1/capture/monitors`, with capture settings reduced to on, monitor, and a disclosure (`b3e900d3`, `eebc6a6d`)
- Add SDK input support: `sdk/packages/core/src/input` module, an input-reactive effect capability, and the `Keystrike` interactive showcase effect (`b159f32f`, `613a9a1d`)
- Add browser-preview input injection: a focused preview canvas drives interactive effects over the `input_inject` WebSocket message with no host capture permission, though the handler still requires a **control-tier WebSocket authorization** (`245e987a`, `32fad660`)
- Add inline cover artwork for 40 bundled effects, embedded at build time at 960px and served by the daemon (`b4bbd657`, `16e6671b`, `25e40b83`, `e0bae710`)
- Add **portable device identity**: typed identity claims on every discovery carrier, portable key pins persisted in `device-aliases.json`, and orphaned layout bindings re-bound over `GET /api/v1/devices/bindings` and `POST /api/v1/devices/rebind` (`3dd54801`, `c89f41c4`, `a446fb50`)
- Add a durable driver inventory store persisted independently of runtime state (`9f88ab01`, `e0385736`)
- Add `hypercolor-platform-fs` for atomic, durable state-file replacement on Windows (`1033f4a1`, `946df5f6`)
- Add `screen_capture_capacity` to the `SystemStatus` payload on `GET /api/v1/status`, reporting physical reservations, publication budgets, analysis extent, and worker capacity; `admission_enforced` is `false` where no capacity plan is installed (`732c6de7`, `fc502bf4`)
- Add a mobile-responsive web UI (spec 75): bottom-nav shell, mobile-first headers and effect/media views, compact phone dashboards, and mobile-only nav destinations (`a4939c07`, `048eed1b`, `eb94ef19`, `334bde9b`)
- Add input health surfaces to the UI: access banner, source-health and routing panels, and a dedicated input settings section (`bb8d89c5`, `02ceec5c`)
- Add wide and chunked preview wire frames with a byte-bounded latest queue and connection-scoped interactive preview lanes (`6497af66`, `118af090`, `aa6c2703`, `648eb04a`)

### Changed

- Promote **Studio** to the only navigation entry and drop the legacy page flag (`d753a927`)
- Rebuild screen capture around exact publication plans, ticket-scoped ledgers, and typed demand, with CPU and GPU reducers that handle arbitrary source resolutions (`78b5ff3a`, `104390d9`, `e3d603a7`)
- Remove arbitrary canvas, preview, and capture cadence ceilings across layout, renderer, simulator, and config (`9ac3e8f9`, `1f8084a9`, `56b208bb`)
- Make daemon persistence transactional: layouts, scenes, profiles, display preferences, and runtime state now reserve, commit, and roll back instead of best-effort writing (`1d4e2860`, `81007d9a`, `4f6735a7`)
- Normalize the device-settings key space to **schema v2**, with quarantine and rebind inheritance honored (`3198c353`, `c7e37cbf`)
- Rename `windows_admission_enforced` to `admission_enforced` and return the capacity block on all platforms instead of gating it to Windows (`732c6de7`)
- Add scalable GPU area and summed-area-table sampling, pooled CPU screen uploads, and incremental CPU layer transforms in the renderer (`c3b555db`, `987522d2`, `b4982b4b`, `fb8ea562`)
- Upgrade the Servo embedder stack to 0.4.0 (`ba596b55`)
- Default the Windows renderer GPU to DX12 and use the native Windows HID backend (`d4596591`, `a29252ba`)
- Promote saved presets into the effect catalog (`24d92038`)
- Rework settings to read as a product rather than a config editor, consolidate dashboard telemetry into one Performance strip, and compact the sidebar chrome (`f3b6e90d`, `ff4fb732`, `5b9724e0`, `0390a43d`)
- Rewrite the guides, API references, and hardware pages against the current tree, and refresh all UI and TUI screenshots (`c06e1232`, `4be3cf82`, `e447e712`)
- Split the release build so Servo LTO stays out of the rest of the workspace, and share sccache across worktrees (`d56fb9ba`, `5610261e`)

### Fixed

- Budget the Hue connect deadline for the whole bridge handshake instead of a single request (`956eddf5`, `f7de8678`)
- Pace Push2 MIDI writes and bound palette sysex to stop a firmware wedge (`364b6633`)
- Fall back to cpal when PulseAudio cannot answer, and make audio reconfiguration real-time safe (`0c52e158`, `d09a9f3f`)
- Send ENE delays in-batch over SMBus and stop forcing PawnIO SMBus polls onto the kernel sleep timer (`6c8fe8bd`, `b6b433e2`)
- Register the SMBus broker during Windows install and re-probe the Windows CPU temperature reader (`d89177dd`, `bac7d609`)
- Connect devices placed only through a scene zone, and resync connectivity on scene activation off the hot path (`2607914a`, `20a63869`)
- Stop letterbox detection from eating the whole frame and keep the screen downscale at the source aspect ratio (`93bf8a0f`, `05c1251e`)
- Stop raising frame-encode threads to render priority (`8923ecdc`)
- Summarize WebSocket slow-consumer drops instead of logging each one, and stop exact-plan retry warnings from flooding the log (`87e37ca7`, `12907ee2`)
- Preserve unknown top-level config sections across saves (`b0114f01`)
- Replace state files atomically and durably on Windows, honoring filesystem identity and rejecting superseded snapshots (`1033f4a1`, `dd6aa1af`, `86f4234b`)
- Route bundled presets to the zone instead of a dead endpoint and resolve card artwork by effect id rather than a guessed slug (`5d2de3f5`, `932a17f5`)
- Prune default faces, preferences, and learned discovery targets when a device is deleted (`1674dc13`, `18a2576f`)
- Stop presenting the Apache license as an installer EULA and ad-hoc sign the macOS app bundle instead of building unsigned (`31e14a5d`, `a186e364`)
- Move input uaccess rules before `systemd` seat-late so device permissions apply (`ceb18a8e`)
- Stream Nanoleaf external control to an injectable UDP port (`660f0653`)

### Security

- Gate host input capture behind explicit user consent, with capture health and remediation surfaced so a denied device is visible rather than silently missing (`0a6cbf1a`, `1dd7507c`)
- Bound Windows Raw Input payload reads to the record rather than the buffer, and bound the record walk to the union arm actually read (`40175f83`, `ba5e963f`)
- Close Raw Input lifecycle races and stop delivering every batch to core twice (`5040d66b`, `cef4cd05`)
- Never capture screen content for screen-mirroring effects when generating cover artwork (`003a5794`)
- Route conduct and security reports through GitHub's private reporting flow (`6c047eb2`)

### Removed

- Remove the legacy Displays, Assets, and Layout pages along with their feature flag; Studio is the only workspace (`d753a927`)
- Remove the simulator UI journey E2E suite and `crates/hypercolor-app/src/resources.rs` with its tests (`d7fc98a9`, `d753a927`)
- Remove the legacy SDK canvas resolution warning (`3eebfcd9`)

### Breaking Changes

- **`SystemStatus.screen_capture_capacity.windows_admission_enforced` is now `admission_enforced`.** Update any client reading the old key; the field is present on every platform and reports `false` when no capacity plan is installed.
- **Device settings keys are normalized to schema v2.** Existing entries are migrated on load, but external tooling that wrote raw `device-settings.json` keys must be updated to the v2 key space.
- **Legacy UI routes are gone.** Bookmarks or automation pointing at the Displays, Assets, or Layout pages should target Studio instead.
- **Preview WebSocket transport is negotiated (v2).** Clients that decode preview frames must handle wide and chunked frames; regenerate the Python and TypeScript clients from the checked-in protocol before upgrading.

### Metrics

- Total Commits: 527
- Files Changed: 819
- Insertions: +188,678
- Deletions: -15,247
<!-- -------------------------------------------------------------- -->

## [Unreleased]

### Changed

- `getInputData()` now reports input declaration, routing, health, freshness,
  and degradation independently. Recent keyboard or mouse activity no longer
  implies source availability.

### Deprecated

- `InputData.available` now means `routed && healthy` and remains as a
  compatibility alias through SDK 0.3.x. Read the explicit lifecycle fields
  instead; the alias will be removed in SDK 0.4.0.

## [0.2.1] - 2026-07-15

First public release of Hypercolor, a cross-platform RGB LED orchestration daemon with a GPU-accelerated render pipeline, multi-vendor hardware support, and a full effect authoring SDK.

### Added

- ✨ Scaffold the entire **Cargo workspace** with 25+ crates: `hypercolor-core`, `hypercolor-daemon`, `hypercolor-types`, `hypercolor-hal`, `hypercolor-cli`, `hypercolor-tui`, `hypercolor-ui`, `hypercolor-app`, `hypercolor-tray`, `hypercolor-driver-api`, and platform GPU interop crates (dde4391, 89dbf22)
- ✨ Implement the **Sparkleflinger render pipeline** with CPU and GPU (wgpu) compositor backends, scene transition crossfades, render group isolation, deferred GPU zone sampling, and admission-controlled frame pacing (06eba59, abbc6b9, c5f09097)
- ✨ Add **multi-zone scene system** with per-zone effect assignment, layer stacks, blend modes, media layers (image, animated WebP, Lottie, video, stream URLs), and snapshot mutation guards (0c8d7ae, 59587d4, afc80d7)
- ✨ Add **Servo (embedded browser) HTML effect renderer** with LightScript runtime, GPU import pipelines for Linux (Vulkan/GL), macOS (IOSurface), and Windows (ANGLE/D3D11), multi-session support, and circuit breaker fault isolation (001cea9, 4883d3e, c7f0603, 548fd71)
- ✨ Ship **33+ SDK effects** across canvas, WebGL, and GLSL renderers with the `@hypercolor/create-hypercolor` scaffolding CLI, declarative `effect()` and `canvas()` APIs, preset templates, control groups, and curated cosine palettes (011c94c, 6476e42, 670c548)
- ✨ Add **7 display faces** (Neon Clock, Pulse Temp, Sensor Grid, SilkCircuit HUD, Now Playing, Spectrum, System Pulse) with the Face SDK, descriptor-aware layouts, hermetic vendored fonts, and atmosphere effects (c1600c2, e92355, 583ee5c)
- ✨ Implement **hardware drivers** for Razer (USB HID, scroll wheel, Seiren V3, Blade laptops), Corsair (Lighting Node, iCUE LINK, LCD, Bragi peripherals), ASUS Aura (USB + SMBus/DRAM), Lian Li Uni Hub (ENE/TL/legacy), Dygma (Focus serial), PrismRGB, QMK (OpenRGB protocol), Ableton Push 2 (MIDI + display), ROLI Blocks, and Nollie (Gen1/Gen2/NOS2/Stream65/Legacy) (57c294d, a17c350, 327783e, 0564b64, 935e73d)
- ✨ Add **network device drivers** as isolated crates: WLED (DDP/E131, RGBW, fuzzy dedup), Philips Hue (DTLS entertainment streaming, bridge pairing), Nanoleaf (UDP streaming, topology refresh), Govee (LAN multicast, cloud v1 API key pairing, rate limiting), and OpenRGB fallback bridge (fdedbfc, d13bcb5, faa7ed7, c1917512, 4c36331)
- ✨ Add the **unified driver module API** (`hypercolor-driver-api`) with extensible config registry, dynamic control surfaces, typed actions with confirmation prompts, device pairing flows, presentation metadata, and protocol catalog capabilities (45edd5c, 3eea2da, caa9f74)
- ✨ Build the **Leptos 0.8 CSR web UI** with Luminary (SilkCircuit) design system, Studio composition page with multi-zone tree, layout editor with undo/redo, display face management, device pairing modal, effect controls, preset library, viewport designer, media gallery, WebGL/WebSocket preview, and WebSocket auto-reconnect with exponential backoff (5d2f5b5, c6f2c2b, 996bcec, 901bc70)
- ✨ Add the **Ratatui TUI** with 60fps rendering, Kitty/Sixel/halfblocks live preview, motion effects (border pulse, ambient bleed, breathing, crossfade), HSL color picker, spectrum-reactive borders, resizable split panels, mouse interaction, and multi-zone/scene support (a21226, c023ee0, 2401f09)
- ✨ Add the **hypercolor CLI** with SilkCircuit-themed help, connection profiles, `hyper status` visual output, dynamic driver/device control commands, service management, completions, and TUI as a subcommand (8959a20, 75c61d8, 709deb5, 2b04338)
- ✨ Add the **Tauri desktop app** (`hypercolor-app`) with supervised daemon lifecycle, system tray with brightness presets and scene status, rolling file logging, first-run welcome overlay, pause on window hide, and native installers for Linux/macOS/Windows (7e39e5e, c28cfaa, 69e2628)
- ✨ Add **Windows platform support** with PawnIO SMBus transport and broker service, per-machine NSIS installer with hardware setup, Windows service mode, ANGLE GPU import, ACPI/NVML sensors, sleep/resume rediscovery, and elevated helper for SMBus repair (cbf226a, 65c685, 69e2628, 5310d86)
- ✨ Add the **Python client** (`hypercolor` on PyPI) with async/sync clients generated from OpenAPI, WebSocket protocol helpers, scene/zone surface with If-Match concurrency, and Home Assistant integration helpers (d2b7b06, 47a8b8d, 3007f1f)
- ✨ Add **audio reactive pipeline** with PulseAudio native capture, FFT spectrum analysis, beat detection, transient gating, motion-driven smoothing, and live input switching (0936bca, c2b2168, bf9ed0f)
- ✨ Add **Wayland screen capture** (PipeWire portal), live crop editor, ambilight edge-projection effect, and color tuning pipeline (d625b55, 0d1fdde, 85c9e31)
- ✨ Add **asset library** for user media with drag-and-drop upload, Lottie/WebP/video/stream URL support, and scene media admission caps (a8228d1, d1faf87, 4281b6f)
- ✨ Add **mDNS network discovery**, multi-server support, and per-device brightness control with direct-control locks (b3bcb43, 5ad58de)
- ✨ Add **session and power awareness** via systemd-logind, screensaver monitoring, configurable off-output behavior, and Windows sleep/resume (696b15e, e45f3d3, 4eed5e4)
- ✨ Add **MCP server** (Model Context Protocol) with tool handlers for effects, devices, scenes, displays, and structured output over HTTP (bde7c5e, b937e38, 6598a9b)
- ✨ Add **REST API** with OpenAPI spec generation, auth tiers, rate limiting, CORS, access log middleware, WebSocket binary frame channels, and JPEG preview endpoints (26fc6a5, 1458aaf, 59914f9)
- ✨ Add **CI/CD pipeline** with Rust/Servo/WASM/e2e lanes, Playwright harness, Criterion benchmarks, GitHub Actions release workflow with `.deb`/AUR/Homebrew/NSIS artifacts, and trusted npm publishing (adf39a9, a3b289d, 5cbc1b2)
- ✨ Add **documentation site** (Zola) with Luminary theme, 70+ spec documents, effect authoring guides, hardware compatibility database, and public roadmap (49ed277, b638191, 854ec5d)

### Changed

- 🔄 Switch color pipeline to **linear-light interpolation** with sRGB encode/decode, precomputed LUTs, and Oklch gradient blending (5ea5167, c688c37)
- 🔄 Raise default canvas to **640x480** with live FPS retune and adaptive SDK API (a0ecd22)
- 🔄 Replace the legacy `EffectEngine` with **scene-backed render groups** as the single rendering path (4cde65a, 9b8d221)
- 🔄 Migrate all HAL protocol encoders to **zerocopy typed structs** (Razer, Corsair, ASUS, PrismRGB, Blocks) for zero-copy frame encoding (9f61802, 98b556c, 2525f10)
- 🔄 Rename CLI binary to `hypercolor` and daemon to `hypercolor-daemon` (a7e25a5, 2b04338)
- 🔄 Rename SDK npm packages to `hypercolor` and `create-hypercolor` (ea73f10)

### Fixed

- 🐛 Preserve color saturation on RGBW WLED devices by sending RGB-only DDP frames (178a22c, 51e9c57)
- 🐛 Deduplicate devices by scanner fingerprint across rescans (1fdb0bf)
- 🐛 Cap reconnect retries and harden lifecycle wiring to prevent runaway loops (c08afef)
- 🐛 Fix memory leaks in Servo worker lifecycle, bound WS queues, and manage webview cleanup (585689, 701388f)
- 🐛 Stabilize frame pacing with admission-controlled cadence and paced outputs (ccd4321, 30780f0)
- 🐛 Fix reactive flickering in the web UI with Memo gates and signal identity fixes (13144c2)
- 🐛 Prevent canvas flicker from reactive DOM rebuilds (13ab279)
- 🐛 Preserve animation clocks across long uptime with monotonic daemon clocks (d3248d9, 84ffeb9)
- 🐛 Fix USB reconnect stalls and isolate USB device output actors (09b5466, 74392ab)
- 🐛 Harden WLED connection stability, protocol reliability, and endpoint metadata surfacing (1060ffb, 326109c)

### Security

- 🔒 Require auth for network daemon binds and make CORS config auth-bound (d02b4e5, 11f6aab)
- 🔒 Restrict credential file modes on disk (b05ae75)
- 🔒 Enforce media admission caps for MCP scene activation and validate broadcast targets (748e386, 71c051a)
- 🔒 Harden stream URL SSRF validation including IPv6/unspecified bypass (052a238, 804935f)
- 🔒 Validate WebSocket origin before upgrade and cap WS command body sizes (0a7b47b, 732cfe0)
- 🔒 Block cross-site loopback write requests and enforce control auth for preview writes (11423f1, 36372ef)
- 🔒 Bound animated media decode to prevent OOM (592867d, 18e983f)
- 🔒 Reject per-user PawnIO module directories for Windows services and secure service install paths (18df538, 4a8b24b, 9e4341a)
- 🔒 Pin CI actions to immutable commit SHAs (350d47a, b676e31)
- 🔒 Add unified network access modes with loopback-only defaults (ada10c9, e2812223)

### Removed

- 🔥 Remove OpenRGB direct backend in favor of the clean-room OpenRGB SDK bridge driver (0ecdee6, 4c36331)
- 🔥 Remove legacy `EffectEngine`, compatibility aliases, and stale migration paths (4cde65a, 91b0de1, 2c79e01)
- 🔥 Remove the standalone `hypercolor-desktop` crate, superseded by `hypercolor-app` (5af371a)
- 🔥 Remove the marketing website from the repository (83ac651)
- 🔥 Remove display overlay compositor, subsumed into display face composition blending (9c33e0d, 16e8222)

### Metrics

- Total Commits: 1,308
- Files Changed: 2,591
- Insertions: +720,254
- Deletions: -2,397
