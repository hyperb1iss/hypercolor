use std::sync::Arc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use hypercolor_tui::action::Action;
use hypercolor_tui::component::Component;
use hypercolor_tui::state::EffectSummary;
use hypercolor_tui::views::DashboardView;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn sample_effect() -> EffectSummary {
    EffectSummary {
        id: "rainbow".to_string(),
        name: "Rainbow Wave".to_string(),
        description: String::new(),
        author: String::new(),
        category: String::new(),
        source: "native".to_string(),
        audio_reactive: false,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
    }
}

#[test]
fn clicking_dashboard_favorite_applies_effect() {
    let mut view = DashboardView::new();
    view.update(&Action::EffectsUpdated(Arc::new(vec![sample_effect()])))
        .expect("effects should update");
    view.update(&Action::FavoritesUpdated(Arc::new(vec![
        "rainbow".to_string(),
    ])))
    .expect("favorites should update");

    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).expect("terminal should initialize");
    terminal
        .draw(|frame| view.render(frame, Rect::new(0, 0, 100, 24)))
        .expect("view should render");

    let action = view
        .handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), 3, 22))
        .expect("mouse event should be handled");

    match action {
        Some(Action::ApplyEffect(id)) => assert_eq!(id, "rainbow"),
        other => panic!("expected favorite apply action, got {other:?}"),
    }
}
