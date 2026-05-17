//! App — the central coordinator and main event loop.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::ExecutableCommand;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use hypercolor_types::controls::{ApplyControlChangesRequest, ControlChange};
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::layout::Rect;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::action::Action;
use crate::chrome::Chrome;
use crate::client::rest::DaemonClient;
use crate::component::Component;
use crate::event::{Event, EventReader};
use crate::motion::{MotionSensitivity, MotionSystem};
use crate::preview::PreviewManager;
use crate::screen::ScreenId;
use crate::state::{
    AppState, CanvasFrame, ConnectionStatus, ControlValue, Notification, NotificationLevel,
    PreviewSource, SimulatedDisplaySummary,
};
use crate::theme_picker::ThemePicker;
use opaline::widgets::ThemeSelectorAction;
use ratatui_image::picker::Picker;

const SIMULATOR_LIST_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const SIMULATOR_FRAME_REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// Top-level application that owns all screens and drives the event loop.
pub struct App {
    /// The currently active screen.
    active_screen: ScreenId,
    /// The previously active screen (for `GoBack`).
    previous_screen: Option<ScreenId>,
    /// All registered screens, keyed by ID.
    screens: HashMap<ScreenId, Box<dyn Component>>,
    /// Stable screen order for chrome and help overlays.
    available_screens: Vec<ScreenId>,
    /// Persistent chrome (title bar, LED strip, nav, audio, status).
    chrome: Chrome,
    /// Shared state accessible by all components.
    state: AppState,
    /// Lifecycle flags for the event loop and idle effects.
    lifecycle: LifecycleState,
    /// UI flags for overlays, redraws, and fullscreen mode.
    view: ViewState,
    /// Cached preview routing state for simulator inspection.
    simulator_preview: SimulatorPreviewState,
    /// Latest live canvas frame from the daemon WebSocket.
    latest_canvas_frame: Option<Arc<CanvasFrame>>,
    /// Active live theme picker, when modal is open.
    theme_picker: Option<ThemePicker>,
    /// Motion effects engine (tachyonfx-backed).
    motion: MotionSystem,
    /// Live preview transport manager.
    preview: PreviewManager,
    /// Last rendered frame area, cached so action handlers can scope effects.
    last_frame_area: Rect,
    /// Last user input or significant state event, used to trigger the idle
    /// breathing effect after a period of inactivity.
    last_activity: Instant,
    /// Current notification (auto-dismisses).
    notification: Option<(Notification, Instant)>,
    /// Action sender (cloned to components and bridge).
    action_tx: mpsc::UnboundedSender<Action>,
    /// Action receiver (drained each frame).
    action_rx: mpsc::UnboundedReceiver<Action>,
    /// REST client for sending commands to the daemon.
    client: DaemonClient,
    /// Cancellation token for the data bridge.
    data_cancel: CancellationToken,
    /// Daemon host.
    host: String,
    /// Daemon port.
    port: u16,
    /// API key for authenticated daemon connections.
    api_key: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct LifecycleState {
    running: bool,
    idle_active: bool,
}

#[derive(Debug, Clone, Copy)]
struct ViewState {
    help_visible: bool,
    fullscreen_preview: bool,
    render_dirty: bool,
}

#[derive(Debug, Clone, Default)]
struct SimulatorPreviewState {
    source: PreviewSource,
    simulators: Vec<SimulatedDisplaySummary>,
    latest_frame: Option<Arc<CanvasFrame>>,
    latest_frame_id: Option<String>,
    list_requested_at: Option<Instant>,
    frame_requested_at: Option<(String, Instant)>,
}

impl App {
    /// Create a new app targeting the given daemon.
    pub fn new(host: String, port: u16, api_key: Option<String>) -> Self {
        let (action_tx, action_rx) = mpsc::unbounded_channel();
        let screen_defs = crate::views::create_screens();
        let available_screens = screen_defs.iter().map(|(id, _)| *id).collect();
        let screens = screen_defs.into_iter().collect();

        let client = DaemonClient::new(&host, port, api_key.as_deref());

        // Query the terminal for the best graphics protocol BEFORE entering
        // raw/alternate-screen mode. Falls back to halfblocks on any failure
        // (test environments, dumb terminals, missing capabilities).
        let picker = Picker::from_query_stdio().unwrap_or_else(|e| {
            tracing::info!("graphics protocol query failed, using halfblocks: {e}");
            Picker::halfblocks()
        });
        tracing::info!(
            "image protocol: {:?} font_size: {:?}",
            picker.protocol_type(),
            picker.font_size()
        );

        Self {
            active_screen: ScreenId::Dashboard,
            previous_screen: None,
            screens,
            available_screens,
            chrome: Chrome::new(),
            state: AppState {
                show_donate: crate::theme_picker::load_config().show_donate,
                ..AppState::default()
            },
            lifecycle: LifecycleState {
                running: true,
                idle_active: false,
            },
            view: ViewState {
                help_visible: false,
                fullscreen_preview: false,
                render_dirty: true,
            },
            simulator_preview: SimulatorPreviewState::default(),
            latest_canvas_frame: None,
            theme_picker: None,
            motion: MotionSystem::new(MotionSensitivity::resolve(MotionSensitivity::Full)),
            preview: PreviewManager::new(picker),
            last_frame_area: Rect::new(0, 0, 80, 24),
            last_activity: Instant::now(),
            notification: None,
            action_tx,
            action_rx,
            client,
            data_cancel: CancellationToken::new(),
            host,
            port,
            api_key,
        }
    }

