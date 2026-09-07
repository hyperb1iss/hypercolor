//! `GET /api/v1/effects` narrows the catalog server-side.
//!
//! Every transport that lists effects now hands one
//! `EffectCatalogQuery` to the effect service, so these tests pin the
//! wire spelling of the filters, the expansion contract, and the
//! refusal a caller gets for a value the type system does not have.

use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
    EffectSource, EffectState, PresetTemplate,
};
use hypercolor_types::library::PresetId;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

// ── Harness ──────────────────────────────────────────────────────────────

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppState::new_with_data_dir(data_dir)), tempdir)
}

async fn list(state: &Arc<AppState>, query: &str) -> (StatusCode, Value) {
    let response = api::build_router(Arc::clone(state), None)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/effects{query}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should be served");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should read");
    (
        status,
        serde_json::from_slice(&bytes).expect("response body should be JSON"),
    )
}

async fn names(state: &Arc<AppState>, query: &str) -> Vec<String> {
    let (status, body) = list(state, query).await;
    assert_eq!(status, StatusCode::OK, "listing {query} should succeed");
    body["data"]["items"]
        .as_array()
        .expect("items should be an array")
        .iter()
        .map(|item| {
            item["name"]
                .as_str()
                .expect("name should be a string")
                .to_owned()
        })
        .collect()
}

struct EffectShape {
    name: &'static str,
    category: EffectCategory,
    audio_reactive: bool,
    screen_reactive: bool,
    input_reactive: bool,
    html: bool,
    description: &'static str,
}

async fn register(state: &Arc<AppState>, shape: &EffectShape) {
    let path = std::path::PathBuf::from(format!("builtin/{}", shape.name));
    let metadata = EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: shape.name.to_owned(),
        author: "catalog-test".to_owned(),
        version: "0.1.0".to_owned(),
        description: shape.description.to_owned(),
        category: shape.category,
        tags: vec!["pinned".to_owned()],
        controls: vec![ControlDefinition {
            id: "speed".to_owned(),
            name: "Speed".to_owned(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(5.0),
            min: Some(0.0),
            max: Some(100.0),
            step: Some(0.5),
            labels: Vec::new(),
            group: Some("General".to_owned()),
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }],
        presets: vec![PresetTemplate {
            id: PresetId::stable("calm"),
            name: "Calm".to_owned(),
            description: Some("slow and quiet".to_owned()),
            controls: std::collections::HashMap::new(),
        }],
        audio_reactive: shape.audio_reactive,
        screen_reactive: shape.screen_reactive,
        input_reactive: shape.input_reactive,
        source: if shape.html {
            EffectSource::Html { path }
        } else {
            EffectSource::Native { path }
        },
        license: None,
    };

    let _ = state
        .domains
        .effects
        .register(EffectEntry {
            metadata,
            source_path: format!("/tmp/{}.html", shape.name).into(),
            modified: std::time::SystemTime::now(),
            state: EffectState::Loading,
        })
        .await;
}

/// Four effects that differ on every filterable axis.
async fn seeded_catalog() -> (Arc<AppState>, tempfile::TempDir) {
    let (state, tempdir) = isolated_state();
    for shape in [
        EffectShape {
            name: "aurora",
            category: EffectCategory::Ambient,
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
            html: false,
            description: "slow northern lights",
        },
        EffectShape {
            name: "spectrum",
            category: EffectCategory::Audio,
            audio_reactive: true,
            screen_reactive: false,
            input_reactive: false,
            html: true,
            description: "bars that follow the music",
        },
        EffectShape {
            name: "ambilight",
            category: EffectCategory::Source,
            audio_reactive: false,
            screen_reactive: true,
            input_reactive: false,
            html: true,
            description: "samples the desktop",
        },
        EffectShape {
            name: "ripple",
            category: EffectCategory::Interactive,
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: true,
            html: false,
            description: "keystrokes push waves outward",
        },
    ] {
        register(&state, &shape).await;
    }
    (state, tempdir)
}

// ── Filters ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn unfiltered_listing_returns_the_whole_catalog_ordered_by_name() {
    let (state, _tmp) = seeded_catalog().await;

    assert_eq!(
        names(&state, "").await,
        vec!["ambilight", "aurora", "ripple", "spectrum"]
    );
}

#[tokio::test]
async fn category_filter_narrows_to_one_category() {
    let (state, _tmp) = seeded_catalog().await;

    assert_eq!(names(&state, "?category=audio").await, vec!["spectrum"]);
    assert_eq!(names(&state, "?category=ambient").await, vec!["aurora"]);
}

