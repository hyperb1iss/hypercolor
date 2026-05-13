# 14 — Desktop Environment Integration

> Making Hypercolor feel native on every Linux desktop, from GNOME to Hyprland.

---

## Overview

Linux isn't one desktop -- it's a constellation. GNOME, KDE Plasma, Hyprland, sway, i3, COSMIC, and countless others each have their own integration patterns, IPC mechanisms, and user expectations. Hypercolor must feel "installed" on all of them -- not like a web app that happens to run in the background.

The daemon (`hypercolor-daemon`) is the central nervous system. It exposes a D-Bus interface (`tech.hyperbliss.hypercolor1`), runs as a systemd user service, and speaks XDG standards for cross-desktop compatibility. Per-DE integrations layer on top: GNOME Shell extensions, KDE Plasma widgets, Waybar modules, i3blocks scripts, and COSMIC applets. Each integration is a thin client that talks to the daemon over D-Bus.

**Design principle:** The daemon is desktop-agnostic. All desktop-specific code lives in separate packages (extensions, widgets, modules) that communicate exclusively through D-Bus and CLI. No GNOME code in the daemon. No KDE code in the daemon. Clean separation.

---

## 1. systemd Integration

### 1.1 User Service (Default)

Hypercolor runs as a **systemd user service** by default. This is the right choice for several reasons:

- No root required -- RGB is a user concern, not a system concern
- Lifecycle tied to user session -- starts on login, stops on logout
- Per-user configuration in `$XDG_CONFIG_HOME/hypercolor/`
- Multiple users on the same machine get independent instances
- udev rules grant USB HID access to the `plugdev` group (no root needed)

```ini
# hypercolor.service
# Installed to: ~/.config/systemd/user/ (user install)
#           or: /usr/lib/systemd/user/  (package install)

[Unit]
Description=Hypercolor RGB Lighting Daemon
Documentation=https://github.com/hyperb1iss/hypercolor
After=graphical-session.target
Wants=graphical-session.target
# Ensure D-Bus session bus is available
After=dbus.socket

[Service]
Type=notify
ExecStart=/usr/bin/hypercolor daemon
ExecReload=/bin/kill -HUP $MAINPID

# Watchdog: daemon must call sd_notify(WATCHDOG=1) every 30s
WatchdogSec=30

# Restart on crash, but not on clean exit
Restart=on-failure
RestartSec=3

# Resource limits
MemoryMax=512M
CPUQuota=25%

# Security hardening
ProtectHome=read-only
ProtectSystem=strict
ReadWritePaths=%h/.config/hypercolor %h/.local/share/hypercolor
PrivateTmp=true
NoNewPrivileges=true
RestrictRealtime=true

# Environment
Environment=HYPERCOLOR_LOG=info
Environment=RUST_BACKTRACE=1

[Install]
WantedBy=default.target
```

### 1.2 Socket Activation

systemd opens the HTTP port and hands the file descriptor to Hypercolor on first connection. This eliminates port conflicts and enables on-demand startup.

```ini
# hypercolor.socket
[Unit]
Description=Hypercolor HTTP Socket

[Socket]
ListenStream=9420
Accept=no
# Pass the socket FD to the service
FileDescriptorName=http

[Install]
WantedBy=sockets.target
```

The daemon detects socket activation via the `listenfd` crate:

```rust
use listenfd::ListenFd;
use tokio::net::TcpListener;

pub async fn create_listener() -> TcpListener {
    let mut listenfd = ListenFd::from_env();

    // Try systemd socket activation first
    if let Ok(Some(listener)) = listenfd.take_tcp_listener(0) {
        listener.set_nonblocking(true).unwrap();
        TcpListener::from_std(listener).unwrap()
    } else {
        // Fallback: bind our own socket
        TcpListener::bind("127.0.0.1:9420").await.unwrap()
    }
}
```

### 1.3 Watchdog Monitoring

The daemon sends `sd_notify(WATCHDOG=1)` heartbeats every 15 seconds (half of the 30-second `WatchdogSec`). If the render loop stalls or the async runtime deadlocks, systemd kills and restarts the service.

```rust
use sd_notify::NotifyState;

/// Call from the main render loop or a dedicated watchdog task
pub fn notify_watchdog() {
    let _ = sd_notify::notify(false, &[NotifyState::Watchdog]);
}

/// Call during startup after initialization completes
pub fn notify_ready() {
    let _ = sd_notify::notify(false, &[
        NotifyState::Ready,
        NotifyState::Status("Hypercolor daemon ready"),
    ]);
}

/// Call during graceful shutdown
pub fn notify_stopping() {
    let _ = sd_notify::notify(false, &[NotifyState::Stopping]);
}

/// Spawn a watchdog task on the tokio runtime
pub fn spawn_watchdog(mut shutdown: tokio::sync::watch::Receiver<bool>) {
    tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(15);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {
                    notify_watchdog();
                }
                _ = shutdown.changed() => break,
            }
        }
    });
}
```

### 1.4 Journal Logging

Hypercolor uses `tracing` with `tracing-journald` as a subscriber layer. Structured fields are preserved as journal metadata, enabling powerful filtering:

```bash
# View hypercolor logs
journalctl --user -u hypercolor -f

# Filter by severity
journalctl --user -u hypercolor -p warning

# Filter by structured field
journalctl --user -u hypercolor DEVICE=prism8 SUBSYSTEM=hid

# Show logs since last boot
journalctl --user -u hypercolor -b
```

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub fn init_logging() {
    let journald = tracing_journald::layer()
        .expect("Failed to connect to journald");

    let stderr = tracing_subscriber::fmt::layer()
        .with_target(false)
        .compact();

    tracing_subscriber::registry()
        .with(journald)
        .with(stderr)
        .with(tracing_subscriber::EnvFilter::from_env("HYPERCOLOR_LOG"))
        .init();
}
```

### 1.5 Service Management via CLI

The `hypercolor` CLI wraps common systemctl operations:

```bash
hypercolor service start          # systemctl --user start hypercolor
hypercolor service stop           # systemctl --user stop hypercolor
hypercolor service restart        # systemctl --user restart hypercolor
hypercolor service status         # systemctl --user status hypercolor
hypercolor service enable         # systemctl --user enable hypercolor
hypercolor service disable        # systemctl --user disable hypercolor
hypercolor service logs           # journalctl --user -u hypercolor -f
hypercolor service logs --since "5 min ago"
```

### 1.6 System Service Mode (Optional)

For kiosks, digital signage, or multi-user setups where a single daemon controls all RGB hardware:

```ini
# /etc/systemd/system/hypercolor.service
[Unit]
Description=Hypercolor RGB Lighting Daemon (System)
After=multi-user.target

[Service]
Type=notify
User=hypercolor
Group=hypercolor
SupplementaryGroups=plugdev
ExecStart=/usr/bin/hypercolor daemon --system
WatchdogSec=30
Restart=on-failure

# System-wide config
Environment=XDG_CONFIG_HOME=/etc/hypercolor
Environment=XDG_DATA_HOME=/var/lib/hypercolor

[Install]
WantedBy=multi-user.target
```

---

## 2. D-Bus Interface Specification

The D-Bus interface is the universal integration point. Every desktop integration -- GNOME extension, KDE widget, Waybar module, i3blocks script -- communicates through this interface.

### 2.1 Bus Name & Object Path

```
Bus Name:    tech.hyperbliss.hypercolor1
Object Path: /tech/hyperbliss/hypercolor1
```

### 2.2 Interface: `tech.hyperbliss.hypercolor1.Daemon`

Core daemon control and state queries.

```xml
<node>
  <interface name="tech.hyperbliss.hypercolor1.Daemon">

    <!-- === Properties === -->

    <!-- Current daemon state: "running", "paused", "error" -->
    <property name="State" type="s" access="read">
      <annotation name="org.freedesktop.DBus.Property.EmitsChangedSignal" value="true"/>
    </property>

    <!-- Current effect name, empty if none -->
    <property name="CurrentEffect" type="s" access="read">
      <annotation name="org.freedesktop.DBus.Property.EmitsChangedSignal" value="true"/>
    </property>

    <!-- Current profile name -->
    <property name="CurrentProfile" type="s" access="read">
      <annotation name="org.freedesktop.DBus.Property.EmitsChangedSignal" value="true"/>
    </property>

    <!-- Render FPS (actual, not target) -->
    <property name="Fps" type="u" access="read"/>

    <!-- Target render FPS -->
    <property name="TargetFps" type="u" access="readwrite">
      <annotation name="org.freedesktop.DBus.Property.EmitsChangedSignal" value="true"/>
    </property>

    <!-- Number of connected devices -->
    <property name="DeviceCount" type="u" access="read"/>

    <!-- Global brightness (0-100) -->
    <property name="Brightness" type="u" access="readwrite">
      <annotation name="org.freedesktop.DBus.Property.EmitsChangedSignal" value="true"/>
    </property>

    <!-- Version string -->
    <property name="Version" type="s" access="read"/>

    <!-- Web UI URL -->
    <property name="WebUrl" type="s" access="read"/>


    <!-- === Methods === -->

    <!-- Set the active effect. params is a JSON string of control values. -->
    <method name="SetEffect">
      <arg name="effect_id" type="s" direction="in"/>
      <arg name="params" type="s" direction="in"/>
    </method>

    <!-- Apply a named profile -->
    <method name="SetProfile">
      <arg name="profile_name" type="s" direction="in"/>
    </method>

    <!-- Cycle to the next effect in the current playlist/favorites -->
    <method name="NextEffect"/>

    <!-- Cycle to the previous effect -->
    <method name="PreviousEffect"/>

    <!-- Pause rendering (LEDs hold last frame or dim) -->
    <method name="Pause"/>

    <!-- Resume rendering -->
    <method name="Resume"/>

    <!-- Toggle pause/resume -->
    <method name="Toggle"/>

    <!-- List available effects: returns array of (id, name, category) -->
    <method name="ListEffects">
      <arg name="effects" type="a(sss)" direction="out"/>
    </method>

    <!-- List connected devices: returns array of (id, name, backend, led_count, status) -->
    <method name="ListDevices">
      <arg name="devices" type="a(sssus)" direction="out"/>
    </method>

    <!-- List available profiles -->
    <method name="ListProfiles">
      <arg name="profiles" type="as" direction="out"/>
    </method>

    <!-- Get full daemon state as JSON (for rich UIs) -->
    <method name="GetState">
      <arg name="state_json" type="s" direction="out"/>
    </method>

    <!-- Open the web UI in the default browser -->
    <method name="OpenWebUI"/>


    <!-- === Signals === -->

    <!-- Emitted when a device connects -->
    <signal name="DeviceConnected">
      <arg name="device_id" type="s"/>
      <arg name="device_name" type="s"/>
      <arg name="backend" type="s"/>
      <arg name="led_count" type="u"/>
    </signal>

    <!-- Emitted when a device disconnects -->
    <signal name="DeviceDisconnected">
      <arg name="device_id" type="s"/>
      <arg name="device_name" type="s"/>
    </signal>

    <!-- Emitted when the effect changes -->
    <signal name="EffectChanged">
      <arg name="effect_id" type="s"/>
      <arg name="effect_name" type="s"/>
    </signal>

    <!-- Emitted when the profile changes -->
    <signal name="ProfileChanged">
      <arg name="profile_name" type="s"/>
    </signal>

    <!-- Emitted on errors (rendering failures, device errors) -->
    <signal name="Error">
      <arg name="subsystem" type="s"/>
      <arg name="message" type="s"/>
    </signal>

  </interface>
