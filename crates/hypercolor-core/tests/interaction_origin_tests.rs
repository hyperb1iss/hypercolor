use hypercolor_core::input::{
    BrowserInputSource, InputManager, InteractionSourceOrigin, ManagedSourceRole,
};

fn register_test_source(manager: &mut InputManager, source: ManagedSourceRole) {
    manager
        .add_source(source)
        .expect("interaction fixture source should match its declared role");
}

#[test]
fn manager_graph_marks_browser_aggregate_as_non_host_origin() {
    let mut manager = InputManager::new();
    register_test_source(
        &mut manager,
        ManagedSourceRole::interaction(Box::new(BrowserInputSource::new())),
    );

    let graph = manager.input_graph_handle().snapshot();
    assert_eq!(graph.slots().len(), 1);
    assert_eq!(
        graph.slots()[0].interaction_origin(),
        Some(InteractionSourceOrigin::BrowserCompatibilityAggregate)
    );
}