    /// Run the main event loop.
    pub async fn run(&mut self) -> Result<()> {
        // Initialize terminal
        let mut terminal = ratatui::init();
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;

        // Initialize all screens
        for screen in self.screens.values_mut() {
            screen.init(self.action_tx.clone())?;
        }
        if let Some(screen) = self.screens.get_mut(&self.active_screen) {
            screen.set_focused(true);
        }

        // Install the persistent title bar shimmer effect. Brand area is
        // computed from the first row of the terminal — the title bar
        // always renders at y=0 and the brand always starts at column 1.
        let initial_size = terminal
            .size()
            .unwrap_or_else(|_| ratatui::layout::Size::new(80, 24));
        let title_area = Rect::new(0, 0, initial_size.width, 1);
        let brand_area = crate::chrome::TitleBar::brand_area(title_area);
        if brand_area.width > 0 {
            self.motion.trigger(
                crate::motion::MotionKey::TitleShimmer,
                crate::motion::catalog::title_shimmer(brand_area, self.motion.sensitivity()),
            );
        }

        // Spawn data bridge
        let bridge_tx = self.action_tx.clone();
        let bridge_cancel = self.data_cancel.clone();
        let bridge_host = self.host.clone();
        let bridge_port = self.port;
        let bridge_api_key = self.api_key.clone();
        tokio::spawn(async move {
            crate::bridge::spawn_data_bridge(
                bridge_host,
                bridge_port,
                bridge_api_key,
                bridge_tx,
                bridge_cancel,
            )
            .await;
        });

        // Event reader: 250ms tick, ~15 FPS render
        // 250ms data tick, 16ms (~60 FPS) render — smooth motion for tachyonfx
        let mut events = EventReader::new(Duration::from_millis(250), Duration::from_millis(16));

        tracing::info!("TUI event loop started");

        while self.lifecycle.running {
            let Some(event) = events.next().await else {
                break;
            };
            let mut render_requested = matches!(event, Event::Render);

            // Map event → action
            match event {
                Event::Key(key) => {
                    self.bump_activity();
                    if let Some(action) = self.handle_key_event(key) {
                        let _ = self.action_tx.send(action);
                    }
                }
                Event::Mouse(mouse) => {
                    self.bump_activity();
                    if let Some(action) = self.handle_mouse_event(mouse) {
                        let _ = self.action_tx.send(action);
                    }
                }
                Event::Resize(w, h) => {
                    self.bump_activity();
                    let _ = self.action_tx.send(Action::Resize(w, h));
                }
                Event::Tick => {
                    let _ = self.action_tx.send(Action::Tick);
                }
                Event::Render => {}
            }

            self.view.render_dirty |= self.preview.drain_resize_results();

            // Drain and process all queued actions
            let mut latest_canvas_frame = None;
            let mut latest_spectrum = None;
            while let Ok(action) = self.action_rx.try_recv() {
                if let Action::Render = action {
                    render_requested = true;
                    self.view.render_dirty = true;
                    continue;
                }

                if let Action::CanvasFrameReceived(frame) = action {
                    latest_canvas_frame = Some(frame);
                    continue;
                }

                if let Action::SpectrumUpdated(snapshot) = action {
                    latest_spectrum = Some(snapshot);
                    continue;
                }

                self.process_action(&action);
                self.view.render_dirty = true;
            }

            if let Some(snapshot) = latest_spectrum {
                self.process_action(&Action::SpectrumUpdated(snapshot));
                if !self.view.fullscreen_preview {
                    self.view.render_dirty = true;
                }
            }

            if let Some(frame) = latest_canvas_frame {
                self.process_action(&Action::CanvasFrameReceived(frame));
                if !self.view.fullscreen_preview {
                    self.view.render_dirty = true;
                }
            }

            if self.view.render_dirty
                || (render_requested && !self.view.fullscreen_preview && self.motion.is_active())
            {
                self.view.render_dirty |= self.preview.drain_resize_results();
                self.draw_terminal(&mut terminal)?;
                self.view.render_dirty = false;
            }
        }

        // Cleanup
        self.data_cancel.cancel();
        events.stop();
        self.preview.shutdown();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        ratatui::restore();
        tracing::info!("TUI event loop ended");
        Ok(())
    }

    fn draw_terminal(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let draw_started = Instant::now();
        let sync_started = match terminal
            .backend_mut()
            .execute(BeginSynchronizedUpdate)
            .map(|_| ())
        {
            Ok(()) => true,
            Err(error) => {
                tracing::debug!("sync update unavailable, drawing without it: {error}");
                false
            }
        };

        let draw_result = terminal.draw(|frame| self.render(frame)).map(|_| ());
        if !sync_started {
            self.preview.note_draw_duration(draw_started.elapsed());
            return draw_result.map_err(Into::into);
        }

        let result = match terminal
            .backend_mut()
            .execute(EndSynchronizedUpdate)
            .map(|_| ())
        {
            Ok(()) => draw_result.map_err(Into::into),
            Err(error) => match draw_result {
                Ok(()) => Err(error.into()),
                Err(draw_error) => {
                    tracing::debug!("failed to end sync update after draw error: {error}");
                    Err(draw_error.into())
                }
            },
        };
        self.preview.note_draw_duration(draw_started.elapsed());
        result
    }

    /// Reset the idle timer and cancel the breathing effect if it's running.
    fn bump_activity(&mut self) {
        self.last_activity = Instant::now();
        if self.lifecycle.idle_active {
            self.motion.cancel(crate::motion::MotionKey::IdleBreathing);
            self.lifecycle.idle_active = false;
        }
    }

    /// Check the idle threshold and start the breathing effect if exceeded.
    fn check_idle(&mut self) {
        // 10s idle → start breathing
        const IDLE_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(10);
        if !self.lifecycle.idle_active && self.last_activity.elapsed() >= IDLE_THRESHOLD {
            self.motion.trigger(
                crate::motion::MotionKey::IdleBreathing,
                crate::motion::catalog::idle_breathing(
                    self.last_frame_area,
                    self.motion.sensitivity(),
                ),
            );
            self.lifecycle.idle_active = true;
        }
    }

