use hypercolor_ui::components::silk_select::next_highlight;

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
