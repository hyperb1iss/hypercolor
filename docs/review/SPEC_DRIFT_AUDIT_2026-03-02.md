# Hypercolor Spec Drift Audit (2026-03-02)

## Context

This document captures implementation-vs-spec differences identified during a multi-agent deep review of:

- `crates/hypercolor-core`
- `crates/hypercolor-daemon`
- `crates/hypercolor-cli`
- `docs/specs/*`

Scope note:

- USB/HID scope (`docs/specs/04-usb-hid-backend.md`) is intentionally deferred for now.
- OpenRGB is currently the delegated path for broad device support.

## Executive Summary

The codebase is strong on unit/integration coverage and internal module structure, but there is substantial spec drift concentrated in control-plane seams:

1. CLI <-> daemon API contracts (response shape, endpoints, IDs, command behavior)
2. Daemon protocol surface (WebSocket semantics, MCP runtime wiring, auth/rate-limit policy)
3. Core architecture parity gaps (device identity stability, output pipeline behavior, render contract)

The most impactful runtime issues were in CLI/API compatibility and effect activation flow.

## Confirmed Quality Gate State at Audit Time

- `cargo test --workspace`: passing
- `cargo clippy --workspace -- -D warnings`: failing at audit time (fixed in commit `84e3d8e`)

## Drift Details

## 1) CLI <-> API Contract Drift

### 1.1 Envelope shape mismatch

Spec and daemon handlers return envelope payloads (`{ data, meta }`), but several CLI handlers parsed top-level arrays/fields.

Examples:

- daemon envelope type: `crates/hypercolor-daemon/src/api/envelope.rs`
- CLI array parsing:
  - `crates/hypercolor-cli/src/commands/devices.rs`
  - `crates/hypercolor-cli/src/commands/effects.rs`
  - `crates/hypercolor-cli/src/commands/scenes.rs`
  - `crates/hypercolor-cli/src/commands/profiles.rs`
  - `crates/hypercolor-cli/src/commands/layouts.rs`

### 1.2 Status payload mismatch

`status` command expected fields like `daemon.status`, `engine.fps`, and inline `devices`, while daemon `/api/v1/status` returns a different shape (`running`, `render_loop`, counts, etc.) under `data`.

Files:

- `crates/hypercolor-cli/src/commands/status.rs`
- `crates/hypercolor-daemon/src/api/system.rs`

### 1.3 Missing daemon endpoints consumed by CLI

CLI called endpoints that daemon router did not expose:

- `/api/v1/config*`
- `/api/v1/diagnose`

Files:

- CLI callers:
  - `crates/hypercolor-cli/src/commands/config.rs`
  - `crates/hypercolor-cli/src/commands/diagnose.rs`
- Router map:
  - `crates/hypercolor-daemon/src/api/mod.rs`

### 1.4 Effect activation ID/name mismatch

CLI accepted effect name/slug and passed it in path, while daemon expected UUID parse for `/effects/{id}/apply`.

Files:

- `crates/hypercolor-cli/src/commands/effects.rs`
- `crates/hypercolor-daemon/src/api/effects.rs`

### 1.5 Stop/active endpoint naming drift

Implementation uses `GET /effects/active` and `POST /effects/stop`; specs include different naming in places (for example `effects/current` and state-oriented variants).

Files:

- `crates/hypercolor-daemon/src/api/mod.rs`
- `docs/specs/10-rest-websocket-api.md`

### 1.6 Query/path encoding robustness

CLI used minimal string replacement for path encoding and unescaped query assembly, risking breakage with special characters.

Files:

- `crates/hypercolor-cli/src/output.rs`
- various command modules constructing query strings manually

### 1.7 Quiet-mode destructive behavior

Several destructive commands effectively allowed quiet mode to bypass confirmation intent.

Files:

- `crates/hypercolor-cli/src/commands/profiles.rs`
- `crates/hypercolor-cli/src/commands/scenes.rs`
- `crates/hypercolor-cli/src/commands/config.rs`

## 2) Daemon API and Runtime Drift

### 2.1 Startup bind precedence vs loaded config

Daemon process bound using CLI `--bind` default path directly while config-level bind settings existed separately, producing precedence ambiguity.