    /// Handle a key event, returning an action to dispatch.
    fn handle_key_event(&mut self, key: KeyEvent) -> Option<Action> {
        // Theme picker captures all keys when open
        if let Some(picker) = self.theme_picker.as_mut() {
            match picker.handle_key(key) {
                ThemeSelectorAction::Select(name) => {
                    if let Err(e) = crate::theme_picker::save_theme(&name) {
                        tracing::warn!("failed to persist theme: {e}");
                    }
                    self.theme_picker = None;
                    return Some(Action::Notify(Notification {
                        message: format!("Theme: {name}"),
                        level: NotificationLevel::Success,
                    }));
                }
                ThemeSelectorAction::Cancel => {
                    self.theme_picker = None;
                    return None;
                }
                _ => return None,
            }
        }

        if self.view.fullscreen_preview {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('z' | 'Z') => Some(Action::ToggleFullscreenPreview),
                KeyCode::Char('q') => Some(Action::Quit),
                _ => None,
            };
        }

        if self.view.help_visible {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('?') => Some(Action::ToggleHelp),
                _ => None,
            };
        }

        // Global keybindings (always active)
        match key.code {
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Char('?') => return Some(Action::ToggleHelp),
            KeyCode::Char('$') => return Some(Action::OpenDonate),
            KeyCode::Char('T' | 't') => return Some(Action::ToggleThemePicker),
            KeyCode::Char('M' | 'm') => return Some(Action::CycleMotionSensitivity),
            KeyCode::Char('Z' | 'z') => return Some(Action::ToggleFullscreenPreview),
            KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                if let Some(screen) = ScreenId::from_key(c)
                    && self.screens.contains_key(&screen)
                    && screen != self.active_screen
                {
                    return Some(Action::SwitchScreen(screen));
                }
            }
            KeyCode::Esc => {
                if self.view.help_visible {
                    return Some(Action::ToggleHelp);
                }
                return Some(Action::GoBack);
            }
            _ => {}
        }

        // Delegate to active screen
        if let Some(screen) = self.screens.get_mut(&self.active_screen)
            && let Ok(Some(action)) = screen.handle_key_event(key)
        {
            return Some(action);
        }

        None
    }

    fn handle_mouse_event(&mut self, mouse: MouseEvent) -> Option<Action> {
        if self.view.fullscreen_preview {
            return match mouse.kind {
                MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => {
                    Some(Action::ToggleFullscreenPreview)
                }
                _ => None,
            };
        }

        if self.theme_picker.is_some() {
            return match mouse.kind {
                MouseEventKind::Down(MouseButton::Right) => Some(Action::ToggleThemePicker),
                _ => None,
            };
        }

        if self.view.help_visible {
            return match mouse.kind {
                MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => {
                    Some(Action::ToggleHelp)
                }
                _ => None,
            };
        }

        if let Some(action) = self.handle_chrome_mouse_event(mouse) {
            return Some(action);
        }

        self.screens
            .get_mut(&self.active_screen)
            .and_then(|screen| screen.handle_mouse_event(mouse).ok().flatten())
    }

    fn handle_chrome_mouse_event(&self, mouse: MouseEvent) -> Option<Action> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return None;
        }

        let hit = crate::chrome::StatusBar::hit_test(
            self.status_bar_area(),
            mouse.column,
            mouse.row,
            self.active_screen,
            self.available_screens(),
            self.state.show_donate,
        )?;

        match hit {
            crate::chrome::StatusBarHit::Screen(screen) => Some(Action::SwitchScreen(screen)),
            crate::chrome::StatusBarHit::Sponsor => Some(Action::OpenDonate),
            crate::chrome::StatusBarHit::Help => Some(Action::ToggleHelp),
        }
    }

    fn status_bar_area(&self) -> Rect {
        let area = self.last_frame_area;
        if area.height == 0 {
            return Rect::default();
        }
        Rect::new(
            area.x,
            area.y + area.height.saturating_sub(1),
            area.width,
            1,
        )
    }

    /// Process a single action, updating state and forwarding to components.
    #[allow(clippy::too_many_lines)]
    fn process_action(&mut self, action: &Action) {
        match action {
            Action::Quit => self.lifecycle.running = false,

            Action::SwitchScreen(screen_id) => {
                if !self.screens.contains_key(screen_id) {
                    self.notification = Some((
                        Notification {
                            message: format!("{screen_id} is not available in the TUI yet"),
                            level: NotificationLevel::Warning,
                        },
                        Instant::now(),
                    ));
                    return;
                }

                if let Some(old) = self.screens.get_mut(&self.active_screen) {
                    old.set_focused(false);
                }
                self.previous_screen = Some(self.active_screen);
                self.active_screen = *screen_id;
                self.state.active_screen = *screen_id;
                if let Some(new) = self.screens.get_mut(&self.active_screen) {
                    new.set_focused(true);
                }

                // Motion: dissolve+coalesce the screen content area
                self.motion.trigger(
                    crate::motion::MotionKey::ScreenTransition,
                    crate::motion::catalog::screen_transition(
                        self.last_frame_area,
                        self.motion.sensitivity(),
                    ),
                );
                self.refresh_preview_surface();
            }

            Action::GoBack => {
                if let Some(prev) = self.previous_screen {
                    let _ = self.action_tx.send(Action::SwitchScreen(prev));
                }
            }

            Action::ToggleHelp => {
                self.view.help_visible = !self.view.help_visible;
            }
            Action::ToggleThemePicker => {
                self.theme_picker = if self.theme_picker.is_some() {
                    None
                } else {
                    Some(ThemePicker::open())
                };
            }
            Action::CycleMotionSensitivity => {
                self.motion.cycle_sensitivity();
                let label = self.motion.sensitivity().label();
                self.notification = Some((
                    Notification {
                        message: format!("Motion: {label}"),
                        level: NotificationLevel::Info,
                    },
                    Instant::now(),
                ));
            }
            Action::ToggleFullscreenPreview => {
                self.view.fullscreen_preview = !self.view.fullscreen_preview;
                self.preview.set_fullscreen(self.view.fullscreen_preview);
                self.refresh_preview_surface();
                self.view.render_dirty = true;
            }

            // ── Connection state ────────────────────────────
            Action::DaemonConnected(daemon_state) => {
                let was_disconnected = self.state.connection_status != ConnectionStatus::Connected;
                self.state.daemon = Some(daemon_state.as_ref().clone());
                self.state.connection_status = ConnectionStatus::Connected;
                self.state.disconnect_reason = None;
                self.sync_daemon_device_summary();
                if was_disconnected {
                    self.notification = Some((
                        Notification {
                            message: "Connected to daemon".to_string(),
                            level: NotificationLevel::Success,
                        },
                        Instant::now(),
                    ));
                    // Cancel any persistent connection_lost effect, then green flash
                    self.motion.cancel(crate::motion::MotionKey::ConnectionLost);
                    self.motion.trigger(
                        crate::motion::MotionKey::ConnectionRestored,
                        crate::motion::catalog::connection_restored(
                            self.last_frame_area,
                            self.motion.sensitivity(),
                        ),
                    );
                }
            }
            Action::DaemonDisconnected(reason) => {
                let was_connected = self.state.connection_status == ConnectionStatus::Connected;
                self.state.connection_status = ConnectionStatus::Disconnected;
                self.state.disconnect_reason = Some(reason.clone());
                self.state.spectrum = None;
                self.latest_canvas_frame = None;
                self.simulator_preview.latest_frame = None;
                self.simulator_preview.latest_frame_id = None;
                self.motion.spectrum_channel().clear();
                self.motion.canvas_color_channel().clear();
                self.preview.clear();
                if was_connected {
                    self.notification = Some((
                        Notification {
                            message: format!("Connection lost: {reason}"),
                            level: NotificationLevel::Warning,
                        },
                        Instant::now(),
                    ));
                    // Persistent red border tint until reconnect
                    self.motion.trigger(
                        crate::motion::MotionKey::ConnectionLost,
                        crate::motion::catalog::connection_lost(
                            self.last_frame_area,
                            self.motion.sensitivity(),
                        ),
                    );
                }
            }
            Action::DaemonReconnecting => {
                self.state.connection_status = ConnectionStatus::Reconnecting;
            }
            Action::DaemonStateUpdated(daemon_state) => {
                self.state.daemon = Some(daemon_state.as_ref().clone());
                self.sync_daemon_device_summary();
            }

            // ── Data updates ────────────────────────────────
            Action::EffectsUpdated(effects) => {
                self.state.effects.clone_from(effects.as_ref());
            }
            Action::DevicesUpdated(devices) => {
                self.state.devices.clone_from(devices.as_ref());
                self.sync_daemon_device_summary();
            }
            Action::DeviceControlSurfacesUpdated { .. }
            | Action::DeviceControlSurfacesFailed { .. }
            | Action::DeviceControlChangeApplied { .. }
            | Action::DeviceControlChangeFailed { .. }
            | Action::DeviceControlSurfaceRefreshed { .. } => {}
            Action::DeviceControlActionInvoked { result, .. } => {
                self.notification = Some((
                    Notification {
                        message: format!("Action completed: {}", result.action_id),
                        level: NotificationLevel::Success,
                    },
                    Instant::now(),
                ));
            }
            Action::DeviceControlActionFailed {
                action_id, error, ..
            } => {
                self.notification = Some((
                    Notification {
                        message: format!("Action failed: {action_id}: {error}"),
                        level: NotificationLevel::Error,
                    },
                    Instant::now(),
                ));
            }
            Action::SimulatedDisplaysUpdated(simulators) => {
                self.simulator_preview
                    .simulators
                    .clone_from(simulators.as_ref());
                self.simulator_preview.list_requested_at = Some(Instant::now());
                if let Some(simulator_id) = self.simulator_preview.source.simulator_id()
                    && !self
                        .simulator_preview
                        .simulators
                        .iter()
                        .any(|simulator| simulator.enabled && simulator.id == simulator_id)
                {
                    let _ = self
                        .action_tx
                        .send(Action::SetPreviewSource(PreviewSource::Canvas));
                }
            }
            Action::FavoritesUpdated(favorites) => {
                self.state.favorites.clone_from(favorites.as_ref());
            }
            Action::CanvasFrameReceived(frame) => {
                self.latest_canvas_frame = Some(frame.clone());
                // Sample border pixels for the canvas bleed reactive layer
                if let Some((r, g, b)) = crate::motion::sample_canvas_border(
                    frame.width,
                    frame.height,
                    frame.pixels.as_ref(),
                ) {
                    self.motion.canvas_color_channel().write(r, g, b);
                }
                if self.active_effect_browser_simulator_id().is_none() {
                    self.preview
                        .on_frame(frame.clone(), self.view.fullscreen_preview);
                }
            }
            Action::SimulatorFrameUpdated {
                simulator_id,
                frame,
            } => {
                self.simulator_preview.latest_frame = Some(frame.clone());
                self.simulator_preview.latest_frame_id = Some(simulator_id.clone());
                self.simulator_preview.frame_requested_at =
                    Some((simulator_id.clone(), Instant::now()));
                self.refresh_preview_surface();
            }
            Action::SimulatorFrameCleared(simulator_id) => {
                if self.simulator_preview.latest_frame_id.as_deref() == Some(simulator_id.as_str())
                {
                    self.simulator_preview.latest_frame = None;
                    self.simulator_preview.latest_frame_id = None;
                }
                self.simulator_preview.frame_requested_at =
                    Some((simulator_id.clone(), Instant::now()));
                self.refresh_preview_surface();
            }
            Action::SetPreviewSource(source) => {
                self.simulator_preview.source = source.clone();
                if let Some(simulator_id) = source.simulator_id()
                    && self.simulator_preview.latest_frame_id.as_deref() != Some(simulator_id)
                {
                    self.simulator_preview.latest_frame = None;
                    self.simulator_preview.latest_frame_id = None;
                }
                self.simulator_preview.frame_requested_at = None;
                self.refresh_preview_surface();
            }
            Action::SpectrumUpdated(spectrum) => {
                self.state.spectrum = Some(spectrum.clone());
                // Feed the reactive spectrum layer
                self.motion
                    .spectrum_channel()
                    .write(spectrum.bass, spectrum.level);
            }

            // ── Commands → daemon API ─────────────────────────
            Action::ApplyEffect(effect_id) if !self.is_connected() => {
                self.notify_not_connected();
                let _ = effect_id; // suppress unused warning
            }
            Action::ApplyEffectPreset(_, _)
            | Action::ToggleFavorite(_)
            | Action::UpdateControl(_, _)
            | Action::ResetControls
            | Action::LoadDeviceControls(_)
            | Action::ApplyDeviceControlChange { .. }
            | Action::InvokeDeviceControlAction { .. }
                if !self.is_connected() =>
            {
                self.notify_not_connected();
            }

            Action::ApplyEffect(effect_id) => {
                self.spawn_actions({
                    let client = self.client.clone();
                    let id = effect_id.clone();
                    async move {
                        client.apply_effect(&id, None).await?;
                        let mut actions = refresh_effects_and_status(client).await?;
                        actions.push(Action::Notify(Notification {
                            message: format!("Applied effect: {id}"),
                            level: NotificationLevel::Success,
                        }));
                        Ok(actions)
                    }
                });
            }
            Action::ApplyEffectPreset(effect_id, controls) => {
                self.spawn_actions({
                    let client = self.client.clone();
                    let id = effect_id.clone();
                    let ctrl = controls.clone();
                    async move {
                        let body = serde_json::to_value(&ctrl)?;
                        let payload = serde_json::json!({ "controls": body });
                        client.apply_effect(&id, Some(&payload)).await?;
                        let mut actions = refresh_effects_and_status(client).await?;
                        actions.push(Action::Notify(Notification {
                            message: format!("Applied preset for: {id}"),
                            level: NotificationLevel::Success,
                        }));
                        Ok(actions)
                    }
                });
            }
            Action::ToggleFavorite(effect_id) => {
                let is_fav = self.state.favorites.contains(effect_id);
                self.spawn_actions({
                    let client = self.client.clone();
                    let id = effect_id.clone();
                    async move {
                        client.toggle_favorite(&id, is_fav).await?;
                        let mut actions = refresh_favorites(client).await?;
                        actions.push(Action::Notify(Notification {
                            message: if is_fav {
                                format!("Removed from favorites: {id}")
                            } else {
                                format!("Added to favorites: {id}")
                            },
                            level: NotificationLevel::Info,
                        }));
                        Ok(actions)
                    }
                });
            }
            Action::UpdateControl(control_id, value) => {
                self.spawn_command({
                    let client = self.client.clone();
                    let id = control_id.clone();
                    let json_value = control_value_to_json(value);
                    async move {
                        client.update_control(&id, &json_value).await?;
                        Ok(Action::Tick) // silent success, no notification
                    }
                });
            }
            Action::ResetControls => {
                self.spawn_actions({
                    let client = self.client.clone();
                    async move {
                        client.reset_controls().await?;
                        let mut actions = refresh_effects(client).await?;
                        actions.push(Action::Notify(Notification {
                            message: "Controls reset to defaults".to_string(),
                            level: NotificationLevel::Info,
                        }));
                        Ok(actions)
                    }
                });
            }
            Action::LoadDeviceControls(device_id) => {
                self.spawn_command({
                    let client = self.client.clone();
                    let id = device_id.clone();
                    async move {
                        match client.get_device_control_surfaces(&id, true).await {
                            Ok(surfaces) => Ok(Action::DeviceControlSurfacesUpdated {
                                device_id: id,
                                surfaces: Arc::new(surfaces),
                            }),
                            Err(error) => Ok(Action::DeviceControlSurfacesFailed {
                                device_id: id,
                                error: error.to_string(),
                            }),
                        }
                    }
                });
            }
            Action::ApplyDeviceControlChange {
                device_id,
                surface_id,
                expected_revision,
                field_id,
                value,
            } => {
                self.spawn_command({
                    let client = self.client.clone();
                    let device_id = device_id.clone();
                    let surface_id = surface_id.clone();
                    let request = ApplyControlChangesRequest {
                        surface_id: surface_id.clone(),
                        expected_revision: Some(*expected_revision),
                        changes: vec![ControlChange {
                            field_id: field_id.clone(),
                            value: value.clone(),
                        }],
                        dry_run: false,
                    };
                    async move {
                        match client.apply_control_changes(&request).await {
                            Ok(response) => Ok(Action::DeviceControlChangeApplied {
                                device_id,
                                response: Arc::new(response),
                            }),
                            Err(error) => Ok(Action::DeviceControlChangeFailed {
                                device_id,
                                surface_id,
                                error: error.to_string(),
                            }),
                        }
                    }
                });
            }
            Action::InvokeDeviceControlAction {
                device_id,
                surface_id,
                action_id,
                input,
            } => {
                self.spawn_actions({
                    let client = self.client.clone();
                    let device_id = device_id.clone();
                    let surface_id = surface_id.clone();
                    let action_id = action_id.clone();
                    let input = input.clone();
                    async move {
                        match client
                            .invoke_control_action(&surface_id, &action_id, input)
                            .await
                        {
                            Ok(result) => {
                                let refreshed_surface_id = result.surface_id.clone();
                                let mut actions = vec![Action::DeviceControlActionInvoked {
                                    device_id: device_id.clone(),
                                    result: Arc::new(result),
                                }];
                                match client.get_control_surface(&refreshed_surface_id).await {
                                    Ok(surface) => {
                                        actions.push(Action::DeviceControlSurfaceRefreshed {
                                            device_id,
                                            surface: Arc::new(surface),
                                        });
                                    }
                                    Err(error) => {
                                        tracing::debug!(
                                            %refreshed_surface_id,
                                            %error,
                                            "Failed to refresh dynamic control surface after action"
                                        );
                                    }
                                }
                                Ok(actions)
                            }
                            Err(error) => Ok(vec![Action::DeviceControlActionFailed {
                                device_id,
                                surface_id,
                                action_id,
                                error: error.to_string(),
                            }]),
                        }
                    }
                });
            }

            // ── Notifications ───────────────────────────────
            Action::Notify(notif) => {
                // Error notifications trigger a quick red flash over the
                // full content area in addition to the toast.
                if notif.level == NotificationLevel::Error {
                    self.motion.trigger(
                        crate::motion::MotionKey::ErrorFlash,
                        crate::motion::catalog::error_flash(
                            self.last_frame_area,
                            self.motion.sensitivity(),
                        ),
                    );
                }
                self.notification = Some((notif.clone(), Instant::now()));
            }
            Action::OpenDonate => {
                let _ = open::that("https://github.com/sponsors/hyperb1iss");
            }
            Action::DismissNotification => {
                self.notification = None;
            }

            // ── Tick: auto-dismiss notifications ──
            Action::Tick => {
                if let Some((_, created)) = &self.notification
                    && created.elapsed() > Duration::from_secs(5)
                {
                    self.notification = None;
                }
                self.check_idle();
                self.poll_simulated_displays();
                self.poll_selected_simulator_frame();
            }

            _ => {}
        }

        // Broadcast state-update actions to all screens so inactive views
        // stay current; forward other actions only to the active screen.
        let broadcast = matches!(
            action,
            Action::DaemonConnected(_)
                | Action::DaemonStateUpdated(_)
                | Action::DaemonDisconnected(_)
                | Action::DaemonReconnecting
                | Action::EffectsUpdated(_)
                | Action::DevicesUpdated(_)
                | Action::DeviceControlSurfacesUpdated { .. }
                | Action::DeviceControlSurfacesFailed { .. }
                | Action::DeviceControlChangeApplied { .. }
                | Action::DeviceControlChangeFailed { .. }
                | Action::DeviceControlActionInvoked { .. }
                | Action::DeviceControlSurfaceRefreshed { .. }
                | Action::DeviceControlActionFailed { .. }
                | Action::SimulatedDisplaysUpdated(_)
                | Action::FavoritesUpdated(_)
                | Action::CanvasFrameReceived(_)
                | Action::SimulatorFrameUpdated { .. }
                | Action::SimulatorFrameCleared(_)
                | Action::SpectrumUpdated(_)
        );

        if broadcast {
            for screen in self.screens.values_mut() {
                if let Ok(Some(follow_up)) = screen.update(action) {
                    let _ = self.action_tx.send(follow_up);
                }
            }
        } else if let Some(screen) = self.screens.get_mut(&self.active_screen)
            && let Ok(Some(follow_up)) = screen.update(action)
        {
            let _ = self.action_tx.send(follow_up);
        }
    }

    fn available_screens(&self) -> &[ScreenId] {
        &self.available_screens
    }

    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    fn sync_daemon_device_summary(&mut self) {
        let Some(daemon) = self.state.daemon.as_mut() else {
            return;
        };

        daemon.device_count = self.state.devices.len() as u32;
        daemon.total_leds = self
            .state
            .devices
            .iter()
            .map(|device| device.led_count)
            .sum();
    }

    /// Whether the daemon connection is active.
    fn is_connected(&self) -> bool {
        self.state.connection_status == ConnectionStatus::Connected
    }

    fn active_effect_browser_simulator_id(&self) -> Option<&str> {
        if self.active_screen == ScreenId::EffectBrowser {
            self.simulator_preview.source.simulator_id()
        } else {
            None
        }
    }

    fn refresh_preview_surface(&mut self) {
        if let Some(simulator_id) = self.active_effect_browser_simulator_id() {
            if self.simulator_preview.latest_frame_id.as_deref() == Some(simulator_id) {
                if let Some(frame) = self.simulator_preview.latest_frame.clone() {
                    self.preview.on_frame(frame, self.view.fullscreen_preview);
                } else {
                    self.preview.clear();
                }
            } else {
                self.preview.clear();
            }
            return;
        }

        if let Some(frame) = self.latest_canvas_frame.clone() {
            self.preview.on_frame(frame, self.view.fullscreen_preview);
        } else {
            self.preview.clear();
        }
    }

    fn poll_simulated_displays(&mut self) {
        if self.active_screen != ScreenId::EffectBrowser || !self.is_connected() {
            return;
        }

        let should_refresh = self
            .simulator_preview
            .list_requested_at
            .is_none_or(|requested_at| requested_at.elapsed() >= SIMULATOR_LIST_REFRESH_INTERVAL);
        if !should_refresh {
            return;
        }

        self.simulator_preview.list_requested_at = Some(Instant::now());
        let tx = self.action_tx.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            match client.get_simulated_displays().await {
                Ok(simulators) => {
                    let _ = tx.send(Action::SimulatedDisplaysUpdated(Arc::new(simulators)));
                }
                Err(error) => {
                    tracing::debug!(%error, "Failed to refresh simulated display list");
                }
            }
        });
    }

    fn poll_selected_simulator_frame(&mut self) {
        if !self.is_connected() {
            return;
        }

        let Some(simulator_id) = self.active_effect_browser_simulator_id() else {
            return;
        };

        let should_refresh = self
            .simulator_preview
            .frame_requested_at
            .as_ref()
            .is_none_or(|(requested_id, requested_at)| {
                requested_id != simulator_id
                    || requested_at.elapsed() >= SIMULATOR_FRAME_REFRESH_INTERVAL
            });
        if !should_refresh {
            return;
        }

        let simulator_id = simulator_id.to_owned();
        self.simulator_preview.frame_requested_at = Some((simulator_id.clone(), Instant::now()));
        let tx = self.action_tx.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            match client.get_simulated_display_frame(&simulator_id).await {
                Ok(Some(frame)) => {
                    let _ = tx.send(Action::SimulatorFrameUpdated {
                        simulator_id,
                        frame: Arc::new(frame),
                    });
                }
                Ok(None) => {
                    let _ = tx.send(Action::SimulatorFrameCleared(simulator_id));
                }
                Err(error) => {
                    tracing::debug!(
                        %error,
                        simulator_id = %simulator_id,
                        "Failed to refresh simulator frame"
                    );
                }
            }
        });
    }

    /// Show a "not connected" notification (debounced — won't replace an existing one).
    fn notify_not_connected(&mut self) {
        if self.notification.is_none() {
            self.notification = Some((
                Notification {
                    message: "Not connected to daemon".to_string(),
                    level: NotificationLevel::Warning,
                },
                Instant::now(),
            ));
        }
    }

    /// Spawn async work that can emit multiple follow-up actions.
    fn spawn_actions<F>(&self, fut: F)
    where
        F: std::future::Future<Output = anyhow::Result<Vec<Action>>> + Send + 'static,
    {
        let tx = self.action_tx.clone();
        tokio::spawn(async move {
            match fut.await {
                Ok(actions) => {
                    for action in actions {
                        let _ = tx.send(action);
                    }
                }
                Err(e) => {
                    let _ = tx.send(Action::Notify(Notification {
                        message: format!("Command failed: {e}"),
                        level: NotificationLevel::Error,
                    }));
                }
            }
        });
    }

    /// Spawn an async command that sends a follow-up action on completion.
    fn spawn_command<F>(&self, fut: F)
    where
        F: std::future::Future<Output = anyhow::Result<Action>> + Send + 'static,
    {
        self.spawn_actions(async move { fut.await.map(|action| vec![action]) });
    }

    /// Render the full TUI frame.
    fn render(&mut self, frame: &mut Frame) {
        use ratatui::layout::Rect;
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::Paragraph;

        let area = frame.area();
        self.last_frame_area = area;

        // Fullscreen canvas preview — bypass all chrome
        if self.view.fullscreen_preview {
            self.render_fullscreen_preview(frame, area);
            return;
        }

        // Chrome renders the shell and returns the content area
        let content_area = self
            .chrome
            .render(frame, area, &self.state, self.available_screens());

        // Active screen fills the content area, then App overlays the live
        // canvas preview using ratatui-image (Kitty/Sixel/halfblocks). The
        // screen records its preview rect via Component::canvas_preview_area;
        // we drop the immutable borrow before reaching for canvas_protocol.
        let preview_area = if let Some(screen) = self.screens.get(&self.active_screen) {
            screen.render(frame, content_area);
            screen.canvas_preview_area()
        } else {
            None
        };

        self.preview.render(preview_area, frame.buffer_mut());

        // Render notification toast (centered, overlays content bottom)
        if let Some((notif, _)) = &self.notification {
            let color = match notif.level {
                NotificationLevel::Success => crate::theme::success(),
                NotificationLevel::Error => crate::theme::error(),
                NotificationLevel::Warning => crate::theme::warning(),
                NotificationLevel::Info => crate::theme::accent_primary(),
            };
            let icon = match notif.level {
                NotificationLevel::Success => "\u{2714} ",
                NotificationLevel::Error => "\u{2718} ",
                NotificationLevel::Warning => "\u{26A0} ",
                NotificationLevel::Info => "\u{2139} ",
            };
            let text = format!(" {icon}{} ", notif.message);
            #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
            let width = (text.len() as u16).min(area.width.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(width)) / 2;
            let y = area.y + area.height.saturating_sub(3);

            let toast = Paragraph::new(Line::from(Span::styled(
                text,
                Style::default()
                    .fg(crate::theme::bg_base())
                    .bg(color)
                    .add_modifier(Modifier::BOLD),
            )));
            frame.render_widget(toast, Rect::new(x, y, width, 1));
        }

        // Help overlay (modal)
        if self.view.help_visible {
            self.render_help(frame, area);
        }

        // Theme picker (modal — top of z-order)
        if let Some(picker) = self.theme_picker.as_mut() {
            picker.render(frame, area);
        }

        // Motion effects post-process the composed buffer
        self.motion.tick(frame.buffer_mut(), area);
    }

    /// Render a centered help overlay listing all keybindings.
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    fn render_help(&self, frame: &mut Frame, area: Rect) {
        use ratatui::layout::Rect;
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders, Clear, Paragraph};

        let mut bindings: Vec<(String, String)> = vec![
            ("q".to_string(), "Quit".to_string()),
            ("?".to_string(), "Toggle help".to_string()),
            ("T".to_string(), "Theme picker".to_string()),
            ("M".to_string(), "Motion sensitivity".to_string()),
            ("Tab".to_string(), "Switch pane / control".to_string()),
            ("Z".to_string(), "Fullscreen preview".to_string()),
            ("Esc".to_string(), "Go back".to_string()),
        ];
        if self.state.show_donate {
            bindings.push(("$".to_string(), "Sponsor project".to_string()));
        }
        bindings.extend(self.available_screens().iter().copied().map(|screen| {
            (
                screen.key_hint().to_ascii_lowercase().to_string(),
                screen.to_string(),
            )
        }));
        bindings.extend([
            (String::new(), String::new()),
            (
                "\u{2191}/\u{2193}".to_string(),
                "Navigate up/down".to_string(),
            ),
            ("\u{2190}/\u{2192}".to_string(), "Adjust value".to_string()),
            ("Enter".to_string(), "Apply / confirm".to_string()),
            ("f".to_string(), "Toggle favorite".to_string()),
            ("/".to_string(), "Search".to_string()),
            ("g/G".to_string(), "Jump to top/bottom".to_string()),
        ]);

        let width = 40u16.min(area.width.saturating_sub(4));
        let height = (bindings.len() as u16 + 2).min(area.height.saturating_sub(4));
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let help_area = Rect::new(x, y, width, height);

        // Clear the area behind the overlay
        frame.render_widget(Clear, help_area);

        let lines: Vec<Line<'_>> = bindings
            .iter()
            .map(|(key, desc)| {
                if key.is_empty() {
                    Line::raw("")
                } else {
                    Line::from(vec![
                        Span::styled(
                            format!("  {key:<10}"),
                            Style::default()
                                .fg(Color::Rgb(128, 255, 234))
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(desc, Style::default().fg(Color::Rgb(248, 248, 242))),
                    ])
                }
            })
            .collect();

        let block = Block::default()
            .title(" Keybindings ")
            .title_style(
                Style::default()
                    .fg(Color::Rgb(225, 53, 255))
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(90, 21, 102)))
            .style(Style::default().bg(Color::Rgb(30, 30, 46)));

        let help = Paragraph::new(lines).block(block);
        frame.render_widget(help, help_area);
    }

    /// Render fullscreen canvas preview with a subtle status line.
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    fn render_fullscreen_preview(&mut self, frame: &mut Frame, area: Rect) {
        use ratatui::layout::Rect;
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Paragraph};

        if area.height < 2 || area.width == 0 {
            return;
        }

        // Reserve bottom row for a minimal info bar
        let canvas_area = Rect::new(area.x, area.y, area.width, area.height - 1);
        let info_area = Rect::new(area.x, area.y + area.height - 1, area.width, 1);

        // Render canvas via the live ratatui-image protocol — Kitty graphics
        // on Ghostty/Kitty, Sixel on supporting terminals, halfblocks elsewhere.
        // Picker is auto-selected at startup so this branch always works once
        // a frame has arrived.
        self.preview.render(Some(canvas_area), frame.buffer_mut());

        if !self.preview.has_current_frame() {
            // No canvas — fill with dark background
            let block = Block::default().style(Style::default().bg(Color::Rgb(20, 20, 30)));
            frame.render_widget(block, canvas_area);

            // Centered "no signal" text
            let msg = if self.active_effect_browser_simulator_id().is_some() {
                "No simulator frame yet"
            } else {
                "No canvas data — apply an effect to preview"
            };
            let msg_width = (msg.len() as u16).min(canvas_area.width);
            let msg_x = canvas_area.x + (canvas_area.width.saturating_sub(msg_width)) / 2;
            let msg_y = canvas_area.y + canvas_area.height / 2;
            let text = Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(Color::Rgb(100, 100, 120)),
            )));
            frame.render_widget(text, Rect::new(msg_x, msg_y, msg_width, 1));
        }

        // Info bar: effect name + hint to exit
        let effect_name = if let Some(simulator_id) = self.active_effect_browser_simulator_id() {
            self.simulator_preview
                .simulators
                .iter()
                .find(|simulator| simulator.id == simulator_id)
                .map_or("Simulator", |simulator| simulator.name.as_str())
        } else {
            self.state
                .daemon
                .as_ref()
                .and_then(|d| d.effect_name.as_deref())
                .unwrap_or("—")
        };

        let fps = self.state.daemon.as_ref().map_or(0.0, |d| d.fps_actual);

        let left_preview = " PREVIEW ";
        let left_name = format!(" {effect_name} ");
        let left_fps = format!("{fps:.0} fps");
        let right_hint = "Z/Esc to exit ";
        let used: u16 = (left_preview.len() + left_name.len() + left_fps.len() + right_hint.len())
            .try_into()
            .unwrap_or(0);
        let pad = area.width.saturating_sub(used);

        let muted = Style::default().fg(Color::Rgb(100, 100, 120));
        let info_line = Line::from(vec![
            Span::styled(
                left_preview,
                Style::default()
                    .fg(Color::Rgb(20, 20, 30))
                    .bg(Color::Rgb(225, 53, 255))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                left_name,
                Style::default()
                    .fg(Color::Rgb(128, 255, 234))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(left_fps, muted),
            Span::styled(" ".repeat(pad.into()), muted),
            Span::styled(right_hint, muted),
        ]);

        let info = Paragraph::new(info_line).style(Style::default().bg(Color::Rgb(20, 20, 30)));
        frame.render_widget(info, info_area);
    }
}

