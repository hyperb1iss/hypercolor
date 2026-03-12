//! Action enum — the sole mechanism for state mutation in the TUI.

use std::sync::Arc;

use crate::screen::ScreenId;
use crate::state::{
    CanvasFrame, ControlValue, DaemonState, DeviceSummary, EffectSummary, Notification,
    SpectrumSnapshot,
};

/// Every state change in the TUI flows through an Action.
#[derive(Debug, Clone)]
pub enum Action {
    // ── Lifecycle ────────────────────────────────────────────
    /// Quit the TUI (daemon keeps running).
    Quit,
    /// Periodic tick for data refresh and animation.
    Tick,
    /// Time to redraw the terminal.
    Render,
    /// Terminal was resized.
    Resize(u16, u16),

    // ── Navigation ──────────────────────────────────────────
    /// Switch to a different screen.
    SwitchScreen(ScreenId),
    /// Return to the previous screen.
    GoBack,
    /// Cycle focus to the next panel.
    FocusNext,
    /// Cycle focus to the previous panel.
    FocusPrev,

    // ── Daemon Connection ───────────────────────────────────
    /// Daemon connected, initial state received.
    DaemonConnected(Box<DaemonState>),
    /// Daemon connection lost.
    DaemonDisconnected(String),
    /// Attempting to reconnect.
    DaemonReconnecting,
    /// Daemon state updated (periodic refresh).
    DaemonStateUpdated(Box<DaemonState>),

    // ── Data Updates (from DataBridge) ──────────────────────
    /// Effect list refreshed.
    EffectsUpdated(Arc<Vec<EffectSummary>>),
    /// Device list refreshed.
    DevicesUpdated(Arc<Vec<DeviceSummary>>),
    /// Favorites list refreshed.
    FavoritesUpdated(Arc<Vec<String>>),
    /// New canvas frame received (binary WS).
    CanvasFrameReceived(Arc<CanvasFrame>),
    /// New spectrum snapshot received (binary WS).
    SpectrumUpdated(Arc<SpectrumSnapshot>),

    // ── Effect Browser ──────────────────────────────────────
    /// Select an effect in the browser list.
    SelectEffect(usize),
    /// Apply an effect by ID.
    ApplyEffect(String),
    /// Toggle favorite status for an effect.
    ToggleFavorite(String),
    /// Open the search filter.
    OpenSearch,
    /// Close the search filter.
    CloseSearch,
    /// Update the search query text.
    SearchInput(String),

    // ── Effect Control ──────────────────────────────────────
    /// Update a control value on the active effect.
    UpdateControl(String, ControlValue),
    /// Apply a preset by name.
    ApplyPreset(String),
    /// Reset all controls to defaults.
    ResetControls,

    // ── UI State ────────────────────────────────────────────
    /// Toggle the help overlay.
    ToggleHelp,
    /// Show a transient notification.
    Notify(Notification),
    /// Dismiss the current notification.
    DismissNotification,

    // ── Scroll ──────────────────────────────────────────────
    /// Scroll up in the focused list.
    ScrollUp,
    /// Scroll down in the focused list.
    ScrollDown,
    /// Page up in the focused list.
    PageUp,
    /// Page down in the focused list.
    PageDown,
    /// Jump to top.
    ScrollToTop,
    /// Jump to bottom.
    ScrollToBottom,
}
