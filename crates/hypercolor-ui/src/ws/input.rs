//! Browser-preview input injection: upstream `input_inject` client messages.
//!
//! Wire-shaped mirror of the daemon's `BrowserInputEdgeWire`: the daemon
//! stamps a per-connection `source_id`, folds edges into the
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
    Scroll {
        delta_x_q16_16: i64,
        delta_y_q16_16: i64,
        unit: InputEdgeScrollUnit,
        phase: InputEdgeScrollPhase,
        momentum_phase: InputEdgeScrollPhase,
    },
}

/// Coordinate unit for an exact two-axis scroll edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEdgeScrollUnit {
    Line120,
    Pixels,
}

/// Lifecycle phase for an exact scroll edge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEdgeScrollPhase {
    #[default]
    None,
    MayBegin,
    Began,
    Changed,
    Stationary,
    Ended,
    Cancelled,
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