</node>
```

### 2.3 zbus Server Implementation

```rust
use zbus::{interface, Connection, SignalContext};

pub struct HypercolorDbus {
    bus: HypercolorBus,
    config: Arc<RwLock<Config>>,
}

#[interface(name = "tech.hyperbliss.hypercolor1.Daemon")]
impl HypercolorDbus {
    // --- Properties ---

    #[zbus(property)]
    async fn state(&self) -> String {
        self.bus.state().to_string()
    }

    #[zbus(property)]
    async fn current_effect(&self) -> String {
        self.bus.current_effect().unwrap_or_default()
    }

    #[zbus(property)]
    async fn current_profile(&self) -> String {
        self.bus.current_profile().unwrap_or_default()
    }

    #[zbus(property)]
    async fn fps(&self) -> u32 {
        self.bus.actual_fps()
    }

    #[zbus(property)]
    async fn target_fps(&self) -> u32 {
        self.config.read().await.target_fps
    }

    #[zbus(property)]
    async fn set_target_fps(&self, fps: u32) {
        self.config.write().await.target_fps = fps;
    }

    #[zbus(property)]
    async fn device_count(&self) -> u32 {
        self.bus.device_count() as u32
    }

    #[zbus(property)]
    async fn brightness(&self) -> u32 {
        self.config.read().await.brightness
    }

    #[zbus(property)]
    async fn set_brightness(&self, value: u32) {
        self.config.write().await.brightness = value.min(100);
    }

