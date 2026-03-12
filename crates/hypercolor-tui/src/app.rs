//! App — the central coordinator and main event loop.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::action::Action;
use crate::chrome::Chrome;
use crate::client::rest::DaemonClient;
use crate::component::Component;
use crate::event::{Event, EventReader};
use crate::screen::ScreenId;
use crate::state::{AppState, ConnectionStatus, ControlValue, Notification, NotificationLevel};

/// Top-level application that owns all screens and drives the event loop.
pub struct App {
    /// The currently active screen.
    active_screen: ScreenId,
    /// The previously active screen (for `GoBack`).
    previous_screen: Option<ScreenId>,
    /// All registered screens, keyed by ID.
    screens: HashMap<ScreenId, Box<dyn Component>>,
    /// Persistent chrome (title bar, LED strip, nav, audio, status).
    chrome: Chrome,
    /// Shared state accessible by all components.
    state: AppState,
    /// Whether the app is running.
    running: bool,
    /// Help overlay visible.
    help_visible: bool,
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
}

impl App {
    /// Create a new app targeting the given daemon.
    pub fn new(host: String, port: u16) -> Self {
        let (action_tx, action_rx) = mpsc::unbounded_channel();
        let screens = crate::views::create_screens();

        let client = DaemonClient::new(&host, port);

        Self {
            active_screen: ScreenId::Dashboard,
            previous_screen: None,
            screens: screens.into_iter().collect(),
            chrome: Chrome::new(),
            state: AppState::default(),
            running: true,
            help_visible: false,
            notification: None,
            action_tx,
            action_rx,
            client,
            data_cancel: CancellationToken::new(),
            host,
            port,
        }
    }

    /// Run the main event loop.
    pub async fn run(&mut self) -> Result<()> {
        // Initialize terminal
        let mut terminal = ratatui::init();

        // Initialize all screens
        for screen in self.screens.values_mut() {
            screen.init(self.action_tx.clone())?;
        }
        if let Some(screen) = self.screens.get_mut(&self.active_screen) {
            screen.set_focused(true);
        }

        // Spawn data bridge
        let bridge_tx = self.action_tx.clone();
        let bridge_cancel = self.data_cancel.clone();
        let bridge_host = self.host.clone();
        let bridge_port = self.port;
        tokio::spawn(async move {
            crate::bridge::spawn_data_bridge(bridge_host, bridge_port, bridge_tx, bridge_cancel)
                .await;
        });

        // Event reader: 250ms tick, ~15 FPS render
        let mut events = EventReader::new(Duration::from_millis(250), Duration::from_millis(66));

        tracing::info!("TUI event loop started");

        while self.running {
            let Some(event) = events.next().await else {
                break;
            };

            // Map event → action
            match event {
                Event::Key(key) => {
                    if let Some(action) = self.handle_key_event(key) {
                        let _ = self.action_tx.send(action);
                    }
                }
                Event::Mouse(mouse) => {
                    if let Some(screen) = self.screens.get_mut(&self.active_screen)
                        && let Ok(Some(action)) = screen.handle_mouse_event(mouse)
                    {
                        let _ = self.action_tx.send(action);
                    }
                }
                Event::Resize(w, h) => {
                    let _ = self.action_tx.send(Action::Resize(w, h));
                }
                Event::Tick => {
                    let _ = self.action_tx.send(Action::Tick);
                }
                Event::Render => {
                    let _ = self.action_tx.send(Action::Render);
                }
            }

            // Drain and process all queued actions
            while let Ok(action) = self.action_rx.try_recv() {
                if let Action::Render = action {
                    terminal.draw(|frame| self.render(frame))?;
                } else {
                    self.process_action(&action);
                }
            }
        }

        // Cleanup
        self.data_cancel.cancel();
        events.stop();
        ratatui::restore();
        tracing::info!("TUI event loop ended");
        Ok(())
    }

