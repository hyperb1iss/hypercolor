use std::sync::Arc;

use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::DomainError;
use hypercolor_types::api::layouts::{CreateLayoutRequest, UpdateLayoutRequest};

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("layout domain tempdir should create");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("layout domain data directory should create");
    (Arc::new(AppState::new_with_data_dir(data_dir)), tempdir)
}

#[tokio::test]
async fn catalog_create_and_update_share_one_durable_transaction() {
    let (state, tempdir) = isolated_state();

    let created = state
        .domains
        .layout
        .create(CreateLayoutRequest {
            name: "  Studio  ".to_owned(),
            canvas_width: Some(640),
            canvas_height: Some(360),
            ..CreateLayoutRequest::default()
        })
        .await
        .expect("layout should create");
    assert_eq!(created.name, "Studio");

    let updated = state
        .domains
        .layout
        .update(
            "studio".to_owned(),
            UpdateLayoutRequest {
                name: Some("Editing Suite".to_owned()),
                canvas_width: Some(800),
                ..UpdateLayoutRequest::default()
            },
        )
        .await
        .expect("layout should update by name");
    assert_eq!(updated.id, created.id);
    assert_eq!(updated.name, "Editing Suite");
    assert_eq!(updated.canvas_width, 800);

    let persisted =
        hypercolor_daemon::layout_store::load(&tempdir.path().join("data/layouts.json"))
            .expect("layout catalog should load");
    assert_eq!(persisted[&created.id].name, "Editing Suite");
    assert_eq!(persisted[&created.id].canvas_width, 800);
}

#[tokio::test]
async fn catalog_identity_resolution_rejects_ambiguous_names() {
    let (state, _tempdir) = isolated_state();
    let first = state
        .domains
        .layout
        .create(CreateLayoutRequest {
            name: "Desk".to_owned(),
            ..CreateLayoutRequest::default()
        })
        .await
        .expect("first layout should create");
    let second = state
        .domains
        .layout
        .create(CreateLayoutRequest {
            name: "Other".to_owned(),
            ..CreateLayoutRequest::default()
        })
        .await
        .expect("second layout should create");
    state
        .domains
        .layout
        .update(
            second.id,
            UpdateLayoutRequest {
                name: Some("desk".to_owned()),
                ..UpdateLayoutRequest::default()
            },
        )
        .await
        .expect("second layout should update");

    assert_eq!(
        state
            .domains
            .layout
            .resolve(&first.id)
            .await
            .expect("canonical id should win")
            .id,
        first.id
    );
    assert!(matches!(
        state.domains.layout.resolve("DESK").await,
        Err(DomainError::Conflict { .. })
    ));
}

#[tokio::test]
async fn catalog_list_marks_and_filters_the_active_layout() {
    let (state, _tempdir) = isolated_state();
    let active = state.domains.layout.current();
    state
        .domains
        .layout
        .create(CreateLayoutRequest {
            name: "Inactive".to_owned(),
            ..CreateLayoutRequest::default()
        })
        .await
        .expect("inactive layout should create");

    let result = state.domains.layout.list(50, 0, true).await;
    assert_eq!(result.total, 1);
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].id, active.id);
    assert!(result.items[0].is_active);
}

// ─── Change Stream ──────────────────────────────────────────────────────────

fn drain_layout_changes(
    events: &mut tokio::sync::broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
) -> Vec<(Option<String>, String)> {
    let mut changes = Vec::new();
    while let Ok(timestamped) = events.try_recv() {
        if let hypercolor_types::event::HypercolorEvent::LayoutChanged { previous, current } =
            timestamped.event
        {
            changes.push((previous, current));
        }
    }
    changes
}

#[tokio::test]
async fn catalog_create_publishes_layout_changed_with_the_new_id() {
    let (state, _tempdir) = isolated_state();
    let mut events = state.event_bus.subscribe_all();

    let created = state
        .domains
        .layout
        .create(CreateLayoutRequest {
            name: "Studio".to_owned(),
            ..CreateLayoutRequest::default()
        })
        .await
        .expect("layout should create");

    assert_eq!(
        drain_layout_changes(&mut events),
        vec![(None, created.id)],
        "a created layout is not active, so nothing was active before it"
    );
}

#[tokio::test]
async fn catalog_update_publishes_layout_changed_with_the_updated_id() {
    let (state, _tempdir) = isolated_state();
    let created = state
        .domains
        .layout
        .create(CreateLayoutRequest {
            name: "Studio".to_owned(),
            ..CreateLayoutRequest::default()
        })
        .await
        .expect("layout should create");
    let mut events = state.event_bus.subscribe_all();

    state
        .domains
        .layout
        .update(
            "studio".to_owned(),
            UpdateLayoutRequest {
                name: Some("Editing Suite".to_owned()),
                ..UpdateLayoutRequest::default()
            },
        )
        .await
        .expect("layout should update by name");

    assert_eq!(
        drain_layout_changes(&mut events),
        vec![(None, created.id)],
        "an update names the stored id even when selected by name"
    );
}

#[tokio::test]
async fn catalog_rejected_update_publishes_nothing() {
    let (state, _tempdir) = isolated_state();
    let mut events = state.event_bus.subscribe_all();

    let rejected = state
        .domains
        .layout
        .update(
            "missing".to_owned(),
            UpdateLayoutRequest {
                name: Some("Nowhere".to_owned()),
                ..UpdateLayoutRequest::default()
            },
        )
        .await;

    assert!(matches!(rejected, Err(DomainError::NotFound { .. })));
    assert!(drain_layout_changes(&mut events).is_empty());
}
