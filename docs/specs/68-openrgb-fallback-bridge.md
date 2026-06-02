# Spec 68: OpenRGB Fallback Bridge

## Status

Driver slice implemented. The active milestone is the clean SDK crate and Bridge
driver against a user-installed or externally managed OpenRGB server.

Bundled OpenRGB supervision, post-install download/install flows, Steam lanes,
and release distribution compliance artifacts are deferred to later milestones.
They must not block the driver slice and must not be quietly folded into it.

## Goal

Hypercolor remains the primary effects, spatial mapping, audio, scene, and render
engine. OpenRGB can fill hardware coverage gaps as a separate, unmodified
runtime, controlled only through the documented OpenRGB SDK TCP protocol.

Hypercolor code stays Apache-2.0. No OpenRGB source code, GPL bindings, or copied
GPL implementation expressions enter Hypercolor.

## Non-Goals

- Replacing native Hypercolor drivers with OpenRGB.
- Linking to OpenRGB, `openrgb2`, or any GPL OpenRGB-derived library.
- Auto-installing privileged OpenRGB components without explicit user opt-in.
- Driving the same physical device or bus from both Hypercolor and OpenRGB.
- Shipping OpenRGB in a Steam depot without human legal approval.

## Architecture

OpenRGB support is split into four independent layers:

1. `hypercolor-openrgb-sdk`: an Apache-2.0 SDK protocol crate with zero
   Hypercolor dependencies.
2. `hypercolor-driver-openrgb`: a Bridge-class Hypercolor driver that consumes
   the SDK crate.
3. App supervisor support for optional system or bundled OpenRGB endpoint
   management.
4. Packaging compliance artifacts for release lanes that distribute OpenRGB.

The driver is classified as `DriverModuleKind::Bridge` and
`DriverTransportKind::Bridge`. The underlying IPC is TCP, but the controlled
hardware sits behind an out-of-process OpenRGB service, not directly on the LAN.

## Provenance Gate

Before SDK implementation starts, the project must choose one provenance mode and
record it in `hypercolor-openrgb-sdk/CLEANROOM.md`.

### Public-Docs/Capture Mode

Implementation may use only:

- Public OpenRGB SDK protocol documentation.
- Black-box packet captures from a running OpenRGB server.
- Tests written against fake or captured server frames.

Implementation must not read OpenRGB implementation files such as
`NetworkProtocol`, `NetworkClient`, or `RGBController` sources.

### Formal Clean-Room Mode

One isolated reader may study GPL implementation sources and produce a neutral
wire-protocol spec. A separate implementer builds the SDK crate from that spec
without viewing GPL source. The reader, implementer, source files consulted, and
neutral spec artifacts must be logged.

### Shared Rules

- Wire facts are recorded by field in `CLEANROOM.md`.
- Apache crate purity checks are defense-in-depth, not proof of provenance.
- Legal approval happens before SDK implementation, not only before release.

## Ownership Gate

Hardware safety must be enforced before OpenRGB scans or claims devices.
Hypercolor-side filtering controls what the user sees; it does not protect the
bus.

The ownership stack has two layers:

1. OpenRGB detector configuration prevents OpenRGB from scanning or claiming
   native-owned detector classes.
2. Hypercolor driver filtering exposes only controllers allowed by the current
   ownership partition.

Gate B must prove that unmodified OpenRGB can be configured at the detector-class
granularity needed by the target release lane. If that cannot be proven, OpenRGB
fallback is limited to explicit static partitions such as "native HAL owns local
USB/SMBus" or "OpenRGB owns this detector class." Dynamic cross-process locks are
not assumed.

Native Hypercolor drivers win by default only where OpenRGB can be prevented from
claiming the same detector class or bus. Otherwise the user must choose a static
partition before either stack opens the hardware.

## Identity Confidence

OpenRGB controller indices are routing metadata only. They are not stable
identity.

The driver computes a fingerprint from the endpoint plus the best available
controller identity fields:

- serial
- location
- vendor
- name
- model or description
- zone and LED shape

Each controller receives a confidence level:

- `high`: stable serial or location plus vendor/name.
- `medium`: vendor/name plus stable topology and endpoint.
- `low`: ambiguous strings, empty serial/location, or index-only fallback.

Low-confidence controllers that could overlap native USB or SMBus hardware are
visible but output-disabled until the user assigns ownership. Index-only
fallbacks are never auto-output-enabled for contention-prone hardware.

On `DEVICE_LIST_UPDATED`, the driver re-enumerates controllers and remaps
runtime indices to fingerprints before writing another frame.

## SDK Protocol Scope

The SDK crate implements only non-persistent realtime control:

- packet header codec and little-endian fields
- fragmented TCP reassembly
- strict length, count, and multiplication-overflow validation
- `SET_CLIENT_NAME`
- protocol version negotiation
- controller count and controller data requests
- version-specific controller parsing for approved protocol versions
- `DEVICE_LIST_UPDATED`
- best-effort rescan when the negotiated protocol supports it
- writable mode discovery
- writable mode activation
- `UPDATELEDS`
- `UPDATEZONELEDS`

Forbidden opcodes:

- `SAVEMODE`
- `RESIZEZONE`

Those opcodes require a future spec update before any implementation.

Supported protocol versions must be pinned before implementation. Negotiation is
`min(client_max, server_max)`. Servers below the minimum supported protocol
version fail with an actionable error instead of best-effort parsing.

Writable modes are selected by capability flags, not by the display name
"Direct." The mode must support per-LED color writes and must not persist writes
to device flash. Modes with auto-save semantics are rejected even if `SAVEMODE`
is never sent.

## Driver Behavior

Driver config lives under `[drivers.openrgb]` and includes:

