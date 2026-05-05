//! Resizable split — a draggable divider between two terminal panels.
//!
//! Each `Split` tracks a ratio (0.0–1.0) that determines how the parent
//! area is divided. Mouse drag on the boundary adjusts the ratio in
//! real-time with clamping to configurable min sizes.

use std::cell::Cell;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};

/// Direction of a resizable split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    /// Side-by-side: first │ second.
    Horizontal,
    /// Stacked: first ─── second.
    Vertical,
}

/// Grab-zone radius (in cells) around the boundary for mouse targeting.
const GRAB_RADIUS: u16 = 1;

/// A resizable split between two panels with mouse-drag support.
///
/// Usage:
/// - Call `layout(area)` inside your `render` to get the two panel rects.
/// - Call `handle_mouse(event)` inside your `handle_mouse_event` — if it
///   returns `true`, the event was consumed by the divider.
/// - Call `render_divider(frame)` **after** rendering both panels to overlay
///   the hover/drag highlight.
pub struct Split {
    direction: SplitDirection,
    /// Fraction of space allocated to the first panel (0.0..=1.0).
    ratio: f32,
    /// Default ratio for double-click reset.
    default_ratio: f32,
    /// Minimum cells for the first panel.
    min_first: u16,
    /// Minimum cells for the second panel.
    min_second: u16,
    /// Active drag in progress.
    dragging: bool,
    /// Mouse hovering over the divider.
    hover: bool,
    /// Cached parent area from last `layout()` call.
    parent_area: Cell<Rect>,
}

impl Split {
    /// Create a new resizable split with the given direction and default ratio.
    #[must_use]
    pub fn new(direction: SplitDirection, default_ratio: f32) -> Self {
        Self {
            direction,
            ratio: default_ratio,
            default_ratio,
            min_first: 4,
            min_second: 4,
            dragging: false,
            hover: false,
            parent_area: Cell::new(Rect::default()),
        }
    }

    /// Set minimum sizes for both panels.
    #[must_use]
    pub fn min_sizes(mut self, first: u16, second: u16) -> Self {
        self.min_first = first;
        self.min_second = second;
        self
    }

