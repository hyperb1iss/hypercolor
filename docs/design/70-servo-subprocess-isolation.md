# Servo subprocess isolation

Status: proposed architecture. The in-process hard-stall state and bounded command
ingress are short-term containment, not crash isolation.

## Goal

Move HTML rendering behind a supervised process boundary without reducing canvas
resolution, render tiers, display cadence, or GPU residency. A runaway script,
native Servo crash, or worker out-of-memory condition must degrade only the sessions
assigned to that worker. The daemon render loop must remain responsive and retain
last-good output.

## Process shape

Add a `hypercolor-servo-worker` binary and a daemon-side supervisor behind the
existing `ServoRenderer` and `ServoSessionHandle` APIs.

- The daemon owns session intent, deadlines, last-good frames, degradation events,
  and restart policy.
- A worker owns Servo, JavaScript execution, paint, and readback/import resources.
- Workers are sharded fault domains, not a global singleton. Scene HTML and display
  faces can use separate workers, and the pool can grow with independent workloads.
- Worker exit is never interpreted as successful cancellation. The supervisor marks
  affected sessions degraded, retires their transport resources, and starts a clean
  process before replaying session state.

The initial migration may use one child while the protocol settles, but the protocol
must carry a worker and session identifier from day one. It must not encode a new
single-worker assumption.

## Control plane

Use a versioned, length-prefixed protocol over a local Unix-domain socket on Unix and
a named pipe on Windows. Every command carries a monotonic request ID and session ID.
Replies echo both IDs so a response from a retired worker generation cannot satisfy a
new request.

Lifecycle commands are ordered barriers. Render updates use bounded latest-value
semantics per session: one executing render and one replaceable pending render. A
wedged worker therefore cannot accumulate frames or heavy input snapshots.

The minimum protocol covers:

- worker hello, protocol negotiation, capabilities, and heartbeat
- create, load, resize, render, destroy, memory report, and shutdown
- structured success, script error, render error, and transport-fatal replies
- CPU-frame and GPU-frame transport descriptors with explicit generation and lease
  release messages

Protocol types belong in a small dependency-light crate shared by the daemon and
worker. They must not expose Servo implementation types.

## Frame transport

CPU frames use a daemon-created shared-memory ring with generation-qualified slots.
The worker writes pixels, publishes metadata with release ordering, and signals the
daemon. The daemon retains the newest complete slot and explicitly releases older
leases. A worker restart creates a new generation, so stale completion cannot free a
replacement slot.

GPU frames preserve the current zero-copy path:

- Linux exports DMA-BUF-compatible handles and explicit synchronization metadata.
- Windows exports shareable texture handles and fence/keyed-mutex metadata.
- macOS exports IOSurface-backed textures and the synchronization metadata required
  by `hypercolor-macos-gpu-interop`.
- Unsupported platforms negotiate CPU shared memory during worker startup.

CPU transport is a capability fallback, not the default recovery strategy on systems
where GPU import already works. The migration gate requires the current GPU frame
rate, copy count, and import-failure baseline to hold.

## Watchdog and recovery

The parent measures deadlines with its monotonic clock. A render that crosses the
soft threshold retains last-good output. At the hard deadline, the parent emits one
degraded event and starts a short termination grace period. If the worker does not
exit, the parent terminates the process with the operating-system primitive and waits
for confirmed exit before replacing it.

Heartbeats travel on the control transport and are useful only while no render is
outstanding; render deadlines remain authoritative during JavaScript execution. A
dedicated parent watchdog cannot be starved by the child event loop.

After restart, the supervisor replays the session descriptor, URL or generated HTML,
canvas dimensions, display descriptor, controls, and newest demanded input state.
The first valid frame emits the existing recovered event. Crash-loop backoff protects
the host from repeated process creation, but it does not reduce render cadence after
recovery or hide the underlying fault.

## Resource boundary

The child receives no daemon secrets or device handles. It gets only the effect
assets and transport resources needed for its assigned sessions. Apply platform
resource controls where available:

- cgroup v2 or systemd scope limits on Linux
- Job Object memory and process limits on Windows
- process memory and file-descriptor limits on macOS

An out-of-memory kill is treated like any other worker-generation failure. Resource
limits must be configurable and measured against real HTML workloads before defaults
ship.

## Packaging and startup

The worker binary ships beside the daemon and reports its exact protocol version at
startup. Packaging checks must fail when the binary is missing or incompatible.
Development builds resolve the sibling target artifact explicitly; production code
must not search `$PATH` for an arbitrary executable.

The daemon starts workers lazily on first HTML demand and retires them during orderly
shutdown. Worker stderr is captured into bounded structured diagnostics with session
and generation context.

## Migration

1. Extract versioned protocol types and run the existing in-process worker through a
   loopback adapter in tests.
2. Add the worker binary with CPU shared-memory transport and fault-injection tests.
3. Move scene HTML behind the supervisor while display faces remain on the old path.
4. Add platform GPU transport and prove parity against the current import metrics.
5. Move display faces, enable multiple fault domains, then delete the in-process
   worker and its temporary command-channel containment.

Each phase must preserve the public renderer API and support rollback to the previous
phase without changing effect metadata.

## Acceptance gates

- Infinite JavaScript causes one worker termination and restart while the daemon
  continues rendering native effects and device output.
- Native crash and forced out-of-memory tests cannot terminate the daemon.
- Stale replies and frame completions from worker generation N are rejected after
  generation N+1 starts.
- Queue depth remains bounded under a wedged worker and render submission never
  blocks the render thread.
- Session replay restores controls, dimensions, demanded inputs, and display metadata
  before the recovered event.
- GPU-capable systems retain zero CPU framebuffer readbacks in the steady state.
- Controlled benchmarks preserve the current FPS, latency, resolution, and copy-count
  baselines for both scene HTML and display faces.
