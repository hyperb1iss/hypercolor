# Spec 23 — Session & Power Awareness

> Know when the human leaves. Fade gracefully, wake instantly, respect the machine's power state.

**Status:** Draft
**Crate:** `hypercolor-core`
**Module path:** `hypercolor_core::session`
**Depends on:** Event Bus (spec 09), Configuration (spec 12), Scenes & Automation (spec 13)
**Feeds into:** `DesktopTriggerSource` (spec 13 §7), `SystemTriggerSource` (spec 13 §7)

---

## Table of Contents

1. [Overview](#1-overview)
2. [SessionEvent Enum](#2-sessionevent-enum)
3. [SessionMonitor Trait](#3-sessionmonitor-trait)
4. [Logind Monitor](#4-logind-monitor)
5. [Screensaver Monitor](#5-screensaver-monitor)
6. [Idle Monitor](#6-idle-monitor)
7. [Lid Monitor](#7-lid-monitor)
8. [Composite Session Watcher](#8-composite-session-watcher)
9. [Sleep Policy](#9-sleep-policy)
10. [Render Loop Integration](#10-render-loop-integration)
11. [Preferences](#11-preferences)
12. [Configuration Schema](#12-configuration-schema)
13. [Dependencies](#13-dependencies)
14. [Module Layout](#14-module-layout)
15. [Prior Art: uchroma](#15-prior-art-uchroma)

---

## 1. Overview

Hypercolor should be invisible when the user walks away. No LEDs blazing into a locked screen. No USB traffic hammering a suspended laptop. And when the user returns, lighting should restore seamlessly — no stale handles, no missed frames, no jarring pop-in.

This spec defines a **session monitor** subsystem that observes desktop and hardware power events and translates them into a unified `SessionEvent` stream. The daemon's render loop and the automation engine's trigger sources both consume this stream.

**Design principles:**

- **Linux-first.** D-Bus (logind, freedesktop screensaver) and kernel (evdev) are the primary backends. Cross-platform can follow later.
- **Event-driven, not polling.** Subscribe to D-Bus signals and kernel input events. No timers ticking to check state, except as a fallback for idle detection on X11.
- **Graceful degradation.** Each monitor is independent. If logind is unreachable (container, WSL), the screensaver monitor still works. If neither D-Bus bus is available, the daemon runs without session awareness.
- **Inhibitor-aware.** Acquire a logind sleep inhibitor lock so the daemon can complete its fade-out before the kernel actually suspends.
- **Preference-driven.** Users control what happens on each event: fade to black, dim to N%, switch to a scene, or do nothing.

---

## 2. SessionEvent Enum

All session monitors produce the same event type. Consumers never need to know which monitor fired.

```rust
use std::time::Duration;

/// A desktop or hardware session state change.
///
/// Published to the event bus and consumed by the render loop
/// (for immediate LED response) and automation triggers
/// (for user-configured rule evaluation).
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// The user's session was locked (logind Lock signal or
    /// screensaver ActiveChanged=true).
    ScreenLocked,

    /// The user's session was unlocked.
    ScreenUnlocked,

    /// The system is about to suspend (logind PrepareForSleep=true).
    /// The daemon has a short window (held by an inhibitor lock)
    /// to fade LEDs before the kernel freezes everything.
    Suspending,

    /// The system has resumed from suspend (PrepareForSleep=false).
    /// USB handles may be stale and need re-opening.
    Resumed,

    /// The user has been idle for at least the configured threshold.
    /// Fired once when crossing the threshold, not repeatedly.
    IdleEntered { idle_duration: Duration },

    /// The user became active again after being idle.
    IdleExited,

    /// Laptop lid was closed (evdev SW_LID).
    LidClosed,

    /// Laptop lid was opened.
    LidOpened,
}
```

### Relationship to `HypercolorEvent`

`SessionEvent` is a subsystem-internal type. It gets lifted into the main event bus as:

```rust
// In the HypercolorEvent enum (spec 09):
SessionChanged(SessionEvent),
```

And consumed by the `DesktopTriggerSource` and `SystemTriggerSource` (spec 13 §7) to populate their state maps:

| SessionEvent | Trigger Source | Event Type | State Key |
|---|---|---|---|
| `ScreenLocked` | `desktop` | `screen_locked` | `screen_locked: true` |
| `ScreenUnlocked` | `desktop` | `screen_unlocked` | `screen_locked: false` |
| `Suspending` | `system` | `suspend` | `power_state: "suspended"` |
| `Resumed` | `system` | `resume` | `power_state: "active"` |
| `IdleEntered` | `desktop` | `idle_entered` | `idle_seconds: N` |
| `IdleExited` | `desktop` | `idle_exited` | `idle_seconds: 0` |
| `LidClosed` | `system` | `lid_closed` | `lid_open: false` |
| `LidOpened` | `system` | `lid_opened` | `lid_open: true` |

---

## 3. SessionMonitor Trait

Each event source implements this trait. Monitors are spawned as independent tokio tasks and push events through a shared channel.

```rust
use tokio::sync::mpsc;

/// A source of desktop/hardware session events.
///
/// Each implementation watches a single event domain (logind, screensaver,
/// evdev lid switch, idle timer). The composite `SessionWatcher` spawns
/// all available monitors and merges their output.
#[async_trait::async_trait]
pub trait SessionMonitor: Send + Sync + 'static {
    /// Human-readable name for logging.
    fn name(&self) -> &'static str;

    /// Run the monitor until cancellation.
    ///
    /// Implementations should:
    /// - Connect to their event source (D-Bus, evdev, etc.)
    /// - Emit events through `tx` as they occur
    /// - Return `Ok(())` on graceful shutdown (token cancelled)
    /// - Return `Err` on unrecoverable connection failure
    ///
    /// The caller handles retries and degraded-mode logging.
    async fn run(
        self,
        tx: mpsc::Sender<SessionEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<()>;
}
```

---

## 4. Logind Monitor

**Crate:** `zbus` (with `tokio` feature)
**Bus:** System (`org.freedesktop.login1`)

Watches two signal families on the user's session:

### 4.1 Suspend / Resume

Subscribe to `org.freedesktop.login1.Manager::PrepareForSleep(bool)`:
- `true` → emit `SessionEvent::Suspending`
- `false` → emit `SessionEvent::Resumed`

### 4.2 Screen Lock / Unlock

Subscribe to the session object's `Lock` and `Unlock` signals:

```
org.freedesktop.login1.Session → Lock   → SessionEvent::ScreenLocked
org.freedesktop.login1.Session → Unlock → SessionEvent::ScreenUnlocked
```

The monitor must resolve the current session object path. Use `Manager::GetSession(session_id)` where `session_id` comes from the `XDG_SESSION_ID` environment variable, or enumerate sessions for the current user via `Manager::ListSessions`.

### 4.3 Sleep Inhibitor Lock

On startup, acquire a `delay` inhibitor lock:

```rust
// Pseudo-code using zbus proxy:
let fd = manager.inhibit(
    "sleep",                          // what
    "hypercolor",                     // who
    "Fading LEDs before suspend",     // why
    "delay",                          // mode: "delay" not "block"
).await?;
```

The kernel holds suspend for up to `InhibitDelayMaxUSec` (default 5s) while the daemon fades LEDs. After the fade completes (or on timeout), drop the fd to release the lock.

**Important:** Re-acquire the inhibitor lock after every resume. The fd is consumed by each sleep cycle.

### 4.4 Fallback

If the system bus is unreachable (containers, WSL, non-systemd inits):
- Log a warning at startup
- The monitor's `run()` returns `Err` immediately
- `SessionWatcher` continues without logind events

---

## 5. Screensaver Monitor

**Crate:** `zbus` (with `tokio` feature)
**Bus:** Session

Subscribes to `ActiveChanged(bool)` on multiple well-known screensaver interfaces. This catches screen lock on desktops where logind's session-level `Lock`/`Unlock` signals aren't reliably emitted (some XFCE/MATE/Cinnamon configurations).

### 5.1 Watched Interfaces

| Service | Object Path | Interface |
|---|---|---|
| `org.freedesktop.ScreenSaver` | `/org/freedesktop/ScreenSaver` | `org.freedesktop.ScreenSaver` |
| `org.gnome.ScreenSaver` | `/org/gnome/ScreenSaver` | `org.gnome.ScreenSaver` |
| `org.mate.ScreenSaver` | `/org/mate/ScreenSaver` | `org.mate.ScreenSaver` |
| `com.canonical.Unity` | `/org/gnome/ScreenSaver` | `org.gnome.ScreenSaver` |

### 5.2 Deduplication

Both the logind monitor and screensaver monitor can fire `ScreenLocked`/`ScreenUnlocked`. The `SessionWatcher` (§8) deduplicates: if `ScreenLocked` was already the last lock-related event emitted, a second one is suppressed.

### 5.3 Screensaver Inhibition (Future)

When the user enables a "presentation mode" or "keep awake" profile, hypercolor could call `Inhibit(app_name, reason)` on the screensaver interface to prevent screen blanking. This is out of scope for the initial implementation but the monitor should expose the connection for reuse.

---

## 6. Idle Monitor

Detects user inactivity without waiting for the screensaver to activate. Useful for progressive dimming (dim at 2 min, deeper dim at 5 min, off at 10 min).

### 6.1 Wayland: `ext-idle-notify-v1`

The preferred path on Wayland compositors. The protocol lets a client request notification when the user has been idle for N milliseconds.

**Crate:** `wayland-protocols` (staging feature) + `wayland-client`

```
ext_idle_notifier_v1::get_idle_notification(timeout_ms, seat) → ext_idle_notification_v1
    → Event::Idled    → SessionEvent::IdleEntered
    → Event::Resumed  → SessionEvent::IdleExited
```

Multiple notifications can be registered for different thresholds (e.g., 120s for dim, 300s for off). The sleep policy (§9) determines which thresholds to register.

**Compositor support:** sway, KDE 6+, wlroots-based compositors, Mutter (GNOME 45+). Check for protocol availability at runtime; fall back to D-Bus idle query if unavailable.

### 6.2 X11: `XScreenSaverQueryInfo`

**Crate:** `x11` or `x11rb`

Polling-based. Query `XScreenSaverInfo.idle` to get milliseconds since last input. Poll interval: 5 seconds (configurable). Emit `IdleEntered` when idle time crosses the threshold, `IdleExited` when it drops below.

### 6.3 D-Bus Fallback

If neither Wayland nor X11 idle detection is available, query `org.freedesktop.ScreenSaver.GetSessionIdleTime()` via the session bus. Returns seconds idle. Same polling approach as X11.

### 6.4 Selection Priority

```
1. Wayland ext-idle-notify-v1 (event-driven, preferred)
2. X11 XScreenSaverQueryInfo (polling, well-tested)
3. D-Bus GetSessionIdleTime (polling, last resort)
4. Disabled (no idle detection, log warning)
```

Auto-detect at startup. The user can force a specific backend or disable idle detection entirely via config.

---

## 7. Lid Monitor

**Crate:** `evdev`
**Kernel interface:** `/dev/input/event*` with `SW_LID` switch capability

### 7.1 Device Discovery

Scan `/dev/input/event*` for devices that report `SW_LID` in their supported switches:

```rust
use evdev::{Device, SwitchType};

fn find_lid_device() -> Option<Device> {
    for path in std::fs::read_dir("/dev/input").ok()? {
        let path = path.ok()?.path();
        if let Ok(device) = Device::open(&path) {
            if device.supported_switches()
                .is_some_and(|s| s.contains(SwitchType::SW_LID))
            {
                return Some(device);
            }
        }
    }
    None
}
```

### 7.2 Event Loop

Use `evdev`'s async tokio support to read `InputEvent`s. Filter for `EV_SW` / `SW_LID`:
- Value `1` → lid closed → `SessionEvent::LidClosed`
- Value `0` → lid opened → `SessionEvent::LidOpened`

### 7.3 Initial State

Read the current switch state on startup via `device.get_switch_state()` to know if the lid is already closed when the daemon launches.

### 7.4 Permissions

Requires read access to `/dev/input/event*`. On most distros, the `input` group grants this. The daemon should already be in this group for USB HID access. If no lid device is found (desktop PC), the monitor silently disables itself.

---

## 8. Composite Session Watcher

`SessionWatcher` is the public API that the daemon spawns. It creates all available monitors and merges their output into a single `broadcast::Sender<SessionEvent>`.

```rust
use tokio::sync::{broadcast, mpsc};

pub struct SessionWatcher {
    /// Merged event stream for consumers.
    event_tx: broadcast::Sender<SessionEvent>,

    /// Cancellation token to shut down all monitors.
    cancel: tokio_util::sync::CancellationToken,

    /// Handles for spawned monitor tasks.
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl SessionWatcher {
    /// Spawn all available session monitors.
    ///
    /// Each monitor runs as an independent tokio task. Monitors that
    /// fail to connect log a warning and are skipped. The watcher
    /// continues with whatever monitors are available.
    pub async fn start(config: &SessionConfig) -> Self { /* ... */ }

    /// Subscribe to the merged session event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.event_tx.subscribe()
    }

    /// Shut down all monitors gracefully.
    pub async fn shutdown(self) { /* ... */ }
}
```

### 8.1 Deduplication

The watcher maintains minimal state to avoid duplicate events:

```rust
struct DeduplicationState {
    screen_locked: bool,
    lid_closed: bool,
    idle: bool,
    suspended: bool,
}
```

Before forwarding an event, check if it's a no-op transition (e.g., `ScreenLocked` when already locked). Suppress duplicates, forward genuine transitions.

### 8.2 Startup State Sync

On launch, query current state from each monitor:
- Logind: `Session.LockedHint` property for current lock state
- Screensaver: `GetActive()` for current screensaver state
- Evdev: `get_switch_state()` for current lid state
- Idle: Start fresh (assume active)

Emit initial events if the daemon starts into an already-locked or lid-closed state.

---

## 9. Sleep Policy

The sleep policy defines **what happens to LEDs** in response to each session event. This is where user preferences meet the event stream.

### 9.1 Policy Actions

```rust
/// What to do with LEDs when a session event fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum SleepAction {
    /// Do nothing. Ignore this event.
    Ignore,

    /// Dim to a target brightness over a duration.
    /// Brightness is a multiplier on the current level (0.0 = off, 1.0 = unchanged).
    Dim {
        brightness: f32,
        fade_ms: u64,
    },

    /// Fade to black and stop sending USB frames.
    /// The most power-efficient option.
    Off {
        fade_ms: u64,
    },

    /// Activate a specific scene (e.g., a dim ambient glow).
    Scene {
        scene_name: String,
        fade_ms: u64,
    },
}
```

### 9.2 Wake Action

```rust
/// What to do when the user returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum WakeAction {
    /// Restore previous brightness / scene with a fade.
    Restore { fade_ms: u64 },

    /// Activate a specific scene on wake.
    Scene { scene_name: String, fade_ms: u64 },
}
```

### 9.3 Default Policies

| Event | Default Sleep Action | Default Wake Action |
|---|---|---|
| `ScreenLocked` | `Dim { brightness: 0.0, fade_ms: 2000 }` | — |
| `ScreenUnlocked` | — | `Restore { fade_ms: 500 }` |
| `Suspending` | `Off { fade_ms: 300 }` | — |
| `Resumed` | — | `Restore { fade_ms: 150 }` |
| `IdleEntered` (stage 1) | `Dim { brightness: 0.3, fade_ms: 3000 }` | — |
| `IdleEntered` (stage 2) | `Dim { brightness: 0.0, fade_ms: 5000 }` | — |
| `IdleExited` | — | `Restore { fade_ms: 300 }` |
| `LidClosed` | `Off { fade_ms: 500 }` | — |
| `LidOpened` | — | `Restore { fade_ms: 300 }` |

**Suspend gets a fast fade** (300ms) because the inhibitor lock has a hard timeout. **Screen lock gets a slow fade** (2s) because there's no time pressure. **Idle is progressive**: stage 1 dims, stage 2 goes dark.

---

## 10. Render Loop Integration

The render loop in `hypercolor-core` must respond to sleep/wake actions efficiently.

### 10.1 States

```rust
/// LED output state managed by the session subsystem.
pub enum LedPowerState {
    /// Normal operation. Effects render, frames write to hardware.
    Active,

    /// Transitioning (fading). A `ValueAnimator` is running.
    /// Effects still render but output is scaled by the animator.
    Fading {
        target_brightness: f32,
        animator: ValueAnimator,
    },

    /// Fully sleeping. No USB frames are sent.
    /// Effects are paused (not stopped) so they resume seamlessly.
    Sleeping,
}
```

### 10.2 Frame Suppression

When `LedPowerState::Sleeping`:
- **Pause the effect engine** (don't advance time or compute frames)
- **Stop calling `write_colors()`** on device backends
- **Keep the render loop alive** (it should check for wake events at a low tick rate, e.g., 1 Hz)

This eliminates USB traffic entirely during sleep, which is critical for:
- Laptop battery life
- Avoiding USB errors on devices that disconnect during suspend
- Not confusing devices that reset state on resume

### 10.3 Resume Sequence

On `Resumed`:
1. Re-discover USB devices (handles may be stale after suspend)
2. Re-connect to devices that were previously connected
3. Re-run init sequences (devices may have reset)
4. Fade brightness back up per the wake action

This mirrors uchroma's `resume()` which explicitly closes and re-opens HID handles.

---

## 11. Preferences

Users configure sleep behavior through the main config file and optionally per-device overrides.

### 11.1 Global Preferences

```toml
[session]
# Master switch. Disable to ignore all session events.
enabled = true

# Idle detection
idle_enabled = true
idle_backend = "auto"           # "auto" | "wayland" | "x11" | "dbus" | "disabled"
idle_dim_timeout_secs = 120     # Stage 1: dim after 2 minutes
idle_off_timeout_secs = 600     # Stage 2: off after 10 minutes

# Screen lock behavior
on_screen_lock = "off"          # "off" | "dim" | "scene" | "ignore"
screen_lock_brightness = 0.0    # Only used when on_screen_lock = "dim"
screen_lock_scene = ""          # Only used when on_screen_lock = "scene"
screen_lock_fade_ms = 2000
screen_unlock_fade_ms = 500

# System suspend behavior
on_suspend = "off"              # "off" | "dim" | "ignore"
suspend_fade_ms = 300
resume_fade_ms = 150

# Laptop lid
on_lid_close = "off"            # "off" | "dim" | "scene" | "ignore"
lid_close_brightness = 0.0
lid_close_scene = ""
lid_close_fade_ms = 500
lid_open_fade_ms = 300
```

### 11.2 Per-Device Overrides

In per-device config files (`devices/razer-huntsman-v2.toml`):

```toml
[session]
# Override global policy for this device.
# Useful for: keeping keyboard backlight on while screen is locked,
# or turning off ambient strips but keeping desk lamp dim.
on_screen_lock = "dim"
screen_lock_brightness = 0.1
```

### 11.3 Runtime Changes

Sleep policy preferences are part of the hot-reloadable config. Changes take effect on the next session event without daemon restart. The REST API exposes:

```
GET  /api/session/status         → current session state + active policies
PUT  /api/session/preferences    → update sleep preferences (hot reload)
POST /api/session/wake           → force wake (override current sleep state)
POST /api/session/sleep          → force sleep (manual trigger)
```

---

## 12. Configuration Schema

### 12.1 Rust Types

```rust
use serde::{Deserialize, Serialize};

/// Session awareness configuration.
/// Lives under `[session]` in `hypercolor.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub enabled: bool,

    // Idle
    pub idle_enabled: bool,
    pub idle_backend: IdleBackend,
    pub idle_dim_timeout_secs: u64,
    pub idle_off_timeout_secs: u64,

    // Screen lock
    pub on_screen_lock: SleepBehavior,
    pub screen_lock_brightness: f32,
    pub screen_lock_scene: String,
    pub screen_lock_fade_ms: u64,
    pub screen_unlock_fade_ms: u64,

    // Suspend
    pub on_suspend: SleepBehavior,
    pub suspend_fade_ms: u64,
    pub resume_fade_ms: u64,

    // Lid
    pub on_lid_close: SleepBehavior,
    pub lid_close_brightness: f32,
    pub lid_close_scene: String,
    pub lid_close_fade_ms: u64,
    pub lid_open_fade_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SleepBehavior {
    #[default]
    Off,
    Dim,
    Scene,
    Ignore,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IdleBackend {
    #[default]
    Auto,
    Wayland,
    X11,
    Dbus,
    Disabled,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            idle_enabled: true,
            idle_backend: IdleBackend::Auto,
            idle_dim_timeout_secs: 120,
            idle_off_timeout_secs: 600,
            on_screen_lock: SleepBehavior::Off,
            screen_lock_brightness: 0.0,
            screen_lock_scene: String::new(),
            screen_lock_fade_ms: 2000,
            screen_unlock_fade_ms: 500,
            on_suspend: SleepBehavior::Off,
            suspend_fade_ms: 300,
            resume_fade_ms: 150,
            on_lid_close: SleepBehavior::Off,
            lid_close_brightness: 0.0,
            lid_close_scene: String::new(),
            lid_close_fade_ms: 500,
            lid_open_fade_ms: 300,
        }
    }
}
```

---

## 13. Dependencies

### 13.1 Required

| Crate | Version | Feature | Purpose |
|---|---|---|---|
| `zbus` | 5.x | `tokio` | D-Bus client (logind, screensaver) |
| `tokio-util` | 0.7.x | — | `CancellationToken` for monitor lifecycle |

### 13.2 Optional (Feature-Gated)

| Crate | Feature Flag | Purpose |
|---|---|---|
| `evdev` | `lid-monitor` | Laptop lid switch via kernel input events |
| `wayland-client` | `idle-wayland` | Wayland display connection |
| `wayland-protocols` | `idle-wayland` | `ext-idle-notify-v1` protocol |
| `x11rb` or `x11` | `idle-x11` | `XScreenSaverQueryInfo` for X11 idle detection |

### 13.3 Dependency Rationale

- **`zbus` over `dbus` crate:** Pure Rust, async-native, vastly better ergonomics. 40M+ downloads, actively maintained. The `dbus` crate requires `libdbus` C library.
- **`evdev` over reading `/proc/acpi`:** Kernel docs warn that procfs lid state is unreliable. evdev is the correct interface.
- **Feature-gated idle/lid:** Desktop PCs don't need lid detection. Headless servers don't need idle detection. Keep the dependency tree lean.

---

## 14. Module Layout

```
crates/hypercolor-core/src/session/
    mod.rs              // SessionEvent, SessionMonitor trait, SessionWatcher,
                        // SleepPolicy, LedPowerState, re-exports
    logind.rs           // LogindMonitor: PrepareForSleep + Lock/Unlock + inhibitor
    screensaver.rs      // ScreensaverMonitor: org.freedesktop.ScreenSaver et al.
    idle.rs             // IdleMonitor: auto-selects backend
    idle_wayland.rs     // WaylandIdleBackend: ext-idle-notify-v1
    idle_x11.rs         // X11IdleBackend: XScreenSaverQueryInfo polling
    idle_dbus.rs        // DbusIdleBackend: GetSessionIdleTime polling
    lid.rs              // LidMonitor: evdev SW_LID
    policy.rs           // SleepPolicy: maps SessionEvent → SleepAction/WakeAction
    config.rs           // SessionConfig, SleepBehavior, IdleBackend serde types
```

---

## 15. Prior Art: uchroma

Hypercolor's session subsystem is a direct evolution of uchroma's `PowerMonitor` (`uchroma/server/power.py`). Key lessons:

### What uchroma got right

| Pattern | uchroma Implementation | Hypercolor Equivalent |
|---|---|---|
| Logind `PrepareForSleep` | `manager.on_prepare_for_sleep()` | `LogindMonitor` §4 |
| Multiple screensaver interfaces | `SCREENSAVERS` tuple with 4 services | `ScreensaverMonitor` §5 |
| Fast vs. slow fade | `FAST_SUSPEND_FADE_TIME = 0.3s` vs normal | `suspend_fade_ms: 300` vs `screen_lock_fade_ms: 2000` |
| Animation pause (not stop) | `AnimationLoop._pause_event` asyncio Event | `LedPowerState::Sleeping` pauses effect engine |
| Brightness save/restore | `preferences.brightness` saved on suspend | Sleep policy saves pre-sleep state |
| Stale USB handle recovery | `resume()` force-closes and re-opens HID | Resume sequence re-discovers and re-connects §10.3 |

### What uchroma was missing

| Gap | Impact | Hypercolor Fix |
|---|---|---|
| No sleep inhibitor lock | LED fade could be cut short by kernel | Logind `Inhibit("delay")` §4.3 |
| No lid detection | Lid close without suspend = LEDs stay on | `LidMonitor` via evdev §7 |
| No idle detection | No progressive dimming before screen lock | `IdleMonitor` with staged thresholds §6 |
| No user preferences | Hardcoded fade times, no per-device override | Full `SessionConfig` with per-device overrides §11 |
| No deduplication | Both logind Lock and screensaver ActiveChanged could double-fire | `DeduplicationState` in SessionWatcher §8.1 |
| Screensaver errors swallowed silently | Hard to debug when session awareness breaks | Structured tracing with monitor names |
| `Suspended` as D-Bus property only | External clients can toggle, but no internal preference-driven policy | REST API + config-driven policy §9, §11 |

---

## Implementation Priority

| Phase | Scope | Value |
|---|---|---|
| **Phase 1** | `LogindMonitor` (suspend/resume + lock/unlock) + sleep inhibitor + basic fade | Covers 80% of use cases with one D-Bus connection |
| **Phase 2** | `ScreensaverMonitor` (DE compatibility) + deduplication | Catches DEs where logind Lock isn't reliable |
| **Phase 3** | `IdleMonitor` (Wayland + X11) + progressive dimming | Smooth experience before screen lock fires |
| **Phase 4** | `LidMonitor` + per-device overrides | Laptop-specific polish |
| **Phase 5** | REST API + UI for sleep preferences | User-facing configuration |
