//! Hypercolor system tray applet.
//!
//! Lightweight binary that provides system tray / menu bar presence for the
//! Hypercolor daemon. Communicates with the daemon exclusively via the REST
//! API and WebSocket on `localhost:9420`.
//!
//! Architecture:
//! - Main thread runs the platform event loop (required by tray-icon)
//! - Background thread runs a tokio runtime for async daemon communication
//! - `std::sync::mpsc` channels bridge daemon state to the UI thread
//! - `tokio::sync::mpsc` channels send commands from UI to daemon client

mod daemon;
mod icons;
mod menu;
mod state;

use std::sync::mpsc;
use std::time::Duration;

use tracing::{error, info};
use tray_icon::TrayIconBuilder;

use crate::daemon::DaemonClient;
use crate::icons::{IconState, build_icon};
use crate::menu::build_menu;
use crate::state::{AppState, DaemonMessage, EffectInfo, StateUpdate, TrayCommand};

/// Initialize the macOS `NSApplication` and set it to accessory mode (no dock icon).
///
/// Without calling `finishLaunching`, the `AppKit` menu subsystem is not fully
/// initialized, so status-item popup menus will not appear on click.
#[cfg(target_os = "macos")]
fn init_platform() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let mtm = MainThreadMarker::new().expect("must be called from the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    // Complete the launch sequence so AppKit menus work properly.
    app.finishLaunching();
    app.activate();
}

/// Initialize GTK on Linux (required by tray-icon's libappindicator backend).
#[cfg(target_os = "linux")]
fn init_platform() {
    gtk::init().expect("failed to initialize GTK");
}

/// No special initialization needed on Windows.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn init_platform() {}

/// Pump platform events on macOS by explicitly pulling events from the
/// `NSApplication` event queue and dispatching them via `sendEvent:`.
///
/// Using `NSRunLoop::runUntilDate` alone does NOT process mouse events through
/// `NSApplication`, so status-item clicks (and their popup menus) are never
/// delivered. The `nextEvent`/`sendEvent` loop is the canonical pattern used
/// by winit, tao, and other non-`[NSApp run]` hosts.
#[cfg(target_os = "macos")]
fn pump_events(timeout: Duration) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEventMask};
    use objc2_foundation::{NSDate, NSString};

    let mtm = MainThreadMarker::new().expect("pump_events must be called from the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let until_date = NSDate::dateWithTimeIntervalSinceNow(timeout.as_secs_f64());
    // NSDefaultRunLoopMode value — constructed directly to avoid `unsafe` extern static access.
    let mode = NSString::from_str("kCFRunLoopDefaultMode");

    loop {
        let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&until_date),
            &mode,
            true,
        );
        match event {
            Some(event) => {
                app.sendEvent(&event);
            }
            None => break,
        }
    }
}

/// On Linux, pump the GTK event loop to dispatch menu and tray events.
#[cfg(target_os = "linux")]
fn pump_events(_timeout: Duration) {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
    // Brief sleep to avoid busy-waiting when no events are pending.
    std::thread::sleep(Duration::from_millis(16));
}

/// On Windows, pump the Win32 message loop for tray icon events.
#[cfg(target_os = "windows")]
fn pump_events(timeout: Duration) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        let has_msg = unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) };
        if has_msg != 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        } else if std::time::Instant::now() >= deadline {
            break;
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Fallback for other platforms: just sleep.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn pump_events(timeout: Duration) {
    std::thread::sleep(timeout);
}

/// Poll interval for the main event loop (milliseconds).
const POLL_INTERVAL: Duration = Duration::from_millis(16);

fn main() -> anyhow::Result<()> {
    // Initialize tracing subscriber.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();

    info!("Starting Hypercolor tray applet");

    // Initialize platform (NSApplication on macOS).
    init_platform();

    // Pump the event loop once before creating the tray icon.
    // On macOS, the tray icon must be created after the NSApplication
    // run loop is active to properly register with the system menu bar.
    pump_events(Duration::from_millis(1));

    // Initial state: disconnected.
    let mut app_state = AppState::disconnected();

    // Build initial icon and menu.
    let icon = build_icon(IconState::Disconnected)?;
    let tray_menu = build_menu(&app_state)?;

    // Create the tray icon (after the event loop is primed).
    let tray_icon = TrayIconBuilder::new()
        .with_tooltip("Hypercolor")
        .with_icon(icon)
        .with_menu(Box::new(tray_menu))
        .build()?;

    // Pump again to let the system process the new tray icon registration.
    pump_events(Duration::from_millis(1));

    // Channels: daemon -> tray UI (state updates).
    let (daemon_tx, daemon_rx) = mpsc::channel::<DaemonMessage>();

    // Channels: tray UI -> daemon client (commands).
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<TrayCommand>();

    // Spawn tokio runtime in a background thread for async daemon communication.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        rt.block_on(async {
            let mut client = DaemonClient::new(daemon_tx, cmd_rx);
            client.run().await;
        });
    });

    // Get event receivers for menu and tray icon events.
    let menu_channel = tray_icon::menu::MenuEvent::receiver();
    let tray_channel = tray_icon::TrayIconEvent::receiver();

    info!("Tray applet running; waiting for daemon connection");

    // Main event loop on the main thread.
    let mut running = true;
    while running {
        // Pump platform events first so pending clicks/menu interactions are
        // delivered to the channel receivers before we drain them.
        pump_events(POLL_INTERVAL);

        // Process all pending daemon messages.
        while let Ok(msg) = daemon_rx.try_recv() {
            match msg {
                DaemonMessage::Connected(new_state) => {
                    info!(
                        "Connected to daemon (devices={}, effects={})",
                        new_state.device_count,
                        new_state.effects.len()
                    );
                    app_state = new_state;
                    update_tray(&tray_icon, &app_state);
                }
                DaemonMessage::Disconnected => {
                    info!("Disconnected from daemon");
                    mark_disconnected(&mut app_state);
                    sync_active_server(&mut app_state);
                    update_tray(&tray_icon, &app_state);
                }
                DaemonMessage::ServersUpdated(servers) => {
                    app_state.servers = servers;
                    sync_active_server(&mut app_state);
                    update_tray(&tray_icon, &app_state);
                }
                DaemonMessage::StateUpdate(update) => {
                    apply_state_update(&mut app_state, update);
                    update_tray(&tray_icon, &app_state);
                }
            }
        }

        // Process menu click events.
        while let Ok(event) = menu_channel.try_recv() {
            if handle_menu_event(&event, &app_state, &cmd_tx) {
                running = false;
            }
        }

        // Process tray icon click events (left-click opens web UI).
        // Note: on macOS, click events are NOT fired when a menu is set —
        // the menu shows automatically. This handler is for Linux.
        while let Ok(event) = tray_channel.try_recv() {
            if let tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                ..
            } = event
            {
                let _ = cmd_tx.send(TrayCommand::OpenWebUi);
            }
        }
    }

    // Drop tray_icon cleanly so platform cleanup (remove status item, delete
    // temp icon files, etc.) runs via the Drop impl.
    drop(tray_icon);
    info!("Tray applet exiting");
    Ok(())
}

