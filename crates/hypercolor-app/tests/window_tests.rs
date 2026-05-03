use hypercolor_app::window::{
    WINDOW_VISIBILITY_EVENT, WINDOW_VISIBILITY_GLOBAL, visibility_state_script,
};

#[test]
fn visibility_state_script_sets_global_and_dispatches_event() {
    let script = visibility_state_script(false);

    assert!(script.contains(WINDOW_VISIBILITY_GLOBAL));
    assert!(script.contains(WINDOW_VISIBILITY_EVENT));
    assert!(script.contains("const visible = false;"));
    assert!(script.contains("CustomEvent"));
}