#[tokio::test]
async fn capability_filters_read_the_declared_capability() {
    let (state, _tmp) = seeded_catalog().await;

    assert_eq!(
        names(&state, "?audio_reactive=true").await,
        vec!["spectrum"]
    );
    assert_eq!(
        names(&state, "?screen_reactive=true").await,
        vec!["ambilight"]
    );
    assert_eq!(names(&state, "?input_reactive=true").await, vec!["ripple"]);
    assert_eq!(
        names(&state, "?audio_reactive=false").await,
        vec!["ambilight", "aurora", "ripple"]
    );
}

#[tokio::test]
async fn source_filter_separates_native_from_html() {
    let (state, _tmp) = seeded_catalog().await;

    assert_eq!(
        names(&state, "?source=native").await,
        vec!["aurora", "ripple"]
    );
    assert_eq!(
        names(&state, "?source=html").await,
        vec!["ambilight", "spectrum"]
    );
}

#[tokio::test]
async fn search_covers_name_description_author_and_tags() {
    let (state, _tmp) = seeded_catalog().await;

    assert_eq!(names(&state, "?q=northern").await, vec!["aurora"]);
    assert_eq!(names(&state, "?q=RIPPL").await, vec!["ripple"]);
    assert_eq!(
        names(&state, "?q=catalog-test").await,
        vec!["ambilight", "aurora", "ripple", "spectrum"]
    );
}

/// Case-insensitive means Unicode-insensitive.
///
/// Effect metadata is free text, so an ASCII fold leaves every
/// non-ASCII uppercase letter unmatchable: `to_ascii_lowercase` maps
/// `Ü` to itself, so a caller searching `überglow` would never find
/// `ÜBERGLOW`. Both directions are checked because the fold has to
/// apply to the stored text and the query term alike.
#[tokio::test]
async fn search_folds_case_beyond_ascii() {
    let (state, _tmp) = isolated_state();
    register(
        &state,
        &EffectShape {
            name: "ÜBERGLOW",
            category: EffectCategory::Ambient,
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
            html: false,
            description: "shouty umlaut",
        },
    )
    .await;
    register(
        &state,
        &EffectShape {
            name: "wärmelicht",
            category: EffectCategory::Ambient,
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
            html: false,
            description: "quiet umlaut",
        },
    )
    .await;

    // Lowercase query against uppercase stored text.
    assert_eq!(names(&state, "?q=überglow").await, vec!["ÜBERGLOW"]);
    assert_eq!(names(&state, "?q=über").await, vec!["ÜBERGLOW"]);

    // Uppercase query against lowercase stored text.
    assert_eq!(names(&state, "?q=WÄRMELICHT").await, vec!["wärmelicht"]);
    assert_eq!(names(&state, "?q=WÄRME").await, vec!["wärmelicht"]);
}

#[tokio::test]
async fn filters_compose_as_an_intersection() {
    let (state, _tmp) = seeded_catalog().await;

    assert_eq!(
        names(&state, "?source=html&screen_reactive=true").await,
        vec!["ambilight"]
    );
    assert!(
        names(&state, "?category=ambient&audio_reactive=true")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn unknown_filter_values_are_refused_rather_than_silently_empty() {
    let (state, _tmp) = seeded_catalog().await;

    for query in ["?category=gaming", "?source=wasm", "?include=everything"] {
        let (status, body) = list(&state, query).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{query} should be refused"
        );
        assert_eq!(body["error"]["code"], "validation_error", "{query}");
    }
}

// ── Expansion ────────────────────────────────────────────────────────────

#[tokio::test]
async fn summaries_omit_controls_and_presets_until_include_asks() {
    let (state, _tmp) = seeded_catalog().await;

    let (_, body) = list(&state, "").await;
    let first = &body["data"]["items"][0];
    assert!(
        first.get("controls").is_none(),
        "the default list shape carries no controls key"
    );
    assert!(
        first.get("presets").is_none(),
        "the default list shape carries no presets key"
    );
}

#[tokio::test]
async fn include_expands_summaries_in_one_round_trip() {
    let (state, _tmp) = seeded_catalog().await;

    let (status, body) = list(&state, "?include=controls,presets").await;
    assert_eq!(status, StatusCode::OK);

    for item in body["data"]["items"]
        .as_array()
        .expect("items should be an array")
    {
        assert_eq!(
            item["controls"][0]["id"], "speed",
            "every expanded summary carries its control definitions"
        );
        assert_eq!(
            item["presets"][0]["name"], "Calm",
            "every expanded summary carries its presets"
        );
    }
}

#[tokio::test]
async fn include_expands_one_side_at_a_time() {
    let (state, _tmp) = seeded_catalog().await;

    let (_, controls_only) = list(&state, "?include=controls").await;
    let item = &controls_only["data"]["items"][0];
    assert!(item.get("controls").is_some());
    assert!(item.get("presets").is_none());

    let (_, presets_only) = list(&state, "?include=presets").await;
    let item = &presets_only["data"]["items"][0];
    assert!(item.get("controls").is_none());
    assert!(item.get("presets").is_some());
}
