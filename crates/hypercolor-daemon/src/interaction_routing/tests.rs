use hypercolor_core::input::browser::{
    BrowserConnectionIncarnation, BrowserInputChildKey, BrowserInputHandle, BrowserPreviewId,
};
use std::time::Instant;

use hypercolor_core::input::routing::{
    ConsumerIncarnation, InteractionRouteCatalog, InteractionRouter, RoutedInteraction,
    SourceIncarnation,
};
use hypercolor_core::input::{DeviceInputHandle, InputManager};
use hypercolor_types::config::InteractionRoutePolicy;
use hypercolor_types::device::DeviceId;

use super::{
    AuthoritativeClaimError, AuthoritativeClaimOutcome, InteractionRoutingControl,
    selected_host_statuses, selected_input_availability,
};

fn key(connection: u64, preview: &str) -> BrowserInputChildKey {
    BrowserInputChildKey::new(
        BrowserConnectionIncarnation::new(connection),
        BrowserPreviewId::new(preview),
    )
}

#[test]
fn authoritative_claim_is_single_owner_idempotent_and_hands_off_cleanly() {
    let handle = BrowserInputHandle::new();
    let first = handle
        .attach(key(1, "main"))
        .expect("first preview attaches");
    let second = handle
        .attach(key(2, "main"))
        .expect("second preview attaches");
    let control = InteractionRoutingControl::new(
        handle.registry(),
        7,
        InteractionRoutePolicy::Host,
        InteractionRoutePolicy::Browser,
    );

    assert_eq!(
        control.claim_authoritative(&first),
        Ok(AuthoritativeClaimOutcome::Granted)
    );
    assert_eq!(
        control.claim_authoritative(&first),
        Ok(AuthoritativeClaimOutcome::AlreadyOwned)
    );
    assert_eq!(
        control.claim_authoritative(&second),
        Err(AuthoritativeClaimError::Conflict)
    );

    assert!(control.close_preview(&first));
    assert_eq!(
        control.claim_authoritative(&second),
        Ok(AuthoritativeClaimOutcome::Granted)
    );
    assert!(!control.close_preview(&first));
}

#[test]
fn route_requests_use_exact_preview_and_authoritative_publications() {
    let handle = BrowserInputHandle::new();
    let preview = handle.attach(key(9, "cabinet")).expect("preview attaches");
    let control = InteractionRoutingControl::new(
        handle.registry(),
        1,
        InteractionRoutePolicy::Merge,
        InteractionRoutePolicy::Browser,
    );

    let initial = control.snapshot();
    let preview_request = initial.preview_request(preview.publication_id());
    assert_eq!(preview_request.policy, InteractionRoutePolicy::Browser);
    assert_eq!(
        preview_request.browser_source,
        Some(SourceIncarnation::browser_child(
            preview.publication_id().get()
        ))
    );
    assert_eq!(initial.daemon_request().browser_source, None);
    assert_eq!(initial.config_generation, 1);

    control
        .claim_authoritative(&preview)
        .expect("claim should succeed");
    let claimed = control.snapshot();
    assert_eq!(
        claimed.daemon_request().browser_source,
        preview_request.browser_source
    );
    assert_eq!(claimed.config_generation, 1);
}

#[test]
fn policy_publication_is_coherent_and_avoids_noop_generation_churn() {
    let browser = BrowserInputHandle::new();
    let control = InteractionRoutingControl::new(
        browser.registry(),
        3,
        InteractionRoutePolicy::Host,
        InteractionRoutePolicy::Browser,
    );
    let initial = control.snapshot();

    let unchanged = control.publish_policies(
        3,
        InteractionRoutePolicy::Host,
        InteractionRoutePolicy::Browser,
    );
    assert_eq!(unchanged.generation, initial.generation);

    let changed = control.publish_policies(
        4,
        InteractionRoutePolicy::Merge,
        InteractionRoutePolicy::Host,
    );
    assert_eq!(changed.generation, initial.generation + 1);
    assert_eq!(changed.config_generation, 4);
    assert_eq!(changed.daemon_policy, InteractionRoutePolicy::Merge);
    assert_eq!(changed.preview_policy, InteractionRoutePolicy::Host);
}

#[test]
fn routed_devices_do_not_report_host_input_availability() {
    let devices = DeviceInputHandle::new();
    devices.set_demanded(true);
    let _pad = devices.attach(DeviceId::new(), "pad");
    let browser = BrowserInputHandle::new();
    let control = InteractionRoutingControl::new(
        browser.registry(),
        1,
        InteractionRoutePolicy::Host,
        InteractionRoutePolicy::Browser,
    )
    .with_device_input(devices);

    let mut catalog = InteractionRouteCatalog::default();
    catalog.refresh(
        &InputManager::new().input_graph_handle().snapshot(),
        &control.browser_registry_snapshot(),
        &control.device_registry_snapshot(),
        Instant::now(),
    );
    let consumer = ConsumerIncarnation::new(1);
    let mut routed = RoutedInteraction::new(consumer);
    catalog.resolve_into(
        &mut InteractionRouter::default(),
        consumer,
        control.snapshot().daemon_request(),
        1,
        0,
        &mut routed,
    );

    let selected = &routed.diagnostics.selected;
    assert_eq!(selected.len(), 1, "the device routes under the host policy");
    assert!(
        selected_input_availability(
            selected.iter().filter_map(|source| source.status.as_ref()),
            Instant::now()
        )
        .routed,
        "a live device status would otherwise count as routed input"
    );
    assert!(
        !selected_input_availability(selected_host_statuses(selected), Instant::now()).routed,
        "host input availability ignores device sources"
    );
}
