//! App — the central coordinator and main event loop.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::action::Action;
use crate::chrome::Chrome;
use crate::component::Component;
use crate::event::{Event, EventReader};
use crate::screen::ScreenId;
use crate::state::{AppState, ConnectionStatus, Notification};

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

            // ── Notifications ───────────────────────────────
            Action::Notify(notif) => {
                self.notification = Some((notif.clone(), Instant::now()));
            }
            Action::DismissNotification => {
                self.notification = None;
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

    /// Render the full TUI frame.
    fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        // Chrome renders the shell and returns the content area
        let content_area = self.chrome.render(frame, area, &self.state);

        // Active screen fills the content area
        if let Some(screen) = self.screens.get(&self.active_screen) {
            screen.render(frame, content_area);
        }

        // Auto-dismiss stale notifications (> 5 seconds)
        // (checked on next tick, not here — render is pure)
    }
}