    #[zbus(property)]
    async fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    #[zbus(property)]
    async fn web_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.config.read().await.port)
    }

    // --- Methods ---

    async fn set_effect(&self, effect_id: &str, params: &str) -> zbus::fdo::Result<()> {
        let params: serde_json::Value = serde_json::from_str(params)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        self.bus.set_effect(effect_id, params).await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn set_profile(&self, profile_name: &str) -> zbus::fdo::Result<()> {
        self.bus.set_profile(profile_name).await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn next_effect(&self) { self.bus.next_effect().await; }
    async fn previous_effect(&self) { self.bus.previous_effect().await; }
    async fn pause(&self) { self.bus.pause().await; }
    async fn resume(&self) { self.bus.resume().await; }
    async fn toggle(&self) { self.bus.toggle().await; }

    async fn list_effects(&self) -> Vec<(String, String, String)> {
        self.bus.list_effects().await
            .into_iter()
            .map(|e| (e.id, e.name, e.category))
            .collect()
    }

    async fn list_devices(&self) -> Vec<(String, String, String, u32, String)> {
        self.bus.list_devices().await
            .into_iter()
            .map(|d| (d.id, d.name, d.backend, d.led_count, d.status.to_string()))
            .collect()
    }

    async fn list_profiles(&self) -> Vec<String> {
        self.bus.list_profiles().await
    }

    async fn get_state(&self) -> String {
        serde_json::to_string(&self.bus.full_state().await).unwrap_or_default()
    }

    async fn open_web_ui(&self) {
        let url = format!("http://127.0.0.1:{}", self.config.read().await.port);
        let _ = open::that(&url);
    }

    // --- Signals ---

    #[zbus(signal)]
    async fn device_connected(
        ctx: &SignalContext<'_>,
        device_id: &str,
        device_name: &str,
        backend: &str,
        led_count: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_disconnected(
        ctx: &SignalContext<'_>,
        device_id: &str,
        device_name: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn effect_changed(
        ctx: &SignalContext<'_>,
        effect_id: &str,
        effect_name: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn profile_changed(
        ctx: &SignalContext<'_>,
        profile_name: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn error(
        ctx: &SignalContext<'_>,
        subsystem: &str,
        message: &str,
    ) -> zbus::Result<()>;
}
```

### 2.4 D-Bus Service Activation

If the daemon isn't running, D-Bus can auto-start it:

```ini
# /usr/share/dbus-1/services/tech.hyperbliss.hypercolor1.service
[D-BUS Service]
Name=tech.hyperbliss.hypercolor1
Exec=/usr/bin/hypercolor daemon
SystemdService=hypercolor.service
```

### 2.5 CLI Convenience Wrappers

Every D-Bus method is accessible via CLI, using `busctl` under the hood:

```bash
# These are equivalent:
hypercolor set aurora --speed 5
busctl --user call tech.hyperbliss.hypercolor1 \
  /tech/hyperbliss/hypercolor1 \
  tech.hyperbliss.hypercolor1.Daemon \
  SetEffect ss "aurora" '{"speed": 5}'

# Query state
busctl --user get-property tech.hyperbliss.hypercolor1 \
  /tech/hyperbliss/hypercolor1 \
  tech.hyperbliss.hypercolor1.Daemon \
  CurrentEffect

# Monitor signals
busctl --user monitor tech.hyperbliss.hypercolor1
```

---

## 3. XDG Standards -- Cross-Desktop Compatibility

Before diving into per-DE integrations, these XDG standards provide the baseline that works everywhere.

### 3.1 Desktop Entry

```ini
# /usr/share/applications/hypercolor.desktop
[Desktop Entry]
Type=Application
Name=Hypercolor
GenericName=RGB Lighting Controller
Comment=Open-source RGB lighting orchestration for Linux
Exec=hypercolor open
Icon=hypercolor
Categories=Settings;HardwareSettings;
Keywords=RGB;LED;lighting;effects;
StartupNotify=false
Terminal=false
Actions=toggle;next-effect;settings;

[Desktop Action toggle]
Name=Toggle Lighting
Exec=hypercolor toggle
Icon=hypercolor-toggle

[Desktop Action next-effect]
Name=Next Effect
Exec=hypercolor next
Icon=hypercolor-next

[Desktop Action settings]
Name=Open Settings
Exec=hypercolor open
Icon=hypercolor
```

### 3.2 Autostart Entry

```ini
# /etc/xdg/autostart/hypercolor.desktop
# (or ~/.config/autostart/hypercolor.desktop for user install)
[Desktop Entry]
Type=Application
Name=Hypercolor Tray
Comment=Hypercolor system tray indicator
Exec=hypercolor-tray
Icon=hypercolor
X-GNOME-Autostart-enabled=true
X-KDE-autostart-phase=2
OnlyShowIn=GNOME;KDE;XFCE;Cinnamon;MATE;
# WM users (i3/sway/Hyprland) manage autostart themselves
```

Note: The autostart entry launches the tray indicator, not the daemon. The daemon starts via systemd (either socket activation or `default.target`). The tray connects to the running daemon over D-Bus.

### 3.3 XDG Directory Compliance

```
$XDG_CONFIG_HOME/hypercolor/          # ~/.config/hypercolor/
  config.toml                          # Main configuration
  profiles/                            # User profiles
    default.toml
    gaming.toml
  layouts/                             # Spatial layouts
    my-setup.json

$XDG_DATA_HOME/hypercolor/            # ~/.local/share/hypercolor/
  effects/                             # User-installed effects
  devices/                             # Device presets
  cache/                               # Thumbnail cache, compiled shaders

$XDG_STATE_HOME/hypercolor/           # ~/.local/state/hypercolor/
  last-state.json                      # State for resume after restart
  hypercolor.log                       # Fallback log (when journald unavailable)

$XDG_RUNTIME_DIR/hypercolor/          # /run/user/1000/hypercolor/
  hypercolor.sock                      # Unix socket for TUI/CLI IPC
  hypercolor.pid                       # PID file
```

### 3.4 Icon Theme Support

Icons follow the freedesktop icon naming spec. Installed into the hicolor fallback theme:

```
/usr/share/icons/hicolor/
  scalable/apps/hypercolor.svg                 # Primary icon (SVG)
  scalable/status/hypercolor-active.svg        # Tray: active/rendering
  scalable/status/hypercolor-paused.svg        # Tray: paused
  scalable/status/hypercolor-error.svg         # Tray: error state
  scalable/status/hypercolor-idle.svg          # Tray: no devices
  symbolic/apps/hypercolor-symbolic.svg        # Monochrome symbolic icon
  symbolic/status/hypercolor-active-symbolic.svg
  symbolic/status/hypercolor-paused-symbolic.svg
  16x16/apps/hypercolor.png                    # Raster fallbacks
  24x24/apps/hypercolor.png
  32x32/apps/hypercolor.png
  48x48/apps/hypercolor.png
  64x64/apps/hypercolor.png
  128x128/apps/hypercolor.png
  256x256/apps/hypercolor.png
```

### 3.5 Freedesktop Notifications

All notifications go through the XDG notification spec (`org.freedesktop.Notifications`). This works on every desktop that implements the spec (all major ones).

```rust
use zbus::Connection;

pub struct Notifier {
    connection: Connection,
}

impl Notifier {
    pub async fn new() -> zbus::Result<Self> {
        let connection = Connection::session().await?;
        Ok(Self { connection })
    }

    pub async fn notify(
        &self,
        summary: &str,
        body: &str,
        icon: &str,
        urgency: Urgency,
    ) -> zbus::Result<u32> {
        let proxy = zbus::Proxy::new(
            &self.connection,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        ).await?;

        let hints: std::collections::HashMap<&str, zbus::zvariant::Value> = [
            ("urgency", zbus::zvariant::Value::U8(urgency as u8)),
            ("desktop-entry", zbus::zvariant::Value::Str("hypercolor".into())),
            // Transient: don't persist in notification center
            ("transient", zbus::zvariant::Value::Bool(true)),
        ].into();

        let notification_id: u32 = proxy.call(
            "Notify",
            &(
                "Hypercolor",           // app_name
                0u32,                   // replaces_id (0 = new)
                icon,                   // app_icon
                summary,                // summary
                body,                   // body
                Vec::<&str>::new(),     // actions
                hints,                  // hints
                5000i32,                // expire_timeout (ms)
            ),
        ).await?;

        Ok(notification_id)
    }

    /// Check if the user has Do Not Disturb enabled (GNOME/KDE)
    pub async fn is_do_not_disturb(&self) -> bool {
        // GNOME: check org.gnome.desktop.notifications show-banners
        // KDE: check org.kde.notificationmanager inhibited
        // Fall back to false if unknown DE
        false
    }
}

pub enum Urgency {
    Low = 0,
    Normal = 1,
    Critical = 2,
}
```

### 3.6 XDG Desktop Portal -- Screen Capture

Screen capture on Wayland requires the XDG Desktop Portal `ScreenCast` interface. The portal handles the user consent dialog (DE-native picker).

```rust
use zbus::Connection;

pub struct ScreenCapturePortal {
    connection: Connection,
    session_handle: Option<String>,
}

impl ScreenCapturePortal {
    /// Request screen capture access via the portal
    pub async fn request_capture(&mut self) -> Result<PipeWireFd> {
        let proxy = zbus::Proxy::new(
            &self.connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.ScreenCast",
        ).await?;

        // 1. Create session
        let session: zbus::zvariant::OwnedObjectPath = proxy.call(
            "CreateSession",
            &({
                let mut opts = std::collections::HashMap::new();
                opts.insert("session_handle_token", "hypercolor_screen".into());
                opts
            },),
        ).await?;

        // 2. Select sources (monitor output)
        proxy.call("SelectSources", &(
            &session,
            {
                let mut opts = std::collections::HashMap::new();
                // types: 1 = monitor, 2 = window, 4 = virtual
                opts.insert("types", zbus::zvariant::Value::U32(1));
                opts.insert("multiple", zbus::zvariant::Value::Bool(false));
                // persist_mode: 2 = persist until explicitly revoked
                opts.insert("persist_mode", zbus::zvariant::Value::U32(2));
                opts
            },
        )).await?;

        // 3. Start capture -- returns PipeWire node ID + fd
        let (fd, streams) = proxy.call("Start", &(
            &session,
            "",  // parent_window (empty for daemon)
            std::collections::HashMap::<&str, zbus::zvariant::Value>::new(),
        )).await?;

        self.session_handle = Some(session.to_string());
        Ok(fd)
    }
}
```

---

## 4. System Tray / Status Indicator

The system tray icon is Hypercolor's persistent desktop presence. It works on GNOME (via AppIndicator/extension), KDE Plasma, XFCE, MATE, and any DE that supports `StatusNotifierItem`.

### 4.1 StatusNotifierItem (SNI)

SNI is the modern, Wayland-compatible standard for system tray icons. KDE Plasma, XFCE, MATE, and Budgie support it natively. GNOME requires the "AppIndicator" extension.

```rust
use ksni::{Tray, TrayService, MenuItem};

pub struct HypercolorTray {
    state: DaemonState,
    effects: Vec<EffectInfo>,
    profiles: Vec<String>,
}

impl Tray for HypercolorTray {
    fn id(&self) -> String {
        "hypercolor".into()
    }

    fn title(&self) -> String {
        "Hypercolor".into()
    }

    fn icon_name(&self) -> String {
        match self.state {
            DaemonState::Running => "hypercolor-active",
            DaemonState::Paused  => "hypercolor-paused",
            DaemonState::Error   => "hypercolor-error",
            DaemonState::Idle    => "hypercolor-idle",
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Hypercolor".into(),
            description: format!(
                "{} | {} devices | {}fps",
                self.state.current_effect_or("No effect"),
                self.state.device_count,
                self.state.fps,
            ),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            // Header: current state
            MenuItem::Standard(ksni::StandardItem {
                label: format!("Effect: {}", self.state.current_effect_or("None")),
                enabled: false,
                ..Default::default()
            }),
            MenuItem::Separator,

            // Quick effect switcher (submenu)
            MenuItem::Standard(ksni::StandardItem {
                label: "Effects".into(),
                submenu: self.effects.iter().map(|e| {
                    let id = e.id.clone();
                    MenuItem::Standard(ksni::StandardItem {
                        label: e.name.clone(),
                        icon_name: if self.state.current_effect_id == Some(&id) {
                            "emblem-ok-symbolic".into()
                        } else {
                            String::new()
                        },
                        activate: Box::new(move |tray: &mut Self| {
                            tray.set_effect(&id);
                        }),
                        ..Default::default()
                    })
                }).collect(),
                ..Default::default()
            }),

            // Profiles submenu
            MenuItem::Standard(ksni::StandardItem {
                label: "Profiles".into(),
                submenu: self.profiles.iter().map(|p| {
                    let name = p.clone();
                    MenuItem::Standard(ksni::StandardItem {
                        label: p.clone(),
                        activate: Box::new(move |tray: &mut Self| {
                            tray.set_profile(&name);
                        }),
                        ..Default::default()
                    })
                }).collect(),
                ..Default::default()
            }),

            MenuItem::Separator,

            // Brightness slider (approximated as submenu tiers)
            MenuItem::Standard(ksni::StandardItem {
                label: format!("Brightness: {}%", self.state.brightness),
                submenu: [25, 50, 75, 100].iter().map(|&b| {
                    MenuItem::Standard(ksni::StandardItem {
                        label: format!("{}%", b),
                        activate: Box::new(move |tray: &mut Self| {
                            tray.set_brightness(b);
                        }),
                        ..Default::default()
                    })
                }).collect(),
                ..Default::default()
            }),

            MenuItem::Separator,

            // Toggle pause/resume
            MenuItem::Standard(ksni::StandardItem {
                label: if self.state.is_paused() {
                    "Resume".into()
                } else {
                    "Pause".into()
                },
                activate: Box::new(|tray: &mut Self| { tray.toggle(); }),
                ..Default::default()
            }),

            // Next effect
            MenuItem::Standard(ksni::StandardItem {
                label: "Next Effect".into(),
                activate: Box::new(|tray: &mut Self| { tray.next_effect(); }),
                ..Default::default()
            }),

            MenuItem::Separator,

            // Open web UI
            MenuItem::Standard(ksni::StandardItem {
                label: "Open Hypercolor".into(),
                activate: Box::new(|tray: &mut Self| { tray.open_web_ui(); }),
                ..Default::default()
            }),

            // Quit (stop daemon)
            MenuItem::Standard(ksni::StandardItem {
                label: "Quit".into(),
                activate: Box::new(|_: &mut Self| {
                    std::process::exit(0);
                }),
                ..Default::default()
            }),
        ]
    }

    /// Left-click opens the web UI
    fn activate(&mut self, _x: i32, _y: i32) {
        self.open_web_ui();
    }

    /// Scroll wheel adjusts brightness
    fn scroll(&mut self, delta: i32, orientation: ksni::ScrollOrientation) {
        if orientation == ksni::ScrollOrientation::Vertical {
            let new = (self.state.brightness as i32 + delta * 5).clamp(0, 100);
            self.set_brightness(new as u32);
        }
    }
}
```

### 4.2 Icon State Mapping

| Daemon State  | Icon                                | Tooltip                       | Tray Behavior      |
| ------------- | ----------------------------------- | ----------------------------- | ------------------ | ----------- | --------------- |
| Running       | `hypercolor-active` (animated glow) | "Aurora                       | 3 devices          | 60fps"      | Full color icon |
| Paused        | `hypercolor-paused` (dimmed)        | "Paused                       | 3 devices"         | Grayed icon |
| Error         | `hypercolor-error` (warning badge)  | "Error: Prism 8 disconnected" | Red badge overlay  |
| Idle          | `hypercolor-idle` (outline only)    | "No devices connected"        | Monochrome outline |
| Battery Saver | `hypercolor-active` + battery badge | "Battery saver: 15fps"        | Modified tooltip   |

### 4.3 Tray Binary

The tray runs as a separate lightweight binary (`hypercolor-tray`), not embedded in the daemon. It connects via D-Bus and subscribes to signals. This keeps the daemon headless and avoids pulling in GUI dependencies.

```
hypercolor-tray (ksni + zbus)
    |
    | D-Bus session bus
    |
    v
hypercolor-daemon (headless)
```

---

## 5. GNOME Integration

GNOME is the most popular Linux desktop and also the most opinionated. It deliberately removed the system tray in GNOME 3, requiring extensions for tray icon support. Native GNOME integration goes beyond a tray icon.

### 5.1 GNOME Shell Extension

A GNOME Shell extension provides native integration: Quick Settings tile, panel indicator, and desktop event hooks.

**Extension structure:**

```
hypercolor@hyperbliss.tech/
  metadata.json
  extension.js
  prefs.js
  stylesheet.css
  schemas/
    org.gnome.shell.extensions.hypercolor.gschema.xml
```

**metadata.json:**

```json
{
  "uuid": "hypercolor@hyperbliss.tech",
  "name": "Hypercolor",
  "description": "RGB lighting control for Hypercolor daemon",
  "shell-version": ["45", "46", "47", "48"],
  "url": "https://github.com/hyperb1iss/hypercolor"
}
```

**extension.js (GJS):**

```javascript
import Gio from "gi://Gio";
import GLib from "gi://GLib";
import St from "gi://St";
import * as Main from "resource:///org/gnome/shell/ui/main.js";
import * as PanelMenu from "resource:///org/gnome/shell/ui/panelMenu.js";
import * as PopupMenu from "resource:///org/gnome/shell/ui/popupMenu.js";
import * as QuickSettings from "resource:///org/gnome/shell/ui/quickSettings.js";
import { Extension } from "resource:///org/gnome/shell/extensions/extension.js";

const DBUS_NAME = "tech.hyperbliss.hypercolor1";
const DBUS_PATH = "/tech/hyperbliss/hypercolor1";
const DBUS_IFACE = "tech.hyperbliss.hypercolor1.Daemon";

// D-Bus proxy interface
const HypercolorIface = `
<node>
  <interface name="${DBUS_IFACE}">
    <property name="State" type="s" access="read"/>
    <property name="CurrentEffect" type="s" access="read"/>
    <property name="Brightness" type="u" access="readwrite"/>
    <property name="Fps" type="u" access="read"/>
    <property name="DeviceCount" type="u" access="read"/>
    <method name="Toggle"/>
    <method name="NextEffect"/>
    <method name="PreviousEffect"/>
    <method name="SetProfile"><arg name="profile_name" type="s" direction="in"/></method>
    <method name="ListProfiles"><arg name="profiles" type="as" direction="out"/></method>
    <method name="OpenWebUI"/>
    <signal name="EffectChanged">
      <arg name="effect_id" type="s"/>
      <arg name="effect_name" type="s"/>
    </signal>
    <signal name="DeviceConnected">
      <arg name="device_id" type="s"/>
      <arg name="device_name" type="s"/>
      <arg name="backend" type="s"/>
      <arg name="led_count" type="u"/>
    </signal>
    <signal name="DeviceDisconnected">
      <arg name="device_id" type="s"/>
      <arg name="device_name" type="s"/>
    </signal>
  </interface>
</node>`;

const HypercolorProxy = Gio.DBusProxy.makeProxyWrapper(HypercolorIface);

// --- Quick Settings Tile (GNOME 44+) ---

class HypercolorToggle extends QuickSettings.QuickToggle {
  static {
    GObject.registerClass(this);
  }

  constructor() {
    super({
      title: "Hypercolor",
      iconName: "hypercolor-symbolic",
      toggleMode: true,
    });
  }
}

class HypercolorMenuToggle extends QuickSettings.QuickMenuToggle {
  static {
    GObject.registerClass(this);
  }

  constructor(proxy) {
    super({
      title: "Hypercolor",
      subtitle: "No effect",
      iconName: "hypercolor-symbolic",
      toggleMode: true,
    });

    this._proxy = proxy;

    // Brightness slider in the expanded menu
    this._brightnessSlider = new QuickSettings.QuickSlider({
      iconName: "display-brightness-symbolic",
    });
    this._brightnessSlider.slider.connect("notify::value", () => {
      const brightness = Math.round(this._brightnessSlider.slider.value * 100);
      this._proxy.Brightness = brightness;
    });

    this.menu.addMenuItem(this._brightnessSlider);
    this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

    // Profile quick-switch items
    this._profileSection = new PopupMenu.PopupMenuSection();
    this.menu.addMenuItem(this._profileSection);

    this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

    // Open web UI
    this.menu.addMenuItem(
      new PopupMenu.PopupMenuItem("Open Hypercolor", {
        activate: () => this._proxy.OpenWebUIRemote(),
      }),
    );

    // Toggle handler
    this.connect("clicked", () => {
      this._proxy.ToggleRemote();
    });

    this._sync();
  }

  _sync() {
    const state = this._proxy.State;
    this.checked = state === "running";
    this.subtitle = this._proxy.CurrentEffect || "No effect";
    this._brightnessSlider.slider.value = (this._proxy.Brightness || 100) / 100;
  }
}

// --- Panel Indicator ---

class HypercolorIndicator extends PanelMenu.Button {
  static {
    GObject.registerClass(this);
  }

  constructor(proxy) {
    super(0.0, "Hypercolor");
    this._proxy = proxy;

    // Panel icon
    this._icon = new St.Icon({
      icon_name: "hypercolor-symbolic",
      style_class: "system-status-icon",
    });
    this.add_child(this._icon);

    // Dropdown menu
    this._buildMenu();

    // Subscribe to D-Bus property changes
    this._proxy.connect("g-properties-changed", () => this._updateState());
    this._updateState();
  }

  _buildMenu() {
    // Current effect (non-interactive label)
    this._effectLabel = new PopupMenu.PopupMenuItem("No effect", {
      reactive: false,
      style_class: "hypercolor-effect-label",
    });
    this.menu.addMenuItem(this._effectLabel);

    this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

    // Toggle
    this._toggleItem = new PopupMenu.PopupSwitchMenuItem("Active", true);
    this._toggleItem.connect("toggled", () => {
      this._proxy.ToggleRemote();
    });
    this.menu.addMenuItem(this._toggleItem);

    // Next / Previous effect
    const navSection = new PopupMenu.PopupMenuSection();
    navSection.addMenuItem(
      new PopupMenu.PopupMenuItem("Next Effect", {
        activate: () => this._proxy.NextEffectRemote(),
      }),
    );
    navSection.addMenuItem(
      new PopupMenu.PopupMenuItem("Previous Effect", {
        activate: () => this._proxy.PreviousEffectRemote(),
      }),
    );
    this.menu.addMenuItem(navSection);

    this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

    // Open web UI
    this.menu.addMenuItem(
      new PopupMenu.PopupMenuItem("Open Hypercolor", {
        activate: () => this._proxy.OpenWebUIRemote(),
      }),
    );
  }

  _updateState() {
    const state = this._proxy.State;
    const effect = this._proxy.CurrentEffect;

    this._icon.icon_name =
      state === "running"
        ? "hypercolor-active-symbolic"
        : "hypercolor-paused-symbolic";

    this._effectLabel.label.text = effect || "No effect";
    this._toggleItem.setToggleState(state === "running");
  }
}

// --- Extension Lifecycle ---

export default class HypercolorExtension extends Extension {
  enable() {
    this._proxy = new HypercolorProxy(Gio.DBus.session, DBUS_NAME, DBUS_PATH);

    // Panel indicator
    this._indicator = new HypercolorIndicator(this._proxy);
    Main.panel.addToStatusArea("hypercolor", this._indicator);

    // Quick Settings toggle (GNOME 44+)
    this._quickToggle = new HypercolorMenuToggle(this._proxy);
    Main.panel.statusArea.quickSettings.addExternalIndicator(this._quickToggle);

    // --- Desktop Event Hooks ---

    // Workspace change -> profile trigger
    this._workspaceSignal = global.workspace_manager.connect(
      "active-workspace-changed",
      () => this._onWorkspaceChanged(),
    );

    // Screen lock -> dim
    this._screenShield = Main.screenShield;
    this._lockSignal = this._screenShield?.connect("active-changed", () =>
      this._onLockChanged(),
    );

    // Night Light sync
    this._nightLightProxy = new Gio.DBusProxy.new_sync(
      Gio.DBus.session,
      Gio.DBusProxyFlags.NONE,
      null,
      "org.gnome.SettingsDaemon.Color",
      "/org/gnome/SettingsDaemon/Color",
      "org.gnome.SettingsDaemon.Color",
      null,
    );
    this._nightLightSignal = this._nightLightProxy?.connect(
      "g-properties-changed",
      () => this._onNightLightChanged(),
    );
  }

  disable() {
    this._indicator?.destroy();
    this._indicator = null;

    this._quickToggle?.destroy();
    this._quickToggle = null;

    if (this._workspaceSignal) {
      global.workspace_manager.disconnect(this._workspaceSignal);
    }
    if (this._lockSignal && this._screenShield) {
      this._screenShield.disconnect(this._lockSignal);
    }
    if (this._nightLightSignal && this._nightLightProxy) {
      this._nightLightProxy.disconnect(this._nightLightSignal);
    }

    this._proxy = null;
  }

  _onWorkspaceChanged() {
    const index = global.workspace_manager.get_active_workspace_index();
    // Emit workspace index to daemon for profile mapping
    // The daemon's config maps workspace indices to profiles:
    //   [workspace_profiles]
    //   0 = "coding"
    //   1 = "gaming"
    //   2 = "chill"
    this._proxy.SetProfileRemote(`workspace:${index}`);
  }

  _onLockChanged() {
    const locked = this._screenShield?.active;
    if (locked) {
      // Save current brightness, dim to nightlight level
      this._proxy.SetProfileRemote("__locked__");
    } else {
      // Restore previous state
      this._proxy.SetProfileRemote("__restore__");
    }
  }

  _onNightLightChanged() {
    const temperature =
      this._nightLightProxy.get_cached_property("Temperature");
    if (temperature) {
      const kelvin = temperature.unpack();
      // Sync color temperature to the daemon for warm-shift effects
      // The daemon adjusts effect output based on this value
    }
  }
}
```

### 5.2 GNOME Night Light Sync

GNOME's Night Light shifts the screen color temperature on a schedule. Hypercolor can read this value and apply a matching warm shift to the lighting.

```
D-Bus: org.gnome.SettingsDaemon.Color
Path:  /org/gnome/SettingsDaemon/Color
Property: Temperature (uint32, Kelvin -- 6500 = neutral, 3500 = warm)
Property: NightLightActive (boolean)
```

The daemon monitors this property and applies a color temperature multiplier to the final LED output:

```rust
pub fn apply_color_temperature(color: Rgb, kelvin: u32) -> Rgb {
    // Tanner Helland's algorithm for RGB from color temperature
    let temp = kelvin as f64 / 100.0;
    let (r_mult, g_mult, b_mult) = if temp <= 66.0 {
        let r = 1.0;
        let g = (99.4708025861 * temp.ln() - 161.1195681661).clamp(0.0, 255.0) / 255.0;
        let b = if temp <= 19.0 {
            0.0
        } else {
            (138.5177312231 * (temp - 10.0).ln() - 305.0447927307).clamp(0.0, 255.0) / 255.0
        };
        (r, g, b)
    } else {
        let r = (329.698727446 * (temp - 60.0).powf(-0.1332047592)).clamp(0.0, 255.0) / 255.0;
        let g = (288.1221695283 * (temp - 60.0).powf(-0.0755148492)).clamp(0.0, 255.0) / 255.0;
        let b = 1.0;
        (r, g, b)
    };

    Rgb {
        r: (color.r as f64 * r_mult) as u8,
        g: (color.g as f64 * g_mult) as u8,
        b: (color.b as f64 * b_mult) as u8,
    }
}
```

### 5.3 GNOME Do Not Disturb Sync

When the user enables Do Not Disturb in GNOME, Hypercolor suppresses all non-critical notifications:

```
D-Bus: org.gnome.desktop.notifications
GSettings key: show-banners (boolean)
```

The notification module checks this before sending any notification:

```rust
pub async fn should_notify(&self) -> bool {
    let settings = gio::Settings::new("org.gnome.desktop.notifications");
    settings.boolean("show-banners")
}
```

---

## 6. KDE Plasma Integration

KDE Plasma is the second most popular Linux desktop. It has the richest integration APIs of any Linux DE and native StatusNotifierItem support.

### 6.1 System Tray

KDE Plasma natively supports StatusNotifierItem. The `hypercolor-tray` binary (section 4) works out of the box -- no extension or plugin required.

### 6.2 Plasma Widget (QML)

A Plasma widget (plasmoid) for the panel or desktop provides quick effect switching and a mini preview.

**Directory structure:**

```
org.hyperbliss.hypercolor/
  metadata.json
  contents/
    ui/
      main.qml
      CompactRepresentation.qml
      FullRepresentation.qml
    config/
      main.xml
```

**metadata.json:**

```json
{
  "KPlugin": {
    "Id": "org.hyperbliss.hypercolor",
    "Name": "Hypercolor",
    "Description": "RGB lighting control",
    "Icon": "hypercolor",
    "Category": "System Information",
    "Authors": [{ "Name": "Hyperbliss", "Email": "hyperb1iss@gmail.com" }],
    "Website": "https://github.com/hyperb1iss/hypercolor"
  },
  "X-Plasma-API": "declarativeappletscript",
  "X-Plasma-MainScript": "ui/main.qml",
  "KPackageStructure": "Plasma/Applet"
}
```

**main.qml:**

```qml
import QtQuick 2.15
import org.kde.plasma.plasmoid 2.0
import org.kde.plasma.core 2.0 as PlasmaCore

PlasmoidItem {
    id: root

    // D-Bus connection to Hypercolor daemon
    PlasmaCore.DataSource {
        id: hypercolorSource
        engine: "executable"
        connectedSources: ["busctl --user get-property tech.hyperbliss.hypercolor1 /tech/hyperbliss/hypercolor1 tech.hyperbliss.hypercolor1.Daemon CurrentEffect"]
        interval: 2000
    }

    compactRepresentation: CompactRepresentation {}
    fullRepresentation: FullRepresentation {}

    toolTipMainText: "Hypercolor"
    toolTipSubText: currentEffect || "No effect"

    property string currentEffect: ""
    property int brightness: 100
    property string state: "running"
}
```

**CompactRepresentation.qml** (panel icon):

```qml
import QtQuick 2.15
import org.kde.plasma.core 2.0 as PlasmaCore

PlasmaCore.IconItem {
    source: root.state === "running" ? "hypercolor-active" : "hypercolor-paused"

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton | Qt.MiddleButton
        onClicked: (mouse) => {
            if (mouse.button === Qt.LeftButton) {
                root.expanded = !root.expanded;
            } else if (mouse.button === Qt.MiddleButton) {
                hypercolorToggle();
            }
        }
        onWheel: (wheel) => {
            // Scroll to adjust brightness
            const delta = wheel.angleDelta.y > 0 ? 5 : -5;
            hypercolorSetBrightness(root.brightness + delta);
        }
    }
}
```

**FullRepresentation.qml** (expanded panel widget):

```qml
import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import org.kde.plasma.components 3.0 as PC3
import org.kde.plasma.extras 2.0 as PlasmaExtras

PlasmaExtras.Representation {
    implicitWidth: PlasmaCore.Units.gridUnit * 20
    implicitHeight: PlasmaCore.Units.gridUnit * 16

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: PlasmaCore.Units.smallSpacing

        // Header: current effect
        PlasmaExtras.Heading {
            level: 3
            text: root.currentEffect || "No Effect"
        }

        // Brightness slider
        RowLayout {
            PC3.Label { text: "Brightness" }
            PC3.Slider {
                from: 0; to: 100
                value: root.brightness
                onMoved: hypercolorSetBrightness(value)
            }
            PC3.Label { text: root.brightness + "%" }
        }

        PC3.ToolSeparator { Layout.fillWidth: true }

        // Effect list (scrollable)
        PC3.ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ListView {
                id: effectList
                model: effectModel
                delegate: PC3.ItemDelegate {
                    width: effectList.width
                    text: model.name
                    highlighted: model.id === root.currentEffect
                    onClicked: hypercolorSetEffect(model.id)
                }
            }
        }

        // Profile selector
        PC3.ComboBox {
            Layout.fillWidth: true
            model: profileModel
            onActivated: hypercolorSetProfile(currentText)
        }

        // Action buttons
        RowLayout {
            PC3.Button {
                icon.name: root.state === "running" ? "media-playback-pause" : "media-playback-start"
                onClicked: hypercolorToggle()
            }
            PC3.Button {
                icon.name: "media-skip-backward"
                onClicked: hypercolorPreviousEffect()
            }
            PC3.Button {
                icon.name: "media-skip-forward"
                onClicked: hypercolorNextEffect()
            }
            Item { Layout.fillWidth: true }
            PC3.Button {
                text: "Open"
                icon.name: "preferences-system"
                onClicked: hypercolorOpenWebUI()
            }
        }
    }
}
```

### 6.3 KDE Connect Integration

KDE Connect lets Android/iOS phones communicate with the desktop. Hypercolor can register as a KDE Connect plugin to allow phone-based lighting control.

**Integration points:**

- **Phone as remote:** Send D-Bus commands from KDE Connect's "Run Command" plugin. Pre-configure commands like "Next Effect", "Toggle", "Gaming Profile".
- **Phone battery:** When phone charges wirelessly on the desk, trigger a "charging" lighting effect via KDE Connect's battery plugin.
- **Media sync:** KDE Connect shares media player state. Sync lighting to now-playing album art colors.
- **Telephony:** Incoming call notification can trigger a subtle pulse effect.

**KDE Connect command configuration:**

```json
{
  "commands": {
    "hypercolor-toggle": {
      "name": "Toggle Lighting",
      "command": "hypercolor toggle"
    },
    "hypercolor-next": {
      "name": "Next Effect",
      "command": "hypercolor next"
    },
    "hypercolor-gaming": {
      "name": "Gaming Mode",
      "command": "hypercolor profile gaming"
    },
    "hypercolor-chill": {
      "name": "Chill Mode",
      "command": "hypercolor profile chill"
    }
  }
}
```

### 6.4 KDE Color Scheme Sync

When the user changes the KDE color scheme (Breeze Dark, Catppuccin, Nord), Hypercolor can extract the accent color and apply it to a "Desktop Sync" profile.

```
D-Bus: org.freedesktop.portal.Settings
Method: Read("org.freedesktop.appearance", "accent-color")
Signal: SettingChanged  (fires when user changes theme)
```

```rust
/// Read the KDE/GNOME accent color via the portal
pub async fn get_accent_color(connection: &Connection) -> Option<(f64, f64, f64)> {
    let proxy = zbus::Proxy::new(
        connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    ).await.ok()?;

    let color: (f64, f64, f64) = proxy.call(
        "Read",
        &("org.freedesktop.appearance", "accent-color"),
    ).await.ok()?;

    Some(color)
}
```

---

## 7. Hyprland / wlroots Compositors

Hyprland, sway, river, labwc, and other wlroots-based compositors are popular among power users. These users value CLI control, keybindings, and bar module integration over GUIs.

### 7.1 Waybar Module

Waybar is the de facto status bar for Hyprland and sway. Hypercolor integrates as a custom module.

**Waybar configuration (`~/.config/waybar/config`):**

```jsonc
{
  "modules-right": [
    "custom/hypercolor",
    // ... other modules
  ],

  "custom/hypercolor": {
    "exec": "hypercolor waybar",
    "return-type": "json",
    "interval": 2,
    "on-click": "hypercolor open",
    "on-click-right": "hypercolor toggle",
    "on-click-middle": "hypercolor next",
    "on-scroll-up": "hypercolor brightness +5",
    "on-scroll-down": "hypercolor brightness -5",
    "tooltip": true,
  },
}
```

**Waybar output format (JSON):**

The `hypercolor waybar` command outputs Waybar-compatible JSON on stdout:

```rust
/// Called by `hypercolor waybar` subcommand
pub fn waybar_output(state: &DaemonState) -> String {
    let (icon, class) = match state.status {
        Status::Running => ("", "active"),
        Status::Paused  => ("", "paused"),
        Status::Error   => ("", "error"),
        Status::Idle    => ("", "idle"),
    };

    let text = match &state.current_effect {
        Some(effect) => format!("{} {}", icon, effect),
        None => format!("{} --", icon),
    };

    let tooltip = format!(
        "Hypercolor\nEffect: {}\nDevices: {}\nFPS: {}\nBrightness: {}%",
        state.current_effect.as_deref().unwrap_or("None"),
        state.device_count,
        state.fps,
        state.brightness,
    );

    serde_json::json!({
        "text": text,
        "tooltip": tooltip,
        "class": class,
        "alt": state.status.to_string(),
    }).to_string()
}
```

**Waybar stylesheet (`~/.config/waybar/style.css`):**

```css
#custom-hypercolor {
  padding: 0 8px;
  font-family: "JetBrains Mono", monospace;
}

#custom-hypercolor.active {
  color: #e135ff; /* SilkCircuit Electric Purple */
}

