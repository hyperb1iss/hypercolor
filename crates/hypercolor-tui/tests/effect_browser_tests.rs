use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hypercolor_tui::action::Action;
use hypercolor_tui::component::Component;
use hypercolor_tui::state::{EffectSummary, PreviewSource, SimulatedDisplaySummary};
use hypercolor_tui::views::EffectBrowserView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
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
fn preview_pane_cycles_between_canvas_and_simulator_sources() {
    let mut view = EffectBrowserView::new();
    view.update(&Action::EffectsUpdated(Arc::new(vec![sample_effect()])))
        .expect("effects should update");
    view.update(&Action::SimulatedDisplaysUpdated(Arc::new(vec![
        SimulatedDisplaySummary {
            id: "sim-1".to_string(),
            name: "Desk Preview".to_string(),
            width: 480,
            height: 480,
            circular: true,
            enabled: true,
        },
    ])))
    .expect("simulators should update");

    view.handle_key_event(key(KeyCode::Tab))
        .expect("tab should move focus to preview");

    let next = view
        .handle_key_event(key(KeyCode::Right))
        .expect("right should be handled");
    match next {
        Some(Action::SetPreviewSource(PreviewSource::Simulator(id))) => {
            assert_eq!(id, "sim-1");
        }
        other => panic!("expected simulator preview selection, got {other:?}"),
    }

    view.update(&Action::SetPreviewSource(PreviewSource::Simulator(
        "sim-1".to_string(),
    )))
    .expect("preview source should update");

    let previous = view
        .handle_key_event(key(KeyCode::Left))
        .expect("left should be handled");
    match previous {
        Some(Action::SetPreviewSource(PreviewSource::Canvas)) => {}
        other => panic!("expected canvas preview selection, got {other:?}"),
    }
}