/// Apply an incremental state update to the app state.
fn apply_state_update(state: &mut AppState, update: StateUpdate) {
    match update {
        StateUpdate::EffectChanged { id, name } => {
            state.current_effect = Some(EffectInfo { id, name });
        }
        StateUpdate::EffectStopped => {
            state.current_effect = None;
        }
        StateUpdate::BrightnessChanged(value) => {
            state.brightness = value;
        }
        StateUpdate::Paused => {
            state.paused = true;
        }
        StateUpdate::Resumed => {
            state.paused = false;
        }
        StateUpdate::DeviceCountChanged(count) => {
            state.device_count = count;
        }
        StateUpdate::EffectsRefreshed(effects) => {
            state.effects = effects;
        }
    }
}

fn mark_disconnected(state: &mut AppState) {
    state.connected = false;
    state.running = false;
    state.paused = false;
    state.brightness = 0;
    state.current_effect = None;
    state.device_count = 0;
    state.effects.clear();
    state.profiles.clear();
    state.server_identity = None;
}

fn sync_active_server(state: &mut AppState) {
    state.active_server = state.server_identity.as_ref().and_then(|identity| {
        state
            .servers
            .iter()
            .position(|entry| entry.server.identity.instance_id == identity.instance_id)
    });
}

/// Update the tray icon and menu to reflect the current state.
fn update_tray(tray_icon: &tray_icon::TrayIcon, state: &AppState) {
    // Update icon.
    let icon_state = if !state.connected {
        IconState::Disconnected
    } else if state.paused {
        IconState::Paused
    } else {
        IconState::Active
    };

    match build_icon(icon_state) {
        Ok(icon) => {
            if let Err(e) = tray_icon.set_icon(Some(icon)) {
                error!("Failed to update tray icon: {e}");
            }
        }
        Err(e) => error!("Failed to build tray icon: {e}"),
    }

    // Update tooltip.
    let tooltip = if state.connected {
        match &state.current_effect {
            Some(effect) => format!("Hypercolor - {}", effect.name),
            None => "Hypercolor - No effect".to_owned(),
        }
    } else {
        "Hypercolor - Disconnected".to_owned()
    };
    let _ = tray_icon.set_tooltip(Some(&tooltip));

    // Rebuild menu.
    match build_menu(state) {
        Ok(new_menu) => {
            tray_icon.set_menu(Some(Box::new(new_menu)));
        }
        Err(e) => error!("Failed to build tray menu: {e}"),
    }
}

/// Handle a menu item click event. Returns `true` if the applet should quit.
fn handle_menu_event(
    event: &tray_icon::menu::MenuEvent,
    _state: &AppState,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<TrayCommand>,
) -> bool {
    let id = event.id().as_ref();

    match id {
        menu::ids::OPEN_WEB_UI => {
            let _ = cmd_tx.send(TrayCommand::OpenWebUi);
        }
        menu::ids::PAUSE_RESUME => {
            let _ = cmd_tx.send(TrayCommand::TogglePause);
        }
        menu::ids::REFRESH_SERVERS => {
            let _ = cmd_tx.send(TrayCommand::RefreshServers);
        }
        menu::ids::STOP_EFFECT => {
            let _ = cmd_tx.send(TrayCommand::StopEffect);
        }
        menu::ids::QUIT => {
            let _ = cmd_tx.send(TrayCommand::Quit);
            return true;
        }
        other => {
            if let Some(effect_id) = other.strip_prefix(menu::ids::EFFECT_PREFIX) {
                let _ = cmd_tx.send(TrayCommand::ApplyEffect(effect_id.to_owned()));
            } else if let Some(profile_id) = other.strip_prefix(menu::ids::PROFILE_PREFIX) {
                let _ = cmd_tx.send(TrayCommand::ApplyProfile(profile_id.to_owned()));
            } else if let Some(index) = other.strip_prefix(menu::ids::SERVER_PREFIX)
                && let Ok(index) = index.parse::<usize>()
            {
                let _ = cmd_tx.send(TrayCommand::SwitchServer(index));
            }
        }
    }
    false
}