    /// Handle a key event, returning an action to dispatch.
    fn handle_key_event(&mut self, key: KeyEvent) -> Option<Action> {
        // Global keybindings (always active)
        match key.code {
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Char('?') => return Some(Action::ToggleHelp),
            KeyCode::Tab => return Some(Action::FocusNext),
            KeyCode::BackTab => return Some(Action::FocusPrev),
            KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                if let Some(screen) = ScreenId::from_key(c)
                    && screen != self.active_screen
                {
                    return Some(Action::SwitchScreen(screen));
                }
            }
            KeyCode::Esc => {
                if self.help_visible {
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

    /// Process a single action, updating state and forwarding to components.
    #[allow(clippy::too_many_lines)]
    fn process_action(&mut self, action: &Action) {
        match action {
            Action::Quit => self.running = false,

            Action::SwitchScreen(screen_id) => {
                if let Some(old) = self.screens.get_mut(&self.active_screen) {
                    old.set_focused(false);
                }
                self.previous_screen = Some(self.active_screen);
                self.active_screen = *screen_id;
                self.state.active_screen = *screen_id;
                if let Some(new) = self.screens.get_mut(&self.active_screen) {
                    new.set_focused(true);
                }
            }

            Action::GoBack => {
                if let Some(prev) = self.previous_screen {
                    let _ = self.action_tx.send(Action::SwitchScreen(prev));
                }
            }

            Action::ToggleHelp => {
                self.help_visible = !self.help_visible;
            }

            // ── Connection state ────────────────────────────
            Action::DaemonConnected(daemon_state) => {
                self.state.daemon = Some(daemon_state.as_ref().clone());
                self.state.connection_status = ConnectionStatus::Connected;
            }
            Action::DaemonDisconnected(_reason) => {
                self.state.connection_status = ConnectionStatus::Disconnected;
            }
            Action::DaemonReconnecting => {
                self.state.connection_status = ConnectionStatus::Reconnecting;
            }
            Action::DaemonStateUpdated(daemon_state) => {
                self.state.daemon = Some(daemon_state.as_ref().clone());
            }

            // ── Data updates ────────────────────────────────
            Action::EffectsUpdated(effects) => {
                self.state.effects.clone_from(effects.as_ref());
            }
            Action::DevicesUpdated(devices) => {
                self.state.devices.clone_from(devices.as_ref());
            }
            Action::FavoritesUpdated(favorites) => {
                self.state.favorites.clone_from(favorites.as_ref());
            }
            Action::CanvasFrameReceived(frame) => {
                self.state.canvas_frame = Some(frame.as_ref().clone());
            }
            Action::SpectrumUpdated(spectrum) => {
                self.state.spectrum = Some(spectrum.as_ref().clone());
            }

            // ── Commands → daemon API ─────────────────────────
            Action::ApplyEffect(effect_id) => {
                self.spawn_command({
                    let client = self.client.clone();
                    let id = effect_id.clone();
                    async move {
                        client.apply_effect(&id, None).await?;
                        Ok(Action::Notify(Notification {
                            message: format!("Applied effect: {id}"),
                            level: NotificationLevel::Success,
                        }))
                    }
                });
            }
            Action::ToggleFavorite(effect_id) => {
                let is_fav = self.state.favorites.contains(effect_id);
                self.spawn_command({
                    let client = self.client.clone();
                    let id = effect_id.clone();
                    async move {
                        client.toggle_favorite(&id, is_fav).await?;
                        Ok(Action::Notify(Notification {
                            message: if is_fav {
                                format!("Removed from favorites: {id}")
                            } else {
                                format!("Added to favorites: {id}")
                            },
                            level: NotificationLevel::Info,
                        }))
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
                self.spawn_command({
                    let client = self.client.clone();
                    async move {
                        client.reset_controls().await?;
                        Ok(Action::Notify(Notification {
                            message: "Controls reset to defaults".to_string(),
                            level: NotificationLevel::Info,
                        }))
                    }
                });
            }

            // ── Notifications ───────────────────────────────
            Action::Notify(notif) => {
                self.notification = Some((notif.clone(), Instant::now()));
            }
            Action::DismissNotification => {
                self.notification = None;
            }

            // ── Tick: auto-dismiss notifications ─────────────
            Action::Tick => {
                if let Some((_, created)) = &self.notification
                    && created.elapsed() > Duration::from_secs(5)
                {
                    self.notification = None;
                }
            }

            _ => {}
        }

        // Forward to active screen
        if let Some(screen) = self.screens.get_mut(&self.active_screen)
            && let Ok(Some(follow_up)) = screen.update(action)
        {
            let _ = self.action_tx.send(follow_up);
        }
    }

    /// Spawn an async command that sends a follow-up action on completion.
    fn spawn_command<F>(&self, fut: F)
    where
        F: std::future::Future<Output = anyhow::Result<Action>> + Send + 'static,
    {
        let tx = self.action_tx.clone();
        tokio::spawn(async move {
            match fut.await {
                Ok(action) => {
                    let _ = tx.send(action);
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

    /// Render the full TUI frame.
    fn render(&self, frame: &mut Frame) {
        use ratatui::layout::Rect;
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::Paragraph;

        let area = frame.area();

        // Chrome renders the shell and returns the content area
        let content_area = self.chrome.render(frame, area, &self.state);

        // Active screen fills the content area
        if let Some(screen) = self.screens.get(&self.active_screen) {
            screen.render(frame, content_area);
        }

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
        if self.help_visible {
            Self::render_help(frame, area);
        }
    }

    /// Render a centered help overlay listing all keybindings.
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    fn render_help(frame: &mut Frame, area: Rect) {
        use ratatui::layout::Rect;
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders, Clear, Paragraph};

        let bindings = [
            ("q", "Quit"),
            ("?", "Toggle help"),
            ("Tab", "Focus next panel"),
            ("Esc", "Go back"),
            ("d", "Dashboard"),
            ("e", "Effect Browser"),
            ("c", "Effect Control"),
            ("v", "Device Manager"),
            ("p", "Profiles"),
            ("s", "Settings"),
            ("b", "Debug"),
            ("", ""),
            ("j/k", "Navigate up/down"),
            ("h/l", "Adjust value"),
            ("Enter", "Apply / confirm"),
            ("f", "Toggle favorite"),
            ("/", "Search"),
            ("g/G", "Jump to top/bottom"),
        ];

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
                        Span::styled(*desc, Style::default().fg(Color::Rgb(248, 248, 242))),
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