#custom-hypercolor.paused {
  color: #f1fa8c; /* SilkCircuit Electric Yellow */
}

#custom-hypercolor.error {
  color: #ff6363; /* SilkCircuit Error Red */
}

#custom-hypercolor.idle {
  color: #6272a4;
}
```

### 7.2 Hyprland IPC Events

Hyprland exposes a Unix socket IPC that emits events for workspace changes, window focus, fullscreen state, and more. Hypercolor subscribes to these events for reactive desktop integration.

```rust
use tokio::net::UnixStream;
use tokio::io::{AsyncBufReadExt, BufReader};

pub struct HyprlandWatcher {
    bus: HypercolorBus,
}

impl HyprlandWatcher {
    pub async fn connect_and_watch(&self) -> Result<()> {
        let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")?;
        let socket_path = format!(
            "/tmp/hypr/{}/.socket2.sock",
            signature,
        );

        let stream = UnixStream::connect(&socket_path).await?;
        let reader = BufReader::new(stream);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await? {
            self.handle_event(&line).await;
        }

        Ok(())
    }

    async fn handle_event(&self, event: &str) {
        // Hyprland events: "eventname>>data"
        let Some((event_name, data)) = event.split_once(">>") else {
            return;
        };

        match event_name {
            "workspace" => {
                // Workspace switched: data = workspace name/id
                let workspace = data.trim();
                tracing::debug!(workspace, "Hyprland workspace changed");
                self.bus.trigger_workspace_profile(workspace).await;
            }

            "activewindow" => {
                // Active window changed: data = "class,title"
                if let Some((class, title)) = data.split_once(',') {
                    tracing::debug!(class, title, "Hyprland active window changed");
                    self.bus.trigger_window_context(class, title).await;
                }
            }

            "fullscreen" => {
                // Fullscreen state: data = "0" or "1"
                let is_fullscreen = data.trim() == "1";
                tracing::debug!(is_fullscreen, "Hyprland fullscreen changed");
                if is_fullscreen {
                    self.bus.trigger_gaming_mode().await;
                } else {
                    self.bus.trigger_restore_mode().await;
                }
            }

            "activespecial" => {
                // Special workspace (scratchpad) activated
                tracing::debug!("Hyprland special workspace activated");
            }

            "monitoradded" | "monitorremoved" => {
                // Monitor hotplug
                self.bus.trigger_monitor_change().await;
            }

            "submap" => {
                // Keybind submap changed
                let submap = data.trim();
                tracing::debug!(submap, "Hyprland submap changed");
            }

            _ => {} // Ignore unknown events
        }
    }
}
```

**Hyprland keybind configuration (`~/.config/hypr/hyprland.conf`):**

```ini
# Hypercolor keybinds
bind = SUPER, L, exec, hypercolor next
bind = SUPER SHIFT, L, exec, hypercolor previous
bind = SUPER, bracketright, exec, hypercolor brightness +10
bind = SUPER, bracketleft, exec, hypercolor brightness -10
bind = SUPER SHIFT, P, exec, hypercolor toggle

