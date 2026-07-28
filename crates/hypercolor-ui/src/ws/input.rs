//! Browser-preview input injection — upstream `input_inject` client messages.
//!
//! Wire-shaped mirror of the daemon's `BrowserInputEdgeWire` (spec 71 W4):
//! the daemon stamps a per-connection `source_id`, folds edges into the
//! interaction state, and synthesizes releases on socket close. Injection is
//! control-tier authorized server-side; read-only sockets receive a
//! `forbidden` protocol error and no state changes.

use serde::Serialize;

/// One injected input edge, serialized exactly as the daemon's
/// `input_inject` message expects.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputInjectEdge {
    Key {
        key: String,
        state: InputEdgeState,
    },
    Button {
        button: InputEdgeButton,
        state: InputEdgeState,
    },
    Move {
        nx: f32,
        ny: f32,
    },
    Wheel {
        delta_hi_res: i32,
    },
}

/// Press state for key and button edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEdgeState {
    Pressed,
    Released,
    Repeated,
}

/// Pointer button identity for button edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEdgeButton {
    Left,
    Right,
    Middle,
}

impl InputEdgeButton {
    /// Map a `PointerEvent.button` index to a wire button. Buttons beyond
    /// the primary three (back/forward) have no wire identity and are
    /// dropped at the call site.
    #[must_use]
    pub fn from_pointer_button(button: i16) -> Option<Self> {
        match button {
            0 => Some(Self::Left),
            1 => Some(Self::Middle),
            2 => Some(Self::Right),
            _ => None,
        }
    }
}
