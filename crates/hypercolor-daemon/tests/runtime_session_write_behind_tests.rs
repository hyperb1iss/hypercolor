//! The runtime session is written behind the scene mutations that change it.
//!
//! A control patch answers as soon as its scene commit succeeds, and the
//! runtime-session file follows on the blocking pool. These pin what makes
//! that safe: the answer never waits on the write, the file always ends at
//! the newest committed state with few writes for a burst, a flush (what
//! shutdown runs per destination) waits for a write still in flight, and a
//! failed write is logged and retried rather than dropped, and the audit
//! trail still names the store the patch changed.

#![cfg(feature = "persistence-test-hooks")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::audit_log::{self, AuditLog};
use hypercolor_daemon::domain::scene::commit_scene;
use hypercolor_daemon::persistence::AtomicFileWriter;
use hypercolor_daemon::runtime_state;
use hypercolor_daemon::state_history::{StoreRoots, daemon_store_files};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
    EffectSource, EffectState,
};
use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::ZoneId;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(5);

struct Fixture {
    state: Arc<AppState>,
    app: axum::Router,
    audit: Arc<AuditLog>,
    runtime: AtomicFileWriter,
    zone_id: ZoneId,
    layer_id: SceneLayerId,
    _root: tempfile::TempDir,
}

fn speed_effect() -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "write-behind".to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: "runtime session write-behind probe".to_owned(),
        category: EffectCategory::Ambient,
        tags: Vec::new(),
        controls: vec![ControlDefinition {
            id: "speed".to_owned(),
            name: "Speed".to_owned(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(0.25),
            min: Some(0.0),
            max: Some(1.0),
            step: None,
            labels: Vec::new(),
            group: None,
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }],
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: "builtin/write-behind".into(),
        },
        license: None,
    }
}

/// The default scene running one effect layer with a `speed` control,
/// whose runtime projection the file already holds.
async fn fixture() -> Fixture {
    let root = tempfile::tempdir().expect("fixture directory");
    let data_dir = root.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("data directory");
    let mut state = AppState::new_with_data_dir(data_dir);
    let roots = StoreRoots {
        config_file: root.path().join("hypercolor.toml"),
        config_dir: root.path().to_path_buf(),
        data_dir: state.data_dir.clone(),
        state_dir: state.state_dir.clone(),
    };
    audit_log::install_store_observer();
    let audit = Arc::new(AuditLog::new(
        root.path().join("logs"),
        &daemon_store_files(&roots),
    ));
    state.audit_log = Some(Arc::clone(&audit));
    let state = Arc::new(state);

    let effect = speed_effect();
    let _ = state
        .domains
        .effects
        .register(EffectEntry {
            metadata: effect.clone(),
            source_path: "builtin/write-behind".into(),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        })
        .await;

    let mut scene = state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .expect("default scene")
        .clone();
    let layer_id = SceneLayerId::new();
    scene.zones[0].layers = vec![SceneLayer::from_effect(
        layer_id,
        effect.id,
        HashMap::new(),
        HashMap::new(),
        None,
    )];
    let zone_id = scene.zones[0].id;
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation.update_scene(scene).expect("seed default scene");
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("seed commit");
    // A whole-session save waits, so every test starts from a durable file.
    state.domains.runtime_session.save().await;

    let runtime = AtomicFileWriter::new(&state.runtime_state_path).expect("runtime writer");
    let app = api::build_router(Arc::clone(&state), None);
    Fixture {
        state,
        app,
        audit,
        runtime,
        zone_id,
        layer_id,
        _root: root,
    }
}

impl Fixture {
    /// Send one control patch the way Remote does and return its status,
    /// failing if the answer does not arrive within the deadline. Values
    /// are binary fractions because controls normalize through `f32`.
    async fn patch_speed(&self, speed: f64) -> StatusCode {
        let request = Request::builder()
            .method("PATCH")
            .uri(format!(
                "/api/v1/scene/zones/{}/layers/{}/controls",
                self.zone_id, self.layer_id
            ))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "values": { "speed": { "kind": "float", "value": speed } } }).to_string(),
            ))
            .expect("request should build");
        tokio::time::timeout(WAIT, self.app.clone().oneshot(request))
            .await
            .expect("the patch must answer without waiting on the runtime-session write")
            .expect("router is infallible")
            .status()
    }

    /// The `speed` the runtime-session file holds for the fixture layer.
    fn persisted_speed(&self) -> Option<ControlValue> {
        let snapshot = runtime_state::load(&self.state.runtime_state_path)
            .expect("runtime state should load")
            .expect("runtime state should exist");
        snapshot
            .default_scene_zones
            .iter()
            .flat_map(|zone| &zone.layers)
            .find(|layer| layer.id == self.layer_id)
            .and_then(|layer| match &layer.source {
                LayerSource::Effect { controls, .. } => controls.get("speed").cloned(),
                _ => None,
            })
    }
}

