//! Component trait — the universal interface for all TUI elements.

use anyhow::Result;
use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;

/// Trait implemented by all screen views and interactive components.
///
/// Follows the Unifly component lifecycle:
/// `init` → `handle_key_event`/`handle_mouse_event` → `update` → `render`
pub trait Component: Send {
    /// Called once when the component is first mounted. Receives the action
    /// sender for dispatching deferred actions.
    fn init(&mut self, _action_tx: UnboundedSender<Action>) -> Result<()> {
        Ok(())
    }

    /// Handle a keyboard event. Return an `Action` to dispatch, or `None`.
    fn handle_key_event(&mut self, _key: KeyEvent) -> Result<Option<Action>> {
        Ok(None)
    }

    /// Handle a mouse event. Return an `Action` to dispatch, or `None`.
    fn handle_mouse_event(&mut self, _mouse: MouseEvent) -> Result<Option<Action>> {
        Ok(None)
    }

    /// Process a dispatched action. May return a follow-up action.
    fn update(&mut self, _action: &Action) -> Result<Option<Action>> {
        Ok(None)
    }

    /// Render the component into the given frame region.
    fn render(&self, frame: &mut Frame, area: Rect);

    /// Whether this component currently holds keyboard focus.
    fn focused(&self) -> bool {
        false
    }

    /// Set the focus state of this component.
    fn set_focused(&mut self, _focused: bool) {}

    /// Stable identifier for this component.
    fn id(&self) -> &'static str;
}