- `endpoints`
- connect, read, and write timeouts
- `auto_connect`
- `startup_rescan`
- detector ownership partition
- controller cadence overrides
- teardown policy
- explicit insecure non-loopback opt-in

Loopback endpoints are the default. Non-loopback endpoints are refused unless the
user explicitly enables insecure remote SDK access. The OpenRGB SDK protocol has
no authentication or encryption; loopback is still reachable by local processes.

Discovery:

1. Connect to configured endpoints with short timeouts.
2. Negotiate protocol version.
3. Enumerate controllers.
4. Apply detector partition and identity confidence rules.
5. Publish eligible `DeviceInfo` values.
6. Publish low-confidence or non-writable devices as output-disabled with exact
   reasons.

Connect:

1. Re-resolve the controller fingerprint to a runtime index.
2. Select and set a writable mode.
3. Verify the active mode when possible.
4. Record the previous mode for teardown.
5. Start per-controller output.

Reconnect uses bounded backoff because OpenRGB is an external process. Every
reconnect repeats negotiation, enumeration, ownership filtering, and writable
mode setup.

Teardown restores the previous mode when known. If the previous mode cannot be
restored, the configured policy decides whether to blackout or leave the last
frame. A crash may leave hardware showing the last direct frame; that is an
inherent external-process limitation.

## Output Model

OpenRGB output uses per-controller latest-value slots and writer tasks exposed
through `DeviceFrameSink`. The render thread never blocks on OpenRGB I/O.

Slow controllers drop intermediate frames locally. Fast controllers keep their
cadence. The global render tier is never lowered because of OpenRGB. SMBus
cadence limits are hardware-fundamental bus-protection constraints, not
performance nerfs; hammering SMBus can stall other bus traffic.

The driver advertises realistic per-device `target_fps` values, tracks
per-controller health inputs, and derives coarse health through the current
driver API:

- connection state
- negotiated protocol version
- controller count
- last successful write
- consecutive write failures
- output-disabled reason

The current `DeviceBackend::health_check` contract exposes only
`healthy`/`degraded`/`unreachable`. Negotiated protocol and output-disabled
reason already surface through discovery metadata, and exact SDK write failures
surface through the daemon's async device-output metrics after the output queue
observes the driver's frame-sink error. Structured per-controller health
metadata in one typed health payload, such as last success timestamp, protocol,
and output-disabled reason, requires a future driver-health schema expansion.

## App Supervisor (Deferred)

The app supervisor is separate from the driver. The driver receives endpoints and
does not care whether they come from a system OpenRGB server or a supervised
bundled runtime.

Supervisor behavior:

- Prefer an existing system OpenRGB server.
- Never double-spawn if a server already owns the endpoint.
- Start bundled OpenRGB only after explicit user opt-in.
- Bind bundled OpenRGB to `127.0.0.1`.
- Keep OpenRGB independently runnable and visibly separate software.
- Explain privilege requirements and the Windows WinRing0/blocklist caveat.
- Avoid orphaning supervised OpenRGB on shutdown or uninstall.

## Distribution (Deferred)

The default Steam lane is post-install OpenRGB download or installation from a
Hypercolor-controlled distribution endpoint, outside the Steam depot and outside
Steamworks DRM.

Shipping an OpenRGB binary inside a Steam depot is blocked until human legal
approval resolves GPLv2 redistribution rights against Steam Subscriber Agreement
restrictions.

Any lane where Hypercolor distributes OpenRGB must:

- ship the GPLv2 license text
- pin the exact OpenRGB tag or commit
- self-archive the exact corresponding source tarball
- provide a written source offer valid for three years when used
- keep OpenRGB separable on disk as a third-party component
- avoid DRM wrapping of the OpenRGB binary where checkable
- audit OpenRGB's bundled dependencies for the target platform

## Tests

Current driver slice:

SDK tests:

- packet header, endian, and size round trips
- fragmented TCP packet reassembly
- truncated packets
- lying length fields
- `count * size` overflow
- plausible headers with short bodies
- supported protocol-version corpus
- below-minimum protocol rejection
- writable mode selection by flags
- persistent mode rejection
- `SAVEMODE` and `RESIZEZONE` never emitted
- `UPDATELEDS` and `UPDATEZONELEDS` payloads
- synthesized golden controller-data corpus for supported protocol versions
- fake SDK server integration coverage for client request/response flow

The active slice uses public-doc synthesized golden fixtures and fake SDK
servers. A real black-box packet corpus from an unmodified OpenRGB SDK server is
a future compatibility-hardening gate, not a provenance requirement for this
milestone.

Driver tests:

- config validation
- loopback-only defaults
- explicit insecure endpoint opt-in
- identity confidence scoring
- output-disabled reasons
- detector ownership partition filtering
- low-confidence native-overlap suppression
- `DEVICE_LIST_UPDATED` index remap
- reconnect sequence
- health state
- per-controller `target_fps`
- slow-controller isolation

Deferred supervisor and distribution tests:

Supervisor and packaging tests:

- system OpenRGB preferred before bundled startup
- no double-spawn on occupied endpoint
- loopback binding configuration
- compliance artifacts present for distribution lanes
- purity checks for Apache crates

## Verification Gates

Each implementation slice runs focused crate tests. Before merge:

- `just check`
- `just verify`
- independent review of SDK parser and ownership logic
- legal checkpoint before distributing OpenRGB artifacts

For the active driver slice, focused verification is:

- `cargo test -p hypercolor-openrgb-sdk -p hypercolor-driver-openrgb`
- `cargo clippy -p hypercolor-openrgb-sdk -p hypercolor-driver-openrgb -- -D warnings`
- `cargo test -p hypercolor-driver-builtin`
