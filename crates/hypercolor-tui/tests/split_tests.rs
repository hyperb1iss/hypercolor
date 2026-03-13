//! Tests for the resizable split widget.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use hypercolor_tui::widgets::{Split, SplitDirection};

fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }
}

// ── Layout tests ────────────────────────────────────────────────────

#[test]
fn horizontal_split_respects_ratio() {
    let split = Split::new(SplitDirection::Horizontal, 0.5).min_sizes(2, 2);
    let area = Rect::new(0, 0, 80, 24);
    let [first, second] = split.layout(area);

    assert_eq!(first.width + second.width, 80);
    assert_eq!(first.width, 40);
    assert_eq!(second.width, 40);
    assert_eq!(first.x, 0);
    assert_eq!(second.x, 40);
}

#[test]
fn vertical_split_respects_ratio() {
    let split = Split::new(SplitDirection::Vertical, 0.5).min_sizes(2, 2);
    let area = Rect::new(0, 0, 80, 24);
    let [first, second] = split.layout(area);

    assert_eq!(first.height + second.height, 24);
    assert_eq!(first.height, 12);
    assert_eq!(second.height, 12);
}

#[test]
fn split_clamps_to_min_first() {
    let split = Split::new(SplitDirection::Horizontal, 0.01).min_sizes(10, 10);
    let area = Rect::new(0, 0, 80, 24);
    let [first, _] = split.layout(area);

    assert!(
        first.width >= 10,
        "first panel should be at least min_first"
    );
}

#[test]
fn split_clamps_to_min_second() {
    let split = Split::new(SplitDirection::Horizontal, 0.99).min_sizes(10, 10);
    let area = Rect::new(0, 0, 80, 24);
    let [_, second] = split.layout(area);

    assert!(
        second.width >= 10,
        "second panel should be at least min_second"
    );
}

#[test]
fn split_preserves_total_width() {
    let split = Split::new(SplitDirection::Horizontal, 0.35).min_sizes(5, 5);
    let area = Rect::new(10, 5, 60, 20);
    let [first, second] = split.layout(area);

    assert_eq!(first.x, area.x);
    assert_eq!(first.width + second.width, area.width);
    assert_eq!(second.x, first.x + first.width);
    assert_eq!(first.height, area.height);
    assert_eq!(second.height, area.height);
}

#[test]
fn split_preserves_total_height() {
    let split = Split::new(SplitDirection::Vertical, 0.45).min_sizes(3, 3);
    let area = Rect::new(0, 2, 80, 30);
    let [first, second] = split.layout(area);

    assert_eq!(first.y, area.y);
    assert_eq!(first.height + second.height, area.height);
    assert_eq!(second.y, first.y + first.height);
    assert_eq!(first.width, area.width);
    assert_eq!(second.width, area.width);
}

// ── Mouse drag tests ────────────────────────────────────────────────

#[test]
fn click_on_boundary_starts_drag() {
    let mut split = Split::new(SplitDirection::Horizontal, 0.5).min_sizes(2, 2);
    let area = Rect::new(0, 0, 80, 24);
    let _ = split.layout(area);

    // Boundary is at column 40. Click on boundary.
    let consumed = split.handle_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 40, 12));
    assert!(consumed, "click on boundary should be consumed");
    assert!(split.is_dragging(), "should start dragging");
}

#[test]
fn click_away_from_boundary_is_ignored() {
    let mut split = Split::new(SplitDirection::Horizontal, 0.5).min_sizes(2, 2);
    let area = Rect::new(0, 0, 80, 24);
    let _ = split.layout(area);

    let consumed = split.handle_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 10, 12));
    assert!(!consumed, "click far from boundary should not be consumed");
    assert!(!split.is_dragging());
}

#[test]
fn drag_updates_ratio() {
    let mut split = Split::new(SplitDirection::Horizontal, 0.5).min_sizes(2, 2);
    let area = Rect::new(0, 0, 80, 24);
    let _ = split.layout(area);

    // Start drag at boundary
    split.handle_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 40, 12));

    // Drag to column 60 (should set ratio to ~0.75)
    split.handle_mouse(&mouse(MouseEventKind::Drag(MouseButton::Left), 60, 12));

    let [first, _] = split.layout(area);
    assert_eq!(first.width, 60);
}

