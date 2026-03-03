# OpenRGB Drift + Decision (2026-03-03)

## 1. Scope

This document compares current OpenRGB implementation in code with the current spec set, and records the decision direction for near-term execution.

Context:

- USB HID backend work is paused for now.
- Hardware coverage is delegated to OpenRGB in the current phase.
- Existing specs are known to be partially stale.

## 2. Current Implementation (Code Reality)

The active integration path is a direct in-process OpenRGB protocol client:

- `crates/hypercolor-core/src/device/openrgb/client.rs`
- `crates/hypercolor-core/src/device/openrgb/backend.rs`
- `crates/hypercolor-core/src/device/openrgb/proto.rs`

Key characteristics:

- TCP SDK protocol client in `hypercolor-core`.
- No gRPC bridge crate in runtime path.
- No `openrgb2` dependency in `hypercolor-core`/`hypercolor-daemon`.
- Strong direct-path test coverage in `crates/hypercolor-core/tests/openrgb_tests.rs`.

## 3. Current Spec Model (Doc Reality)

`docs/specs/05-openrgb-bridge.md` currently describes a different architecture:

- Out-of-process OpenRGB bridge daemon.
- Unix socket + gRPC boundary.
- Mandatory legal firewall framing around GPL concerns.
- Bridge artifacts and RPC surface that are not present in repository code.

## 4. Major Differences Observed

1. Process boundary mismatch
- Spec: out-of-process gRPC bridge.
- Code: direct in-process TCP protocol client.

2. Legal boundary mismatch
- Spec: mandatory GPL firewall via separate bridge process.
- Code: no GPL SDK dependency in current runtime path.

3. Artifact mismatch
- Spec references bridge crates/protos that are absent.
- Code path uses `device/openrgb/*` directly.

4. API/control-plane mismatch
- Spec expects RPCs like `PushFrameBatch`/`HealthCheck` over bridge.
- Code uses direct backend methods and wire packets.

5. Event semantics gap
- Spec expects bridge/event-stream semantics.
- Code path only does request/response-style interactions today.

6. Batching/perf mismatch
- Spec emphasizes bridge batch RPC path.
- Code output manager dispatches per device in current architecture.

7. Identity model mismatch
- Spec describes one identity mapping approach.
- Code uses runtime `DeviceId` + scanner fingerprints for dedupe.

8. USB scope mismatch
- `docs/specs/04-usb-hid-backend.md` reads as active primary scope.
- Current direction is to defer USB/HID and rely on OpenRGB delegation now.

## 5. Risks If We Leave Drift Unresolved

- Contributors implement against the wrong architecture.
- Legal/compliance assumptions remain ambiguous.
- Roadmap planning continues to split across contradictory models.
- Runtime behavior and performance expectations diverge from spec claims.

## 6. Decision Recorded (Current Phase)

Decision for current phase:

- Treat **direct in-process OpenRGB protocol client** as the official implementation path.
- Treat **out-of-process bridge architecture** as a possible future variant, not current truth.
- Keep **USB HID backend formally deferred** until explicitly resumed.

## 7. Changes Applied Alongside This Decision

1. OpenRGB client resiliency update
- `OpenRgbClient` now tolerates unsolicited `DeviceListUpdated` notifications while awaiting response packets.
- Controller cache is invalidated when such notifications arrive.

2. Regression coverage added
- Test added to ensure enumeration succeeds even when async `DeviceListUpdated` notifications interleave with responses.

## 8. Spec Update Instructions (for Spec Agent)

Update these docs to match current project direction:

- `docs/specs/05-openrgb-bridge.md`
  - Mark as future/optional architecture.
  - Add explicit "current implementation" section for direct protocol path.
- `docs/specs/04-usb-hid-backend.md`
  - Add prominent deferred status note.
  - Reference OpenRGB delegation as the active hardware path.
- `docs/ARCHITECTURE.md` and `docs/specs/02-device-backend.md`
  - Align diagrams/text with current direct OpenRGB stack.

## 9. Follow-Up Work Suggested

1. Add explicit dependency policy to prevent accidental GPL SDK linkage in core/daemon.
2. Add performance notes for current per-device dispatch vs future batch opportunities.
3. Add a future ADR for if/when bridge mode is revived.

## 10. Recommendation

Adopt and publish the **direct OpenRGB protocol client as the canonical architecture now**, with USB HID marked deferred and bridge mode treated as future optional work. This minimizes architecture thrash, reflects real code/test reality, and keeps current momentum while preserving a clean path to future bridge isolation if legal or packaging constraints require it later.