# Submap for effect selection (enter with SUPER+E, navigate with numbers)
bind = SUPER, E, submap, hypercolor
submap = hypercolor
bind = , 1, exec, hypercolor profile coding
bind = , 1, submap, reset
bind = , 2, exec, hypercolor profile gaming
bind = , 2, submap, reset
bind = , 3, exec, hypercolor profile chill
bind = , 3, submap, reset
bind = , escape, submap, reset
submap = reset
```

### 7.3 sway/i3 Compatibility

sway and i3 share the same IPC protocol. The Hypercolor watcher detects which is running:

```rust
pub enum WmType {
    Hyprland,
    Sway,
    I3,
    Unknown,
}

pub fn detect_wm() -> WmType {
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        WmType::Hyprland
    } else if std::env::var("SWAYSOCK").is_ok() {
        WmType::Sway
    } else if std::env::var("I3SOCK").is_ok() {
        WmType::I3
    } else {
        WmType::Unknown
    }
}
```

### 7.4 wlr-layer-shell (Optional Overlay)

For users who want a floating overlay panel (e.g., a quick-access brightness slider), Hypercolor can use `wlr-layer-shell` via the `smithay-client-toolkit` crate. This is a stretch goal -- most tiling WM users prefer CLI + keybinds.

---

## 8. i3/sway Bar Integration

### 8.1 i3status/i3blocks Module

**i3blocks configuration (`~/.config/i3blocks/config`):**

```ini
[hypercolor]
command=hypercolor i3blocks
interval=persist
markup=pango
signal=10
# SIGUSR1 (signal=10) triggers immediate update
```

**i3blocks output:**

```rust
/// Called by `hypercolor i3blocks` subcommand
/// Outputs i3blocks-compatible format on stdout, persists (long-running)
pub async fn i3blocks_output(bus: &HypercolorBus) -> Result<()> {
    let mut rx = bus.events.subscribe();

    // Initial output
    print_i3blocks_line(bus).await;

    // Update on every relevant event
    loop {
        match rx.recv().await {
            Ok(HypercolorEvent::EffectChanged(_))
            | Ok(HypercolorEvent::ProfileLoaded(_))
            | Ok(HypercolorEvent::DeviceConnected(_))
            | Ok(HypercolorEvent::DeviceDisconnected(_)) => {
                print_i3blocks_line(bus).await;
            }
            _ => {}
        }
    }
}