    /// Compute the layout rects for the two panels.
    ///
    /// Call this in your `render` method. Returns `[first_area, second_area]`.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        clippy::cast_sign_loss
    )]
    pub fn layout(&self, area: Rect) -> [Rect; 2] {
        self.parent_area.set(area);

        let (total, min_a, min_b) = match self.direction {
            SplitDirection::Horizontal => (area.width, self.min_first, self.min_second),
            SplitDirection::Vertical => (area.height, self.min_first, self.min_second),
        };

        // When the area is too small for both minimums, just split proportionally
        let max_first = total.saturating_sub(min_b);
        let effective_min = min_a.min(max_first);
        let first_size = (f32::from(total) * self.ratio).round() as u16;
        let first_size = first_size.clamp(effective_min, max_first.max(effective_min));
        let second_size = total.saturating_sub(first_size);

        match self.direction {
            SplitDirection::Horizontal => [
                Rect::new(area.x, area.y, first_size, area.height),
                Rect::new(area.x + first_size, area.y, second_size, area.height),
            ],
            SplitDirection::Vertical => [
                Rect::new(area.x, area.y, area.width, first_size),
                Rect::new(area.x, area.y + first_size, area.width, second_size),
            ],
        }
    }

    /// The cell position of the boundary between the two panels.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        clippy::cast_sign_loss
    )]
    fn boundary_pos(&self) -> u16 {
        let area = self.parent_area.get();
        let (total, origin) = match self.direction {
            SplitDirection::Horizontal => (area.width, area.x),
            SplitDirection::Vertical => (area.height, area.y),
        };
        let max_first = total.saturating_sub(self.min_second);
        let effective_min = self.min_first.min(max_first);
        let first_size = (f32::from(total) * self.ratio).round() as u16;
        let first_size = first_size.clamp(effective_min, max_first.max(effective_min));
        origin + first_size
    }

    /// Check if a mouse position is within the grab zone of the boundary.
    fn in_grab_zone(&self, col: u16, row: u16) -> bool {
        let area = self.parent_area.get();
        if area.width == 0 || area.height == 0 {
            return false;
        }

        match self.direction {
            SplitDirection::Horizontal => {
                if row < area.y || row >= area.y + area.height {
                    return false;
                }
                let boundary = self.boundary_pos();
                col >= boundary.saturating_sub(GRAB_RADIUS)
                    && col <= boundary.min(area.x + area.width.saturating_sub(1))
            }
            SplitDirection::Vertical => {
                if col < area.x || col >= area.x + area.width {
                    return false;
                }
                let boundary = self.boundary_pos();
                row >= boundary.saturating_sub(GRAB_RADIUS)
                    && row <= boundary.min(area.y + area.height.saturating_sub(1))
            }
        }
    }

    /// Handle a mouse event. Returns `true` if the event was consumed by the
    /// split divider (hover, drag start, drag move, or drag end).
    pub fn handle_mouse(&mut self, mouse: &MouseEvent) -> bool {
        let col = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::Moved => {
                let was_hover = self.hover;
                self.hover = self.in_grab_zone(col, row);
                // Don't consume move events — other handlers may need them
                self.hover != was_hover && self.hover
            }
            MouseEventKind::Down(MouseButton::Left) if self.in_grab_zone(col, row) => {
                self.dragging = true;
                true
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.update_ratio(col, row);
                true
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                self.dragging = false;
                self.hover = self.in_grab_zone(col, row);
                true
            }
            // Double-click resets to default ratio
            MouseEventKind::Down(MouseButton::Middle) if self.in_grab_zone(col, row) => {
                self.ratio = self.default_ratio;
                true
            }
            _ => false,
        }
    }

    /// Update the ratio from a mouse position during drag.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn update_ratio(&mut self, col: u16, row: u16) {
        let area = self.parent_area.get();
        let (pos, origin, total) = match self.direction {
            SplitDirection::Horizontal => (col, area.x, area.width),
            SplitDirection::Vertical => (row, area.y, area.height),
        };
        if total == 0 {
            return;
        }

        let new_ratio = f32::from(pos.saturating_sub(origin)) / f32::from(total);
        let min_ratio = f32::from(self.min_first) / f32::from(total);
        let max_ratio = f32::from(total.saturating_sub(self.min_second)) / f32::from(total);
        self.ratio = new_ratio.clamp(min_ratio, max_ratio);
    }

    /// Whether a drag is in progress.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Render a visual highlight on the boundary when hovering or dragging.
    ///
    /// Call this **after** rendering both panels so the overlay appears on top
    /// of the existing border characters.
    pub fn render_divider(&self, frame: &mut Frame) {
        if !self.hover && !self.dragging {
            return;
        }

        let area = self.parent_area.get();
        let color = if self.dragging {
            Color::Rgb(225, 53, 255) // electric purple
        } else {
            Color::Rgb(128, 255, 234) // neon cyan
        };

        let style = if self.dragging {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };

        let boundary = self.boundary_pos();
        let buf = frame.buffer_mut();

        match self.direction {
            SplitDirection::Horizontal => {
                for y in area.y..area.y + area.height {
                    if boundary > area.x
                        && let Some(cell) = buf.cell_mut(Position::new(boundary - 1, y))
                    {
                        cell.set_style(cell.style().patch(style));
                    }
                    if boundary < area.x + area.width
                        && let Some(cell) = buf.cell_mut(Position::new(boundary, y))
                    {
                        cell.set_style(cell.style().patch(style));
                    }
                }
            }
            SplitDirection::Vertical => {
                for x in area.x..area.x + area.width {
                    if boundary > area.y
                        && let Some(cell) = buf.cell_mut(Position::new(x, boundary - 1))
                    {
                        cell.set_style(cell.style().patch(style));
                    }
                    if boundary < area.y + area.height
                        && let Some(cell) = buf.cell_mut(Position::new(x, boundary))
                    {
                        cell.set_style(cell.style().patch(style));
                    }
                }
            }
        }
    }
}
