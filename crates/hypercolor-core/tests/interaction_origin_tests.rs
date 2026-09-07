use hypercolor_core::input::{
    BrowserConnectionIncarnation, BrowserInputChildKey, BrowserInputHandle, BrowserPreviewId,
    InputManager, SourceState,
};

#[test]
fn browser_children_remain_outside_the_host_manager_graph() {
    let manager = InputManager::new();
    let browser = BrowserInputHandle::new();
    let attachment = browser
        .attach(BrowserInputChildKey::new(
            BrowserConnectionIncarnation::new(1),
            BrowserPreviewId::new("origin-boundary"),
        ))
        .expect("browser child should attach");

    assert!(manager.input_graph_handle().snapshot().slots().is_empty());
    assert!(
        manager
            .source_status_registry()
            .snapshot()
            .statuses()
            .is_empty()
    );

    let registry = browser.registry().snapshot();
    assert_eq!(registry.children().len(), 1);
    assert_eq!(
        registry.children()[0].publication_id(),
        attachment.publication_id()
    );
    assert_eq!(
        attachment.slot().status().snapshot().state,
        SourceState::Live
    );
}