async fn print_i3blocks_line(bus: &HypercolorBus) {
    let state = bus.state();
    let effect = state.current_effect.as_deref().unwrap_or("--");
    let color = match state.status {
        Status::Running => "#e135ff",  // Electric Purple
        Status::Paused  => "#f1fa8c",  // Electric Yellow
        Status::Error   => "#ff6363",  // Error Red
        Status::Idle    => "#6272a4",
    };

    // i3blocks format: full_text\nshort_text\ncolor
    println!(
        "<span color='{}'> {}</span>",
        color, effect,
    );
    println!(" {}", effect);
    println!("{}", color);
}
```

### 8.2 Polybar Module

```ini
; ~/.config/polybar/config.ini
[module/hypercolor]
type = custom/script
exec = hypercolor polybar
tail = true
click-left = hypercolor open
click-right = hypercolor toggle
click-middle = hypercolor next
scroll-up = hypercolor brightness +5
scroll-down = hypercolor brightness -5

format-prefix = " "
format-prefix-foreground = #e135ff
```

**Polybar output:**

```rust
/// Called by `hypercolor polybar` subcommand
/// Outputs single-line Polybar format, tailing mode
pub async fn polybar_output(bus: &HypercolorBus) -> Result<()> {
    let mut rx = bus.events.subscribe();

    print_polybar_line(bus).await;

    loop {
        if rx.recv().await.is_ok() {
            print_polybar_line(bus).await;
        }
    }
}

async fn print_polybar_line(bus: &HypercolorBus) {
    let state = bus.state();
    let effect = state.current_effect.as_deref().unwrap_or("--");
    let color = match state.status {
        Status::Running => "%{F#e135ff}",
        Status::Paused  => "%{F#f1fa8c}",
        Status::Error   => "%{F#ff6363}",
        Status::Idle    => "%{F#6272a4}",
    };

    println!("{}{} {}%{{F-}}", color, "", effect);
}
```

### 8.3 i3/sway IPC Events

```rust
use swayipc_async::{Connection, EventType, Event};

pub struct SwayWatcher {
    bus: HypercolorBus,
}

impl SwayWatcher {
    pub async fn connect_and_watch(&self) -> Result<()> {
        let subs = [
            EventType::Workspace,
            EventType::Window,
            EventType::Output,
        ];

        let mut events = Connection::new().await?.subscribe(subs).await?;

        while let Some(event) = events.next().await {
            match event? {
                Event::Workspace(ws) => {
                    if let Some(current) = ws.current {
                        let name = current.name.unwrap_or_default();
                        tracing::debug!(workspace = %name, "sway workspace changed");
                        self.bus.trigger_workspace_profile(&name).await;
                    }
                }

                Event::Window(win) => {
                    match win.change {
                        swayipc_async::WindowChange::Focus => {
                            if let Some(container) = win.container {
                                let app_id = container.app_id.unwrap_or_default();
                                let name = container.name.unwrap_or_default();
                                self.bus.trigger_window_context(&app_id, &name).await;
                            }
                        }
                        swayipc_async::WindowChange::Fullscreen => {
                            let is_fs = win.container
                                .map(|c| c.fullscreen_mode.unwrap_or(0) > 0)
                                .unwrap_or(false);
                            if is_fs {
                                self.bus.trigger_gaming_mode().await;
                            } else {
                                self.bus.trigger_restore_mode().await;
                            }
                        }
                        _ => {}
                    }
                }

                Event::Output(_) => {
                    self.bus.trigger_monitor_change().await;
                }

                _ => {}
            }
        }

        Ok(())
    }
}
```

**i3/sway keybind configuration:**

```
# ~/.config/sway/config (or ~/.config/i3/config)
bindsym $mod+l exec hypercolor next
bindsym $mod+Shift+l exec hypercolor previous
bindsym $mod+bracketright exec hypercolor brightness +10
bindsym $mod+bracketleft exec hypercolor brightness -10
bindsym $mod+Shift+p exec hypercolor toggle

# Mode for effect selection
mode "hypercolor" {
    bindsym 1 exec hypercolor profile coding; mode "default"
    bindsym 2 exec hypercolor profile gaming; mode "default"
    bindsym 3 exec hypercolor profile chill; mode "default"
    bindsym Escape mode "default"
}
bindsym $mod+e mode "hypercolor"
```

---

## 9. COSMIC Desktop (System76)

COSMIC is System76's new Rust-native desktop environment built on iced. As a Rust project integrating with a Rust DE, the fit is natural.

### 9.1 COSMIC Applet

COSMIC applets are small iced applications that live in the panel. They follow the `cosmic-applet` crate API.

```rust
use cosmic::app::{Core, Task};
use cosmic::applet::{self, menu_button};
use cosmic::iced::{
    widget::{column, row, slider, text, toggler},
    Length, Subscription,
};
use cosmic::widget::{self, container};
use cosmic::{Element, Theme};
use zbus::Connection;

pub struct HypercolorApplet {
    core: Core,
    state: DaemonState,
    connection: Option<Connection>,
    popup: Option<cosmic::iced::window::Id>,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    Toggle,
    NextEffect,
    PreviousEffect,
    SetBrightness(u32),
    SetProfile(String),
    OpenWebUI,
    StateUpdated(DaemonState),
    DbusEvent(HypercolorEvent),
}

impl cosmic::Application for HypercolorApplet {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "tech.hyperbliss.hypercolor.applet";

    fn core(&self) -> &Core { &self.core }
    fn core_mut(&mut self) -> &mut Core { &mut self.core }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let app = Self {
            core,
            state: DaemonState::default(),
            connection: None,
            popup: None,
        };

        (app, Task::none())
    }

    fn view(&self) -> Element<Self::Message> {
        // Panel icon
        let icon = match self.state.status {
            Status::Running => "hypercolor-active-symbolic",
            Status::Paused  => "hypercolor-paused-symbolic",
            Status::Error   => "hypercolor-error-symbolic",
            _ => "hypercolor-symbolic",
        };

        applet::icon_button(icon)
            .on_press(Message::TogglePopup)
            .into()
    }

    fn view_popup(&self, _id: cosmic::iced::window::Id) -> Element<Self::Message> {
        let effect_label = text(
            self.state.current_effect.as_deref().unwrap_or("No Effect")
        ).size(16);

        let brightness_row = row![
            text("Brightness").width(Length::Fill),
            slider(0..=100, self.state.brightness, Message::SetBrightness),
            text(format!("{}%", self.state.brightness)),
        ].spacing(8);

        let toggle = toggler(
            "Active".into(),
            self.state.status == Status::Running,
            |_| Message::Toggle,
        );

        let nav_row = row![
            widget::button::text("Prev").on_press(Message::PreviousEffect),
            widget::button::text("Next").on_press(Message::NextEffect),
        ].spacing(8);

        let open_button = widget::button::text("Open Hypercolor")
            .on_press(Message::OpenWebUI);

        container(
            column![
                effect_label,
                cosmic::widget::divider::horizontal::default(),
                brightness_row,
                toggle,
                nav_row,
                cosmic::widget::divider::horizontal::default(),
                open_button,
            ]
            .spacing(12)
            .padding(16)
        )
        .width(Length::Fixed(280.0))
        .into()
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::TogglePopup => {
                if let Some(popup) = self.popup.take() {
                    cosmic::iced::window::close(popup)
                } else {
                    let (id, task) = applet::get_popup(self);
                    self.popup = Some(id);
                    task
                }
            }
            Message::Toggle => {
                self.dbus_call("Toggle");
                Task::none()
            }
            Message::NextEffect => {
                self.dbus_call("NextEffect");
                Task::none()
            }
            Message::PreviousEffect => {
                self.dbus_call("PreviousEffect");
                Task::none()
            }
            Message::SetBrightness(val) => {
                self.state.brightness = val;
                self.dbus_set_property("Brightness", val);
                Task::none()
            }
            Message::SetProfile(name) => {
                self.dbus_call_with_arg("SetProfile", &name);
                Task::none()
            }
            Message::OpenWebUI => {
                self.dbus_call("OpenWebUI");
                Task::none()
            }
            Message::StateUpdated(state) => {
                self.state = state;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        // Subscribe to D-Bus signals for real-time updates
        cosmic::iced::subscription::unfold(
            "hypercolor-dbus",
            (),
            |()| async {
                // Connect to D-Bus, monitor signals, yield Messages
                // ... (implementation omitted for brevity)
                (Message::StateUpdated(DaemonState::default()), ())
            },
        )
    }
}
```

### 9.2 COSMIC Settings Panel

COSMIC has a modular settings app. Hypercolor can register as a settings page for hardware configuration:

```
/usr/share/cosmic/settings/pages/hypercolor.ron
```

This is a stretch goal -- the web UI is the primary configuration interface. The COSMIC settings page would link out to the web UI rather than duplicating all controls.

---

## 10. Power Management

### 10.1 UPower Integration

Monitor battery state and power supply changes via UPower's D-Bus interface:

```rust
use zbus::Connection;

