use hypercolor_app::window::{
    SETTINGS_ROUTE, WINDOW_VISIBILITY_EVENT, WINDOW_VISIBILITY_GLOBAL, route_navigation_script,
    visibility_state_script,
};

#[test]
fn visibility_state_script_sets_global_and_dispatches_event() {
    let script = visibility_state_script(false);

    assert!(script.contains(WINDOW_VISIBILITY_GLOBAL));
    assert!(script.contains(WINDOW_VISIBILITY_EVENT));
    assert!(script.contains("const visible = false;"));
    assert!(script.contains("CustomEvent"));
}

#[test]
fn route_navigation_script_pushes_spa_route_and_notifies_router() {
    let script = route_navigation_script(SETTINGS_ROUTE);

    assert!(script.contains("\"/settings\""));
    assert!(script.contains("window.location.pathname"));
    assert!(script.contains("window.history.pushState"));
    assert!(script.contains("PopStateEvent"));
}