#[test]
fn drag_clamps_to_min_sizes() {
    let mut split = Split::new(SplitDirection::Horizontal, 0.5).min_sizes(10, 10);
    let area = Rect::new(0, 0, 80, 24);
    let _ = split.layout(area);

    // Start drag
    split.handle_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 40, 12));

    // Drag all the way left — should clamp at min_first
    split.handle_mouse(&mouse(MouseEventKind::Drag(MouseButton::Left), 0, 12));

    let [first, _] = split.layout(area);
    assert!(first.width >= 10);

    // Drag all the way right — should clamp at (total - min_second)
    split.handle_mouse(&mouse(MouseEventKind::Drag(MouseButton::Left), 79, 12));

    let [_, second] = split.layout(area);
    assert!(second.width >= 10);
}

#[test]
fn mouse_up_ends_drag() {
    let mut split = Split::new(SplitDirection::Horizontal, 0.5).min_sizes(2, 2);
    let area = Rect::new(0, 0, 80, 24);
    let _ = split.layout(area);

    split.handle_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 40, 12));
    assert!(split.is_dragging());

    split.handle_mouse(&mouse(MouseEventKind::Up(MouseButton::Left), 50, 12));
    assert!(!split.is_dragging());
}

#[test]
fn vertical_drag_updates_ratio() {
    let mut split = Split::new(SplitDirection::Vertical, 0.5).min_sizes(2, 2);
    let area = Rect::new(0, 0, 80, 20);
    let _ = split.layout(area);

    // Boundary at row 10
    split.handle_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 40, 10));
    assert!(split.is_dragging());

    // Drag to row 15
    split.handle_mouse(&mouse(MouseEventKind::Drag(MouseButton::Left), 40, 15));

    let [first, _] = split.layout(area);
    assert_eq!(first.height, 15);
}

#[test]
fn middle_click_resets_to_default() {
    let mut split = Split::new(SplitDirection::Horizontal, 0.5).min_sizes(2, 2);
    let area = Rect::new(0, 0, 80, 24);
    let _ = split.layout(area);

    // Drag to change ratio
    split.handle_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 40, 12));
    split.handle_mouse(&mouse(MouseEventKind::Drag(MouseButton::Left), 60, 12));
    split.handle_mouse(&mouse(MouseEventKind::Up(MouseButton::Left), 60, 12));

    let [first, _] = split.layout(area);
    assert_eq!(first.width, 60);

    // Middle-click on boundary resets
    split.handle_mouse(&mouse(MouseEventKind::Down(MouseButton::Middle), 60, 12));

    let [first, _] = split.layout(area);
    assert_eq!(first.width, 40, "should reset to default 50% ratio");
}

#[test]
fn drag_without_prior_click_is_ignored() {
    let mut split = Split::new(SplitDirection::Horizontal, 0.5).min_sizes(2, 2);
    let area = Rect::new(0, 0, 80, 24);
    let _ = split.layout(area);

    let consumed = split.handle_mouse(&mouse(MouseEventKind::Drag(MouseButton::Left), 60, 12));
    assert!(!consumed, "drag without prior click should not be consumed");
}

// ── Edge cases ──────────────────────────────────────────────────────

#[test]
fn zero_area_does_not_panic() {
    let split = Split::new(SplitDirection::Horizontal, 0.5).min_sizes(2, 2);
    let area = Rect::new(0, 0, 0, 0);
    let [first, second] = split.layout(area);
    assert_eq!(first.width, 0);
    assert_eq!(second.width, 0);
}

#[test]
fn offset_area_works_correctly() {
    let split = Split::new(SplitDirection::Horizontal, 0.5).min_sizes(2, 2);
    let area = Rect::new(10, 5, 40, 10);
    let [first, second] = split.layout(area);

    assert_eq!(first.x, 10);
    assert_eq!(first.width, 20);
    assert_eq!(second.x, 30);
    assert_eq!(second.width, 20);
}