pub struct PowerWatcher {
    bus: HypercolorBus,
    config: Arc<RwLock<PowerConfig>>,
}

#[derive(Debug, Clone)]
pub struct PowerConfig {
    /// Reduce FPS when on battery
    pub battery_fps: u32,           // Default: 15
    /// Pause rendering below this battery percentage
    pub pause_below: u8,            // Default: 10
    /// Dim brightness on battery (percentage of normal)
    pub battery_brightness: u32,    // Default: 50
    /// Enable power management
    pub enabled: bool,              // Default: true
}

impl PowerWatcher {
    pub async fn start_monitoring(&self) -> Result<()> {
        let connection = Connection::system().await?;

        let proxy = zbus::Proxy::new(
            &connection,
            "org.freedesktop.UPower",
            "/org/freedesktop/UPower",
            "org.freedesktop.UPower",
        ).await?;

        // Get the display device (composite battery)
        let display_device: zbus::zvariant::OwnedObjectPath = proxy
            .get_property("DisplayDevice").await?;

        let device_proxy = zbus::Proxy::new(
            &connection,
            "org.freedesktop.UPower",
            display_device.as_str(),
            "org.freedesktop.UPower.Device",
        ).await?;

        // Monitor property changes
        let mut stream = device_proxy.receive_all_signals().await?;

        loop {
            if stream.next().await.is_some() {
                self.check_power_state(&device_proxy).await;
            }
        }
    }

    async fn check_power_state(&self, proxy: &zbus::Proxy<'_>) {
        let config = self.config.read().await;
        if !config.enabled { return; }

        // UPower State: 1=Charging, 2=Discharging, 3=Empty, 4=Full, 5=PendingCharge
        let state: u32 = proxy.get_property("State").await.unwrap_or(4);
        let percentage: f64 = proxy.get_property("Percentage").await.unwrap_or(100.0);

        let on_battery = state == 2; // Discharging

        if on_battery {
            if percentage < config.pause_below as f64 {
                tracing::info!(
                    percentage,
                    "Battery critically low, pausing rendering"
                );
                self.bus.pause().await;
                return;
            }

            tracing::info!(
                fps = config.battery_fps,
                brightness = config.battery_brightness,
                "On battery, reducing power usage"
            );
            self.bus.set_target_fps(config.battery_fps).await;
            self.bus.set_brightness(config.battery_brightness).await;
        } else {
            // Restore normal settings
            self.bus.restore_power_settings().await;
        }
    }
}
```

### 10.2 System Suspend / Resume

Integrate with systemd's `sleep` target via `logind` D-Bus:

```rust
pub struct SleepWatcher {
    bus: HypercolorBus,
}

impl SleepWatcher {
    pub async fn start_monitoring(&self) -> Result<()> {
        let connection = Connection::system().await?;

        let proxy = zbus::Proxy::new(
            &connection,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        ).await?;

        // Take a sleep inhibitor lock so we get notified before sleep
        let fd: std::os::unix::io::OwnedFd = proxy.call(
            "Inhibit",
            &("sleep", "Hypercolor", "Save lighting state before suspend", "delay"),
        ).await?;

        // Listen for PrepareForSleep signal
        let mut stream = proxy.receive_signal("PrepareForSleep").await?;

        loop {
            if let Some(signal) = stream.next().await {
                let args = signal.args::<(bool,)>()?;
                let suspending = args.0;

                if suspending {
                    tracing::info!("System suspending, saving state and shutting down devices");
                    self.bus.save_state().await;
                    self.bus.shutdown_devices_gracefully().await;
                    // Release the inhibitor lock to let suspend proceed
                    drop(fd);
                } else {
                    tracing::info!("System resumed, restoring state");
                    self.bus.restore_state().await;
                    self.bus.reconnect_devices().await;
                }
            }
        }
    }
}
```

### 10.3 Lid Close Detection

Monitor lid state for laptops:

```rust
pub async fn monitor_lid_state(bus: &HypercolorBus) -> Result<()> {
    let connection = Connection::system().await?;

    let proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower",
        "org.freedesktop.UPower",
    ).await?;

    let mut stream = proxy.receive_property_changed("LidIsClosed").await;

    while let Some(_) = stream.next().await {
        let lid_closed: bool = proxy.get_property("LidIsClosed").await?;

        if lid_closed {
            tracing::info!("Lid closed, dimming lighting");
            bus.trigger_profile("__lid_closed__").await;
        } else {
            tracing::info!("Lid opened, restoring lighting");
            bus.trigger_profile("__restore__").await;
        }
    }

    Ok(())
}
```

---

## 11. Window Manager Events -- Reactive Desktop

The daemon's WM integration module detects the running compositor/WM and subscribes to relevant events. All DE-specific watchers share a common `DesktopEvent` abstraction.

### 11.1 Desktop Event Abstraction

```rust
/// Unified desktop events from any WM/compositor
#[derive(Debug, Clone)]
pub enum DesktopEvent {
    WorkspaceChanged {
        workspace: String,
        index: Option<u32>,
    },
    ActiveWindowChanged {
        app_id: String,
        title: String,
        class: String,
    },
    FullscreenEntered {
        app_id: String,
    },
    FullscreenExited,
    ScreenLocked,
    ScreenUnlocked,
    MonitorConnected {
        name: String,
    },
    MonitorDisconnected {
        name: String,
    },
    IdleStateChanged {
        idle: bool,
    },
}

/// Profile trigger rules in config.toml
/// The daemon evaluates these rules against incoming DesktopEvents
///
/// [triggers]
///
/// [[triggers.workspace]]
/// workspace = "1"
/// profile = "coding"
///
/// [[triggers.workspace]]
/// workspace = "2"
/// profile = "gaming"
///
/// [[triggers.window]]
/// app_id = "steam"
/// profile = "gaming"
///
/// [[triggers.window]]
/// app_id = "firefox"
/// title = ".*YouTube.*"
/// profile = "media"
///
/// [[triggers.fullscreen]]
/// profile = "gaming"
///
/// [[triggers.idle]]
/// timeout_seconds = 300
/// profile = "nightlight"
///
/// [[triggers.time]]
/// start = "22:00"
/// end = "06:00"
/// profile = "nightshift"
```

### 11.2 Desktop Watcher Orchestrator

```rust
pub struct DesktopIntegration {
    bus: HypercolorBus,
    config: Arc<RwLock<Config>>,
}

impl DesktopIntegration {
    /// Detect the running DE/WM and start appropriate watchers
    pub async fn start(&self) -> Result<()> {
        let wm = detect_wm();
        let de = detect_de();

        tracing::info!(?wm, ?de, "Desktop environment detected");

        let mut tasks: Vec<tokio::task::JoinHandle<Result<()>>> = Vec::new();

        // WM-specific event watcher
        match wm {
            WmType::Hyprland => {
                let watcher = HyprlandWatcher::new(self.bus.clone());
                tasks.push(tokio::spawn(async move { watcher.connect_and_watch().await }));
            }
            WmType::Sway | WmType::I3 => {
                let watcher = SwayWatcher::new(self.bus.clone());
                tasks.push(tokio::spawn(async move { watcher.connect_and_watch().await }));
            }
            _ => {
                tracing::info!("No WM-specific event watcher available");
            }
        }

        // Power management (always active)
        let power = PowerWatcher::new(self.bus.clone(), self.config.clone());
        tasks.push(tokio::spawn(async move { power.start_monitoring().await }));

        // Sleep/suspend watcher (always active)
        let sleep = SleepWatcher::new(self.bus.clone());
        tasks.push(tokio::spawn(async move { sleep.start_monitoring().await }));

        // Idle detection via org.freedesktop.ScreenSaver
        let idle = IdleWatcher::new(self.bus.clone(), self.config.clone());
        tasks.push(tokio::spawn(async move { idle.start_monitoring().await }));

        // Accent color watcher (GNOME/KDE via portal)
        let accent = AccentColorWatcher::new(self.bus.clone());
        tasks.push(tokio::spawn(async move { accent.start_monitoring().await }));

        // Wait for all watchers (they run indefinitely)
        futures::future::try_join_all(tasks).await?;

        Ok(())
    }
}

fn detect_de() -> DesktopEnvironment {
    let xdg_desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let session = std::env::var("XDG_SESSION_DESKTOP").unwrap_or_default();

    match xdg_desktop.to_uppercase().as_str() {
        "GNOME" => DesktopEnvironment::Gnome,
        "KDE" => DesktopEnvironment::Kde,
        "COSMIC" => DesktopEnvironment::Cosmic,
        "XFCE" => DesktopEnvironment::Xfce,
        "MATE" => DesktopEnvironment::Mate,
        "CINNAMON" => DesktopEnvironment::Cinnamon,
        _ => {
            // Fallback to session desktop
            match session.to_uppercase().as_str() {
                "GNOME" | "GNOME-XORG" => DesktopEnvironment::Gnome,
                "PLASMA" | "PLASMAWAYLAND" => DesktopEnvironment::Kde,
                _ => DesktopEnvironment::Unknown,
            }
        }
    }
}
```

### 11.3 Context-Aware Lighting Rules

The trigger system supports pattern matching for intelligent profile switching:

```toml
# ~/.config/hypercolor/config.toml

[triggers]
# Restore previous profile when trigger condition ends
restore_on_exit = true

# Priority: higher number wins when multiple triggers match
# fullscreen > window > workspace > time > idle

[[triggers.fullscreen]]
profile = "immersive"
priority = 100

[[triggers.window]]
app_id = "steam_app_.*"      # Regex match
profile = "gaming"
priority = 90

[[triggers.window]]
app_id = "code"              # VS Code
profile = "coding"
priority = 50

[[triggers.window]]
app_id = "firefox"
title = ".*YouTube.*"
profile = "media"
priority = 50

[[triggers.window]]
app_id = "spotify"
profile = "music"
priority = 50

[[triggers.workspace]]
workspace = "1"
profile = "coding"
priority = 30

[[triggers.workspace]]
workspace = "2"
profile = "gaming"
priority = 30

[[triggers.workspace]]
workspace = "3"
profile = "chill"
priority = 30

[[triggers.time]]
start = "22:00"
end = "06:00"
profile = "nightshift"
priority = 20

[[triggers.idle]]
timeout_seconds = 300
profile = "dim"
priority = 10

[[triggers.idle]]
timeout_seconds = 600
profile = "off"
priority = 15
```

---

## 12. Notifications

### 12.1 Notification Events

| Event                 | Summary               | Body                               | Urgency | Condition                         |
| --------------------- | --------------------- | ---------------------------------- | ------- | --------------------------------- |
| Device connected      | "Device Connected"    | "Prism 8 (8 channels, 1008 LEDs)"  | Low     | Always                            |
| Device disconnected   | "Device Disconnected" | "Prism 8 disconnected"             | Normal  | Always                            |
| Render error          | "Effect Error"        | "Shader compilation failed: ..."   | Normal  | Always                            |
| Profile auto-switched | "Profile Switched"    | "Gaming (triggered by fullscreen)" | Low     | If `notify_profile_switch = true` |
| Update available      | "Update Available"    | "Hypercolor 0.3.0 available"       | Low     | Check interval: 24h               |
| Battery saver engaged | "Battery Saver"       | "Reduced to 15fps (45% battery)"   | Low     | First time per power cycle        |

### 12.2 Notification Preferences

```toml
# ~/.config/hypercolor/config.toml