/// Convert a `ControlValue` to a JSON value for the REST API.
fn control_value_to_json(value: &ControlValue) -> serde_json::Value {
    match value {
        ControlValue::Float(v) => serde_json::json!(v),
        ControlValue::Integer(v) => serde_json::json!(v),
        ControlValue::Boolean(v) => serde_json::json!(v),
        ControlValue::Color(c) => serde_json::json!(c),
        ControlValue::Text(s) => serde_json::json!(s),
    }
}

async fn refresh_effects(client: DaemonClient) -> anyhow::Result<Vec<Action>> {
    let effects = client.get_effects().await?;
    Ok(vec![Action::EffectsUpdated(std::sync::Arc::new(effects))])
}

async fn refresh_favorites(client: DaemonClient) -> anyhow::Result<Vec<Action>> {
    let favorites = client.get_favorites().await?;
    Ok(vec![Action::FavoritesUpdated(std::sync::Arc::new(
        favorites,
    ))])
}

async fn refresh_effects_and_status(client: DaemonClient) -> anyhow::Result<Vec<Action>> {
    let status = client.get_status().await?;
    let effects = client.get_effects().await?;
    Ok(vec![
        Action::DaemonStateUpdated(Box::new(status)),
        Action::EffectsUpdated(std::sync::Arc::new(effects)),
    ])
}
