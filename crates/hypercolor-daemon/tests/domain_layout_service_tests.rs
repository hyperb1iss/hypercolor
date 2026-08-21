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
    let (state, _tempdir) = isolated_state();

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
        hypercolor_daemon::layout_store::load(state.domains.layout.catalog_path_for_test())
            .expect("layout catalog should load");
    assert_eq!(persisted[&created.id].name, "Editing Suite");
    assert_eq!(persisted[&created.id].canvas_width, 800);
}

#[tokio::test]
async fn catalog_identity_resolution_rejects_ambiguous_names() {
    let (state, _tempdir) = isolated_state();
    let current = state.domains.layout.current();
    let mut first = current.clone();
    first.id = "layout_first".to_owned();
    first.name = "Desk".to_owned();
    let mut second = current;
    second.id = "layout_second".to_owned();
    second.name = "desk".to_owned();
    {
        let mut catalog = state.domains.layout.catalog_for_test().write().await;
        catalog.insert(first.id.clone(), first.clone());
        catalog.insert(second.id.clone(), second);
    }

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
    let mut inactive = active.clone();
    inactive.id = "layout_inactive".to_owned();
    inactive.name = "Inactive".to_owned();
    {
        let mut catalog = state.domains.layout.catalog_for_test().write().await;
        catalog.insert(active.id.clone(), active.clone());
        catalog.insert(inactive.id.clone(), inactive);
    }

    let result = state.domains.layout.list(50, 0, true).await;
    assert_eq!(result.total, 1);
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].id, active.id);
    assert!(result.items[0].is_active);
}