[notifications]
enabled = true

# Per-event toggles
device_connect = true
device_disconnect = true
effect_error = true
profile_switch = false        # Can be noisy with workspace triggers
update_check = true
battery_saver = true

# Respect system Do Not Disturb
honor_dnd = true

# Suppress duplicate notifications within this window
dedup_seconds = 30
```

### 12.3 Do Not Disturb Detection

```rust
pub async fn is_dnd_active(connection: &Connection) -> bool {
    // GNOME
    if let Ok(settings) = gio::Settings::new_checked("org.gnome.desktop.notifications") {
        if !settings.boolean("show-banners") {
            return true;
        }
    }

    // KDE
    let kde_proxy = zbus::Proxy::new(
        connection,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    ).await;

    if let Ok(proxy) = kde_proxy {
        if let Ok(inhibited) = proxy.get_property::<bool>("Inhibited").await {
            return inhibited;
        }
    }

    false
}
```

---

## 13. Persona Scenarios

These scenarios validate the design against real usage patterns.

### 13.1 Bliss on Hyprland

**Setup:** CachyOS, Hyprland, Waybar, kitty, PrismRGB Prism 8 + Prism S + WLED strips.

**Experience:**

1. **Login:** systemd user service starts `hypercolor-daemon` automatically. No tray icon needed.

2. **Waybar** shows ` Aurora` in Electric Purple in the status bar. Mouse hover shows "3 devices | 60fps | 100%".

3. **Keybinds:**
   - `Super+L` cycles through favorite effects.
   - `Super+[` / `Super+]` adjusts brightness.
   - `Super+E` opens effect mode -- press `1` for coding, `2` for gaming, `3` for chill.

4. **Workspace triggers:**
   - Workspace 1 (coding): purple ambient glow, low brightness.
   - Workspace 2 (gaming): reactive audio visualization on all devices.
   - Workspace 3 (media): screen-sync ambient lighting.

5. **Fullscreen game launch:** Hyprland IPC fires `fullscreen>>1`. Daemon switches to "gaming" profile with audio-reactive effects and full brightness. Game exits, profile restores.

6. **CLI scripting:**

   ```bash
   # Quick effect change from terminal
   hypercolor set aurora --speed 8

   # Scriptable brightness for screencasting
   hypercolor brightness 30

   # Pipe-friendly status for scripts
   hypercolor status --json | jq '.effect'
   ```

7. **Late night:** Time trigger kicks in at 22:00. Effects shift to warm amber tones, brightness drops to 40%. Night Light integration applies color temperature correction.

### 13.2 Jake on GNOME

**Setup:** Ubuntu, GNOME 47, Razer peripherals + WLED strips. Casual user, wants it to "just work."

**Experience:**

1. **Login:** systemd starts the daemon. GNOME Shell extension loads automatically.

2. **System tray** (via AppIndicator extension): Hypercolor icon in the top bar. Left-click opens web UI. Right-click shows quick menu: current effect, brightness slider, toggle, profiles.

3. **Quick Settings panel** (GNOME 44+): Hypercolor toggle tile in the system menu, right next to Night Light and Do Not Disturb. Expanding the tile shows a brightness slider and profile selector.

4. **Night Light sync:** GNOME Night Light is active from 21:00 to 07:00 with a 4000K color temperature. Hypercolor reads this via D-Bus and applies a matching warm shift to all LED output. The room lighting matches the screen.

5. **Do Not Disturb:** Jake enables DND before a meeting. Hypercolor suppresses all notifications but continues running effects silently.

6. **Game launch:** Jake launches Steam. The daemon detects the `steam_app_*` window class via GNOME's Mutter IPC and switches to the gaming profile. When the game exits, the previous profile restores.

7. **Lock screen:** Jake locks the screen (`Super+L`). The GNOME Shell extension catches the `ScreenShield.active` signal and tells the daemon to switch to a minimal warm nightlight mode. Screen unlock restores the previous state.

8. **Workspace switch:** Jake drags a window to workspace 2. The extension fires `active-workspace-changed`, and the daemon checks the workspace profile map. Workspace 2 is mapped to "chill" -- soft gradient transitions across all devices.

### 13.3 Marcus on KDE Plasma

**Setup:** openSUSE Tumbleweed, KDE Plasma 6, Corsair iCUE LINK + ASUS AURA + WLED. Power user with an Android phone.

**Experience:**

1. **System tray:** StatusNotifierItem works natively on Plasma. The Hypercolor tray icon sits in the system tray with full right-click menu, tooltip showing current state, and scroll-to-adjust brightness.

2. **Plasma widget:** Marcus adds the Hypercolor widget to his panel. It shows the current effect name and provides an expandable popup with a brightness slider, effect list with search, profile selector, and navigation buttons.

3. **KDE Connect (phone control):** Marcus configures KDE Connect "Run Command" entries for Hypercolor. From his phone, he can:
   - Toggle lighting on/off
   - Switch to gaming mode
   - Cycle effects
   - Set brightness

4. **KDE Activities:** Marcus uses KDE Activities (not just workspaces). Each Activity maps to a lighting profile:
   - "Work" Activity: minimal blue ambient
   - "Gaming" Activity: full RGB reactive
   - "Movie" Activity: screen-sync ambient

5. **Color scheme sync:** Marcus switches his Plasma global theme from Breeze Dark to Catppuccin Mocha. Hypercolor reads the new accent color via the `org.freedesktop.portal.Settings` portal and applies it to the "Desktop Sync" effect -- LED colors shift to match the new pink accent.

6. **KDE Connect media sync:** Marcus is playing Spotify. KDE Connect shares the media player state. Hypercolor reads the album art dominant colors and applies them as a gentle ambient wash. When the track changes, the colors smoothly transition.

---

## 14. Package & Installation Matrix

### 14.1 What Ships Where

| Component           | Package                            | Required? | Notes                                                            |
| ------------------- | ---------------------------------- | --------- | ---------------------------------------------------------------- |
| `hypercolor-daemon` | `hypercolor`                       | Yes       | Core daemon binary (`hypercolor-daemon`)                         |
| `hypercolor-cli`    | `hypercolor`                       | Yes       | CLI binary (`hypercolor`, hosts the `hypercolor tui` subcommand) |
| `hypercolor-tui`    | `hypercolor`                       | Yes       | TUI library (launched via `hypercolor tui`)                      |
| `hypercolor-tray`   | `hypercolor-tray`                  | Optional  | System tray indicator (ksni)                                     |
| GNOME extension     | `gnome-shell-extension-hypercolor` | Optional  | GNOME Shell extension                                            |
| KDE widget          | `plasma-widget-hypercolor`         | Optional  | Plasma plasmoid                                                  |
| COSMIC applet       | `cosmic-applet-hypercolor`         | Optional  | COSMIC panel applet                                              |
| systemd units       | `hypercolor`                       | Yes       | Installed with daemon                                            |
| D-Bus service file  | `hypercolor`                       | Yes       | Installed with daemon                                            |
| Desktop entry       | `hypercolor`                       | Yes       | Installed with daemon                                            |
| udev rules          | `hypercolor`                       | Yes       | USB device access                                                |
| Icons               | `hypercolor`                       | Yes       | hicolor theme                                                    |

### 14.2 udev Rules

USB HID devices require udev rules for non-root access:

```udev
# /usr/lib/udev/rules.d/99-hypercolor.rules

# PrismRGB Prism S
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="1294", MODE="0666", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="1294", MODE="0666", TAG+="uaccess"

# PrismRGB Prism 8
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d5", ATTRS{idProduct}=="1f01", MODE="0666", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="16d5", ATTRS{idProduct}=="1f01", MODE="0666", TAG+="uaccess"

# PrismRGB Prism Mini
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="1407", MODE="0666", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="1407", MODE="0666", TAG+="uaccess"

# Nollie 8 v2
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d2", ATTRS{idProduct}=="1f01", MODE="0666", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="16d2", ATTRS{idProduct}=="1f01", MODE="0666", TAG+="uaccess"

# Generic: allow logged-in users (uaccess tag)
# This works with systemd-logind, no group membership needed
```

### 14.3 Post-Install Setup

```bash
# After package install:
sudo udevadm control --reload-rules
sudo udevadm trigger

# Enable and start the service
systemctl --user enable --now hypercolor.service

# Or with socket activation:
systemctl --user enable --now hypercolor.socket

# Verify
systemctl --user status hypercolor
hypercolor status
```

---

## 15. Implementation Priority

### Phase 1: Foundation (Ship First)

1. systemd user service + socket activation
2. D-Bus interface (core methods + properties)
3. XDG desktop entry + autostart
4. CLI subcommands for bar modules (`waybar`, `i3blocks`, `polybar`)
5. udev rules
6. Journal logging
7. Freedesktop notifications

### Phase 2: Desktop Integrations

1. `hypercolor-tray` (StatusNotifierItem via ksni)
2. Hyprland IPC watcher
3. sway/i3 IPC watcher
4. UPower battery integration
5. Suspend/resume state preservation

### Phase 3: Native Extensions

1. GNOME Shell extension (panel indicator + Quick Settings + Night Light + workspace events)
2. KDE Plasma widget (plasmoid)
3. Waybar native module (if Waybar adds Rust module support)

### Phase 4: Advanced Integration

1. COSMIC applet
2. KDE Connect integration
3. Color scheme sync (accent color portal)
4. Context-aware window rules
5. Multi-monitor per-context lighting
6. Idle detection + time-based triggers

---

## 16. Crate Dependencies (Desktop Integration)

| Crate              | Purpose                               | License           |
| ------------------ | ------------------------------------- | ----------------- |
| `zbus`             | D-Bus client + server                 | MIT               |
| `sd-notify`        | systemd watchdog + ready notification | MIT OR Apache-2.0 |
| `listenfd`         | systemd socket activation             | MIT OR Apache-2.0 |
| `tracing-journald` | Journal log subscriber                | MIT               |
| `ksni`             | StatusNotifierItem system tray        | Apache-2.0        |
| `open`             | Open URLs in default browser          | MIT OR Apache-2.0 |
| `swayipc-async`    | sway/i3 IPC events                    | MIT               |
| `dirs`             | XDG directory paths                   | MIT OR Apache-2.0 |

All Apache-2.0 compatible. No GPL contamination in the desktop integration layer.