Files:

- `crates/hypercolor-daemon/src/main.rs`
- `crates/hypercolor-daemon/src/startup.rs`
- `docs/specs/12-configuration.md`

### 2.2 System uptime origin inconsistency

API status uptime was derived from `AppState` creation time rather than daemon lifecycle start time.

Files:

- `crates/hypercolor-daemon/src/api/mod.rs`
- `crates/hypercolor-daemon/src/api/system.rs`

### 2.3 Auth/rate-limit/cors policy parity

Spec describes stronger auth/rate-limit posture than current implementation route stack.

Files:

- `crates/hypercolor-daemon/src/api/mod.rs`
- `docs/specs/10-rest-websocket-api.md`

### 2.4 WebSocket protocol depth

Implementation provides basic WS handling but does not yet match full spec semantics for channel model/binary message types/control protocol.

Files:

- `crates/hypercolor-daemon/src/api/ws.rs`
- `docs/specs/10-rest-websocket-api.md`

### 2.5 MCP runtime wiring/compliance depth

MCP module exists with tests, but startup wiring and spec-level tool/resource completeness are not yet at full parity.

Files:

- `crates/hypercolor-daemon/src/mcp/*`
- `crates/hypercolor-daemon/src/main.rs`
- `docs/specs/11-mcp-server.md`

### 2.6 Screen capture integration depth

Screen capture exists in core input modules but daemon startup/render integration is not fully wired to spec intent.

Files:

- `crates/hypercolor-daemon/src/startup.rs`
- `crates/hypercolor-daemon/src/render_thread.rs`
- `docs/specs/14-screen-capture.md`

## 3) Core Architecture Drift

### 3.1 Device dedup identity model

Registry dedup keyed off `DeviceId`-derived fingerprint instead of scanner-provided transport fingerprint, causing unstable identity across rescans.

Files:

- `crates/hypercolor-core/src/device/registry.rs`
- `crates/hypercolor-core/src/device/discovery.rs`
- `crates/hypercolor-core/src/device/wled/scanner.rs`
- `crates/hypercolor-core/src/device/openrgb/scanner.rs`

### 3.2 Render loop parity vs spec

Current render loop is primarily timing orchestration and does not yet implement the full staged pipeline model described in spec.

Files:

- `crates/hypercolor-core/src/engine/mod.rs`
- `docs/specs/01-core-engine.md`

### 3.3 Output path behavior

Spec targets fully non-blocking latest-frame semantics; current output path still includes synchronous/serial portions.

Files:

- `crates/hypercolor-core/src/device/manager.rs`
- `crates/hypercolor-core/src/device/wled/backend.rs`

### 3.4 OpenRGB architecture boundary

Spec describes an out-of-process bridge boundary; implementation is currently direct in-process OpenRGB SDK client/backend.

Files:

- `crates/hypercolor-core/src/device/openrgb/*`
- `docs/specs/05-openrgb-bridge.md`

## 4) CLI Spec Command Surface Drift

Current CLI command tree (`hyper`) does not fully match `docs/specs/15-cli-commands.md` command surface and transport contract.

Current implementation modules:

- `status`, `devices`, `effects`, `scenes`, `profiles`, `layouts`, `config`, `diagnose`, `completions`

Spec includes additional command families and transport expectations not yet implemented in code.

## 5) Test Strategy Drift

Test suite is broad and mostly green, but high-value missing integration areas remain:

- CLI binary <-> live daemon round-trip tests
- WebSocket contract tests at protocol level
- Auth/rate-limit policy tests
- Full config precedence behavior tests

## Deferred / Product Decisions to Reflect in Spec

1. USB/HID backend work is currently on hold.
2. OpenRGB is the immediate delegation path.
3. Potential future Rust-native OpenRGB path should be reflected as a roadmap option.

## Recommendation for Spec Update Agent

1. Update specs to explicitly mark deferred USB scope and current OpenRGB strategy.
2. Normalize API envelope and endpoint naming to current intended contract.
3. Clarify daemon bind/config precedence rules.
4. Mark MCP/WS/screen-capture sections as phased if full parity is not immediate.
5. Align CLI spec command matrix to near-term implementation plan rather than aspirational long-form surface.