#[tokio::test]
async fn a_control_patch_answers_while_its_runtime_session_write_is_stalled() {
    let fixture = fixture().await;
    let hold = fixture.runtime.hold_writes();

    assert_eq!(fixture.patch_speed(0.75).await, StatusCode::OK);
    assert!(
        hold.wait_for_held(1, WAIT),
        "the runtime-session write is in flight behind the answer"
    );
    assert_eq!(
        fixture.persisted_speed(),
        None,
        "the answer came back before the file caught up"
    );

    hold.release();
    fixture.runtime.flush(WAIT).expect("the write lands");
    assert_eq!(fixture.persisted_speed(), Some(ControlValue::Float(0.75)));
}

#[tokio::test]
async fn a_burst_of_patches_lands_the_newest_state_in_two_writes() {
    let fixture = fixture().await;
    let before = fixture.runtime.replacement_count();
    let hold = fixture.runtime.hold_writes();

    assert_eq!(fixture.patch_speed(1.0 / 16.0).await, StatusCode::OK);
    assert!(hold.wait_for_held(1, WAIT), "the first write is in flight");
    for step in 2..=9 {
        assert_eq!(
            fixture.patch_speed(f64::from(step) / 16.0).await,
            StatusCode::OK,
            "every patch answers while the disk is stalled"
        );
    }

    hold.release();
    fixture.runtime.flush(WAIT).expect("the newest write lands");
    assert_eq!(
        fixture.persisted_speed(),
        Some(ControlValue::Float(9.0 / 16.0))
    );
    assert_eq!(
        fixture.runtime.replacement_count() - before,
        2,
        "nine patches write the snapshot in flight and then the newest one"
    );
}

#[tokio::test]
async fn a_flush_waits_for_a_runtime_session_write_still_in_flight() {
    let fixture = fixture().await;
    let hold = fixture.runtime.hold_writes();
    assert_eq!(fixture.patch_speed(0.625).await, StatusCode::OK);
    assert!(hold.wait_for_held(1, WAIT), "the write is in flight");

    let runtime = fixture.runtime.clone();
    let flush = tokio::task::spawn_blocking(move || runtime.flush(WAIT));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !flush.is_finished(),
        "a flush must not report the destination clean while its write is pending"
    );

    hold.release();
    flush
        .await
        .expect("flush task")
        .expect("the flush converges once the write lands");
    assert_eq!(fixture.persisted_speed(), Some(ControlValue::Float(0.625)));
}

#[derive(Clone, Default)]
struct CapturedWarnings(Arc<StdMutex<Vec<u8>>>);

impl CapturedWarnings {
    fn text(&self) -> String {
        String::from_utf8_lossy(
            &self
                .0
                .lock()
                .expect("captured log lock should not be poisoned"),
        )
        .into_owned()
    }
}

impl std::io::Write for CapturedWarnings {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("captured log lock should not be poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedWarnings {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn a_failed_runtime_session_write_is_logged_and_retried() {
    let fixture = fixture().await;
    let logs = CapturedWarnings::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    fixture.runtime.set_injected_replace_failures(1);

    let _subscriber = tracing::subscriber::set_default(subscriber);
    assert_eq!(fixture.patch_speed(0.375).await, StatusCode::OK);
    // The failure is logged from the blocking worker after the answer, so
    // wait for it within a bound rather than assume it already ran.
    let deadline = Instant::now() + WAIT;
    while !logs
        .text()
        .contains("Failed to persist runtime session snapshot")
    {
        assert!(
            Instant::now() < deadline,
            "the failed write must be logged: {}",
            logs.text()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let warnings = logs.text();
    assert!(
        warnings.contains("injected persistence replacement failure"),
        "the warning carries the cause: {warnings}"
    );
    assert!(
        warnings.contains("runtime-state.json"),
        "the warning names the file: {warnings}"
    );

    fixture
        .runtime
        .flush(WAIT)
        .expect("the retry supervisor lands the newest snapshot");
    assert_eq!(fixture.persisted_speed(), Some(ControlValue::Float(0.375)));
    assert_eq!(fixture.runtime.pending_failure(), None);
}

#[tokio::test]
async fn a_control_patch_is_audited_with_the_runtime_session_it_writes_behind() {
    let fixture = fixture().await;
    let hold = fixture.runtime.hold_writes();

    assert_eq!(fixture.patch_speed(0.125).await, StatusCode::OK);
    let entry = fixture
        .audit
        .recent(1)
        .expect("recent entries")
        .pop()
        .expect("the patch is audited");
    assert!(entry.path.ends_with("/controls"), "{}", entry.path);
    assert!(
        entry.stores.iter().any(|store| store == "runtime-state"),
        "the patch changed the runtime session even though its write is still \
         stalled: {:?}",
        entry.stores
    );

    hold.release();
    fixture.runtime.flush(WAIT).expect("the write lands");
}
