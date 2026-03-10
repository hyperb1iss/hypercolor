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

/// Poll interval for the main event loop (milliseconds).
const POLL_INTERVAL: Duration = Duration::from_millis(50);

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

    // Initial state: disconnected.
    let mut app_state = AppState::disconnected();

    // Build initial icon and menu.
    let icon = build_icon(IconState::Disconnected)?;
    let tray_menu = build_menu(&app_state)?;

    // Create the tray icon.
    let tray_icon = TrayIconBuilder::new()
        .with_tooltip("Hypercolor")
        .with_icon(icon)
        .with_menu(Box::new(tray_menu))
        .build()?;

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
    loop {
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
                    app_state = AppState::disconnected();
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
            handle_menu_event(&event, &app_state, &cmd_tx);
        }

        // Process tray icon click events (left-click opens web UI).
        while let Ok(event) = tray_channel.try_recv() {
            if let tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                ..
            } = event
            {
                let _ = cmd_tx.send(TrayCommand::OpenWebUi);
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    }
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

/// Handle a menu item click event.
fn handle_menu_event(
    event: &tray_icon::menu::MenuEvent,
    _state: &AppState,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<TrayCommand>,
) {
    let id = event.id().as_ref();

    match id {
        menu::ids::OPEN_WEB_UI => {
            let _ = cmd_tx.send(TrayCommand::OpenWebUi);
        }
        menu::ids::PAUSE_RESUME => {
            let _ = cmd_tx.send(TrayCommand::TogglePause);
        }
        menu::ids::STOP_EFFECT => {
            let _ = cmd_tx.send(TrayCommand::StopEffect);
        }
        menu::ids::QUIT => {
            let _ = cmd_tx.send(TrayCommand::Quit);
            // Give the daemon client a moment to shut down cleanly.
            std::thread::sleep(Duration::from_millis(100));
            std::process::exit(0);
        }
        other => {
            if let Some(effect_id) = other.strip_prefix(menu::ids::EFFECT_PREFIX) {
                let _ = cmd_tx.send(TrayCommand::ApplyEffect(effect_id.to_owned()));
            } else if let Some(profile_id) = other.strip_prefix(menu::ids::PROFILE_PREFIX) {
                let _ = cmd_tx.send(TrayCommand::ApplyProfile(profile_id.to_owned()));
            }
        }
    }
}
