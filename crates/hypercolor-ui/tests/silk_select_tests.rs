use hypercolor_ui::components::silk_select::{fit_panel_width, next_highlight};

#[test]
fn arrow_down_advances_and_wraps() {
    assert_eq!(next_highlight(None, 3, "ArrowDown"), Some(0));
    assert_eq!(next_highlight(Some(0), 3, "ArrowDown"), Some(1));
    assert_eq!(next_highlight(Some(2), 3, "ArrowDown"), Some(0));
}

#[test]
fn arrow_up_retreats_and_wraps() {
    assert_eq!(next_highlight(None, 3, "ArrowUp"), Some(2));
    assert_eq!(next_highlight(Some(2), 3, "ArrowUp"), Some(1));
    assert_eq!(next_highlight(Some(0), 3, "ArrowUp"), Some(2));
}

#[test]
fn home_and_end_jump_to_edges() {
    assert_eq!(next_highlight(Some(1), 5, "Home"), Some(0));
    assert_eq!(next_highlight(Some(1), 5, "End"), Some(4));
    assert_eq!(next_highlight(None, 5, "Home"), Some(0));
    assert_eq!(next_highlight(None, 5, "End"), Some(4));
}

#[test]
fn empty_lists_never_highlight() {
    for key in ["ArrowDown", "ArrowUp", "Home", "End"] {
        assert_eq!(next_highlight(None, 0, key), None);
        assert_eq!(next_highlight(Some(3), 0, key), None);
    }
}

#[test]
fn other_keys_keep_the_current_highlight() {
    assert_eq!(next_highlight(Some(2), 5, "a"), Some(2));
    assert_eq!(next_highlight(None, 5, "Enter"), None);
}

#[test]
fn single_option_stays_pinned() {
    assert_eq!(next_highlight(Some(0), 1, "ArrowDown"), Some(0));
    assert_eq!(next_highlight(Some(0), 1, "ArrowUp"), Some(0));
}

#[test]
fn a_fitted_panel_grows_past_its_trigger_to_the_viewport_edge() {
    let style = "top: 40px; left: 300px; width: 128px; max-height: 320px";
    assert_eq!(
        fit_panel_width(style, 128.0, 600.0),
        "top: 40px; left: 300px; min-width: 128px; width: max-content; max-width: 600px; max-height: 320px"
    );
    // A trigger with no room past it never shrinks below its own width.
    assert!(fit_panel_width(style, 128.0, 40.0).contains("max-width: 128px"));
}
