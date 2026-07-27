use hypercolor_core::input::InputSource;
use hypercolor_core::input::browser::{
    BrowserConnectionIncarnation, BrowserInputChildKey, BrowserInputSource, BrowserPreviewId,
};
use hypercolor_core::input::routing::SourceIncarnation;
use hypercolor_types::config::InteractionRoutePolicy;

use super::{AuthoritativeClaimError, AuthoritativeClaimOutcome, InteractionRoutingControl};

fn active_browser_source() -> BrowserInputSource {
    let mut source = BrowserInputSource::new();
    source.start().expect("browser source should start");
    source
}

fn key(connection: u64, preview: &str) -> BrowserInputChildKey {
    BrowserInputChildKey::new(
        BrowserConnectionIncarnation::new(connection),
        BrowserPreviewId::new(preview),
    )
}

#[test]
fn authoritative_claim_is_single_owner_idempotent_and_hands_off_cleanly() {
    let source = active_browser_source();
    let handle = source.handle();
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
    let source = active_browser_source();
    let handle = source.handle();
    let preview = handle.attach(key(9, "cabinet")).expect("preview attaches");
    let control = InteractionRoutingControl::new(
        handle.registry(),
        1,
        InteractionRoutePolicy::Merge,
        InteractionRoutePolicy::Browser,
    );

    let preview_request = control.preview_request(&preview);
    assert_eq!(preview_request.policy, InteractionRoutePolicy::Browser);
    assert_eq!(
        preview_request.browser_source,
        Some(SourceIncarnation::browser_child(
            preview.publication_id().get()
        ))
    );
    assert_eq!(control.daemon_request().browser_source, None);

    control
        .claim_authoritative(&preview)
        .expect("claim should succeed");
    assert_eq!(
        control.daemon_request().browser_source,
        preview_request.browser_source
    );
}

#[test]
fn policy_publication_is_coherent_and_avoids_noop_generation_churn() {
    let source = active_browser_source();
    let control = InteractionRoutingControl::new(
        source.handle().registry(),
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
