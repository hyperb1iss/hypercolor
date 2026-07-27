use hypercolor_core::input::{BrowserInputSource, InputManager, InteractionSourceOrigin};

#[test]
fn manager_graph_marks_browser_aggregate_as_non_host_origin() {
    let mut manager = InputManager::new();
    manager.add_source(Box::new(BrowserInputSource::new()));

    let graph = manager.input_graph_handle().snapshot();
    assert_eq!(graph.slots().len(), 1);
    assert_eq!(
        graph.slots()[0].interaction_origin(),
        Some(InteractionSourceOrigin::BrowserCompatibilityAggregate)
    );
}
