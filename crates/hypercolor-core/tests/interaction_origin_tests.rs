use std::sync::Arc;

use hypercolor_core::input::routing::{
    ConsumerIncarnation, InteractionRouteContext, InteractionRouteRequest, InteractionRouteSource,
    InteractionRouteSourceClass, InteractionRouter, RoutedInteraction, SourceIncarnation,
};
use hypercolor_core::input::{
    BrowserConnectionIncarnation, BrowserInputChildKey, BrowserInputHandle, BrowserPreviewId,
    InputManager, SourceState,
};
use hypercolor_types::config::InteractionRoutePolicy;

#[test]
fn browser_children_stay_out_of_the_manager_sample_graph() {
    let manager = InputManager::new();
    let browser = BrowserInputHandle::new();
    let attachment = browser
        .attach(BrowserInputChildKey::new(
            BrowserConnectionIncarnation::new(7),
            BrowserPreviewId::new("exact-child"),
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

    let incarnation = SourceIncarnation::browser_child(attachment.publication_id().get());
    let sources = [InteractionRouteSource::new(
        incarnation,
        Arc::<str>::from(attachment.slot().source_id()),
        InteractionRouteSourceClass::Browser,
        1,
        Arc::new(attachment.slot()),
    )];
    let consumer = ConsumerIncarnation::new(1);
    let mut router = InteractionRouter::default();
    let mut output = RoutedInteraction::new(consumer);
    router.resolve_into(
        consumer,
        InteractionRouteRequest {
            policy: InteractionRoutePolicy::Browser,
            browser_source: Some(incarnation),
        },
        &sources,
        InteractionRouteContext {
            browser_registry_generation: registry.generation(),
            ..InteractionRouteContext::default()
        },
        &mut output,
    );
    let status = output.diagnostics.selected[0]
        .status
        .as_ref()
        .expect("exact browser route should expose child health")
        .snapshot();
    assert_eq!(status.state, SourceState::Live);
}
