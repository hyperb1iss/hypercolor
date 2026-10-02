//! Device-attributed input vocabulary published by hardware drivers.
//!
//! Some hardware that Hypercolor lights can also be played: touch surfaces,
//! keybeds, and control buttons. Drivers report what they observe here in
//! physical terms, and the interaction pipeline folds those edges into held
//! state and routed [`InputEvent`](crate::event::InputEvent)s.

use std::sync::Arc;

use crate::event::InputButtonState;

/// A normalized position and pressure on a device touch surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPosition {
    /// Horizontal position in `[0, 1]`, from the surface's left edge.
    pub x: f32,
    /// Vertical position in `[0, 1]`, from the surface's top edge.
    pub y: f32,
    /// Contact pressure in `[0, 1]`.
    pub pressure: f32,
}

/// One input edge a driver observed on a device it owns.
///
/// Contacts are identified per device and stay stable from
/// [`TouchBegan`](Self::TouchBegan) through [`TouchEnded`](Self::TouchEnded).
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceInputEdge {
    /// A contact landed on the surface.
    TouchBegan {
        contact: u32,
        position: TouchPosition,
    },
    /// A held contact moved or changed pressure.
    TouchMoved {
        contact: u32,
        position: TouchPosition,
    },
    /// A contact lifted from the surface.
    TouchEnded {
        contact: u32,
        position: TouchPosition,
    },
    /// A named control button changed state.
    Button {
        button: Arc<str>,
        state: InputButtonState,
    },
}
