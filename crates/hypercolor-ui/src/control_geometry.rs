//! Shared geometry helpers for interactive effect controls.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl FrameRect {
    pub(crate) const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(crate) fn right(self) -> f32 {
        self.x + self.width
    }

    pub(crate) fn bottom(self) -> f32 {
        self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameHandle {
    Move,
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}

pub(crate) fn clamp_frame_rect(rect: FrameRect, min_width: f32, min_height: f32) -> FrameRect {
    let min_width = min_width.clamp(0.0, 1.0);
    let min_height = min_height.clamp(0.0, 1.0);
    let width = rect.width.clamp(min_width, 1.0);
    let height = rect.height.clamp(min_height, 1.0);
    let x = rect.x.clamp(0.0, (1.0 - width).max(0.0));
    let y = rect.y.clamp(0.0, (1.0 - height).max(0.0));

    FrameRect::new(x, y, width, height)
}

pub(crate) fn drag_frame_rect(
    start: FrameRect,
    delta_x: f32,
    delta_y: f32,
    min_width: f32,
    min_height: f32,
) -> FrameRect {
    clamp_frame_rect(
        FrameRect::new(
            start.x + delta_x,
            start.y + delta_y,
            start.width,
            start.height,
        ),
        min_width,
        min_height,
    )
}

pub(crate) fn resize_frame_rect(
    start: FrameRect,
    handle: FrameHandle,
    delta_x: f32,
    delta_y: f32,
    min_width: f32,
    min_height: f32,
) -> FrameRect {
    if matches!(handle, FrameHandle::Move) {
        return drag_frame_rect(start, delta_x, delta_y, min_width, min_height);
    }

    let mut left = start.x;
    let mut top = start.y;
    let mut right = start.right();
    let mut bottom = start.bottom();

    match handle {
        FrameHandle::NorthWest => {
            left = (left + delta_x).clamp(0.0, right - min_width);
            top = (top + delta_y).clamp(0.0, bottom - min_height);
        }
        FrameHandle::NorthEast => {
            right = (right + delta_x).clamp(left + min_width, 1.0);
            top = (top + delta_y).clamp(0.0, bottom - min_height);
        }
        FrameHandle::SouthWest => {
            left = (left + delta_x).clamp(0.0, right - min_width);
            bottom = (bottom + delta_y).clamp(top + min_height, 1.0);
        }
        FrameHandle::SouthEast => {
            right = (right + delta_x).clamp(left + min_width, 1.0);
            bottom = (bottom + delta_y).clamp(top + min_height, 1.0);
        }
        FrameHandle::Move => {}
    }

    FrameRect::new(left, top, right - left, bottom - top)
}
