//! Integration tests for the MCP HTTP surface and its reusable domain helpers.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::config::ConfigManager;
use hypercolor_core::input::{
    AudioSource, AudioSourceRole, InputData, InputSource, ManagedSourceRole, SourceIssue,
    SourceKind, SourceRoleBinding, SourceStatusHandle, SourceStatusReporter,
};
use hypercolor_core::scene::OutputPlacement;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::mcp;
use hypercolor_daemon::mcp::prompts::{
    build_prompt_definitions, get_prompt_messages, is_valid_prompt,
};
use hypercolor_daemon::mcp::resources::{
    build_resource_definitions, is_valid_resource_uri, read_resource_with_state,
};
use hypercolor_daemon::mcp::tools::{ToolError, build_tool_definitions, execute_tool_with_state};
use hypercolor_daemon::runtime_state;
use hypercolor_daemon::scene_store;
use hypercolor_types::config::{CURRENT_SCHEMA_VERSION, McpConfig};
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceOrigin, DeviceTopologyHint, DisplayFrameFormat, SegmentInfo,
};
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
    EffectSource,
};
use hypercolor_types::event::{
    ChangeTrigger, EffectStopReason, HypercolorEvent, SceneChangeReason, ZoneChangeKind,
};
use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::{SceneId, Zone};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use reqwest::{Client, Response};
use serde_json::{Value, json};
use strum::VariantNames;
use tempfile::TempDir;
use uuid::Uuid;

const INIT_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
fn effect_controls(zone: &Zone) -> Option<&HashMap<String, ControlValue>> {
    zone.layers.iter().find_map(|layer| match &layer.source {
        LayerSource::Effect { controls, .. } => Some(controls),
        _ => None,
    })
}

async fn spawn_router(router: axum::Router) -> (Client, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("read local addr");

    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("build reqwest client");
    (client, format!("http://{addr}"))
}

fn stateless_mcp_config() -> McpConfig {
    McpConfig {
        enabled: true,
        stateful_mode: false,
        json_response: true,
        ..McpConfig::default()
    }
}

fn isolated_state_with_tempdir() -> (AppState, TempDir) {
    let (builder, tempdir) = isolated_state_builder_with_tempdir();
    (builder.build(), tempdir)
}

fn isolated_state_builder_with_tempdir() -> (AppStateBuilder, TempDir) {
    let tempdir = TempDir::new().expect("create temp dir");
    let data_dir = tempdir.path().join("data");
    fs::create_dir_all(&data_dir).expect("create temp data dir");
    (AppStateBuilder::new(data_dir), tempdir)
}

struct FailedInputSource {
    status: SourceStatusReporter,
    running: bool,
}

impl FailedInputSource {
    fn new() -> Self {
        Self {
            status: SourceStatusReporter::new(
                "failed_mcp_audio",
                SourceKind::Audio,
                "test_capture",
                true,
                true,
                true,
            ),
            running: false,
        }
    }
}

impl InputSource for FailedInputSource {
    fn name(&self) -> &'static str {
        "FailedInputSource"
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        let session = self
            .status
            .begin_session()?
            .expect("manager-bound test source should create a status session");
        assert!(session.failed(SourceIssue::new(
            "capture_worker_exited",
            "test worker exited",
            true,
        )));
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status.stop();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl SourceRoleBinding for FailedInputSource {
    type Role = AudioSourceRole;
}

impl AudioSource for FailedInputSource {}

#[tokio::test]
async fn diagnose_matches_rest_defaults_and_excludes_protected_parity() {
    let (state, _tempdir) = isolated_state_with_tempdir();

    let state = Arc::new(state);
    let tool_payload = execute_tool_with_state("diagnose", &json!({}), state.as_ref())
        .await
        .expect("diagnose should succeed");
    let (client, base_url) = spawn_router(api::build_router(Arc::clone(&state), None)).await;
    let response = client
        .post(format!("{base_url}/api/v1/diagnose"))
        .json(&json!({}))
        .send()
        .await
        .expect("REST diagnose should complete");
    assert!(response.status().is_success());
    let rest_payload: Value = response
        .json()
        .await
        .expect("REST diagnose should return JSON");

    assert_eq!(tool_payload, rest_payload["data"]);
    assert!(tool_payload["checks"].as_array().is_some_and(|checks| {
        checks.iter().all(|check| {
            check["name"] != "macos_screen_parity" && check["name"] != "uptime_seconds"
        })
    }));
    assert!(
        tool_payload["snapshot"]
            .get("macos_screen_parity")
            .is_none()
    );
}

#[tokio::test]
async fn effect_and_scene_listings_match_their_rest_summaries() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);

    let created =
        execute_tool_with_state("create_scene", &json!({ "name": "Parity" }), state.as_ref())
            .await
            .expect("scene creation should succeed");
    assert!(created["scene_id"].is_string());

    let (client, base_url) = spawn_router(api::build_router(Arc::clone(&state), None)).await;

    let rest_effects: Value = client
        .get(format!(
            "{base_url}/api/v1/effects?include=controls,presets&limit=200"
        ))
        .send()
        .await
        .expect("REST effects should complete")
        .json()
        .await
        .expect("REST effects should return JSON");
    let tool_effects =
        execute_tool_with_state("list_effects", &json!({ "limit": 100 }), state.as_ref())
            .await
            .expect("list_effects should succeed");
    assert_eq!(tool_effects["effects"], rest_effects["data"]["items"]);

    let resource_effects = read_resource_with_state("hypercolor://effects", &state)
        .await
        .expect("effects resource should exist");
    let rest_catalog: Value = client
        .get(format!("{base_url}/api/v1/effects?limit=200"))
        .send()
        .await
        .expect("REST catalog should complete")
        .json()
        .await
        .expect("REST catalog should return JSON");
    assert_eq!(resource_effects["effects"], rest_catalog["data"]["items"]);

    let rest_scenes: Value = client
        .get(format!("{base_url}/api/v1/scenes"))
        .send()
        .await
        .expect("REST scenes should complete")
        .json()
        .await
        .expect("REST scenes should return JSON");
    let tool_scenes = execute_tool_with_state("list_scenes", &json!({}), state.as_ref())
        .await
        .expect("list_scenes should succeed");
    let resource_scenes = read_resource_with_state("hypercolor://scenes", &state)
        .await
        .expect("scenes resource should exist");
    assert_eq!(tool_scenes["scenes"], resource_scenes["scenes"]);
    for (tool_row, rest_row) in tool_scenes["scenes"]
        .as_array()
        .expect("tool scenes")
        .iter()
        .zip(
            rest_scenes["data"]["items"]
                .as_array()
                .expect("rest scenes"),
        )
    {
        let mut expected = rest_row.clone();
        expected["active"] = tool_row["active"].clone();
        assert_eq!(tool_row, &expected);
    }
}

#[tokio::test]
async fn diagnose_reports_demanded_input_failure_as_unhealthy() {
    let (state, _tempdir) = isolated_state_with_tempdir();

    {
        let manager = state.input_manager();
        manager
            .add_source(ManagedSourceRole::audio(Box::new(FailedInputSource::new())))
            .expect("failed audio source should register");
        manager.start_all().expect("test input graph should start");
    }

    let result = execute_tool_with_state("diagnose", &json!({}), &state)
        .await
        .expect("diagnose should succeed");

    assert!(result["checks"].as_array().is_some_and(|checks| {
        checks.iter().any(|check| {
            check["category"] == "input"
                && check["name"] == "failed_mcp_audio"
                && check["status"] == "fail"
                && check["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("capture_worker_exited"))
        })
    }));
    assert!(
        result["summary"]["failed"]
            .as_u64()
            .is_some_and(|failed| failed > 0)
    );
}

#[tokio::test]
async fn mcp_status_surfaces_are_exact_with_running_input_manager() {
    let (state, _tempdir) = isolated_state_with_tempdir();

    state
        .input_manager()
        .start_all()
        .expect("input manager should start");

    let status = tokio::time::timeout(
        Duration::from_secs(1),
        execute_tool_with_state("get_status", &json!({}), &state),
    )
    .await
    .expect("get_status should respond promptly")
    .expect("get_status should succeed");
    assert_eq!(status["inputs"]["sources"], json!([]));
    assert!(status["inputs"]["source_graph_generation"].is_number());

    let resource = tokio::time::timeout(
        Duration::from_secs(1),
        read_resource_with_state("hypercolor://state", &state),
    )
    .await
    .expect("state resource should respond promptly")
    .expect("state resource should exist");
    assert_eq!(status, resource, "tool and resource payloads must be exact");
    assert_eq!(resource["inputs"]["sources"], json!([]));

    let diagnose = tokio::time::timeout(
        Duration::from_secs(1),
        execute_tool_with_state("diagnose", &json!({}), &state),
    )
    .await
    .expect("diagnose should respond promptly")
    .expect("diagnose should succeed");

    assert_eq!(diagnose["snapshot"]["input"]["sources"], json!([]));
}

#[tokio::test]
async fn mcp_status_surfaces_report_effective_session_pause() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let generation = state.output_power.begin_session_transition();

    state
        .output_power
        .pause_for_session(
            &state.event_bus,
            generation,
            hypercolor_types::session::OffOutputBehavior::Static,
            [0, 0, 0],
        )
        .await;

    let status = execute_tool_with_state("get_status", &json!({}), &state)
        .await
        .expect("get_status should succeed");
    assert_eq!(status["running"], false);
    assert_eq!(status["paused"], true);

    let resource = read_resource_with_state("hypercolor://state", &state)
        .await
        .expect("state resource should exist");
    assert_eq!(resource["running"], false);
    assert_eq!(resource["paused"], true);

    state
        .output_power
        .set_output_stopped(&state.event_bus)
        .await;

    let stopped_status = execute_tool_with_state("get_status", &json!({}), &state)
        .await
        .expect("stopped status should succeed");
    assert_eq!(stopped_status["running"], false);
    // Paused is the exact complement of running on every surface now, so
    // MCP no longer contradicts GET /output about a stop (Spec 78 §7.1).
    assert_eq!(stopped_status["paused"], true);

    let stopped_resource = read_resource_with_state("hypercolor://state", &state)
        .await
        .expect("stopped state resource should exist");
    assert_eq!(stopped_resource["running"], false);
    assert_eq!(stopped_resource["paused"], true);
}

async fn insert_test_display_device(state: &Arc<AppState>, name: &str) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: Some("LCD".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("wled", "usb", ConnectionType::Usb),
        segments: vec![SegmentInfo {
            name: "LCD".to_owned(),
            led_count: 320 * 320,
            topology: DeviceTopologyHint::Display {
                width: 320,
                height: 320,
                circular: true,
                format: DisplayFrameFormat::Jpeg,
            },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 320 * 320,
            supports_direct: true,
            supports_brightness: true,
            has_display: true,
            display_resolution: Some((320, 320)),
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let _ = state.device_registry.add(info).await;
    id
}

fn test_html_effect_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} html effect"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned(), "html".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Html {
            path: format!("/tmp/{name}.html").into(),
        },
        license: None,
    }
}

fn test_display_face_effect_metadata(name: &str) -> EffectMetadata {
    let mut metadata = test_html_effect_metadata(name);
    metadata.category = EffectCategory::Display;
    metadata.controls.push(ControlDefinition {
        id: "title".to_owned(),
        name: "Title".to_owned(),
        kind: ControlKind::Text,
        control_type: ControlType::TextInput,
        default_value: ControlValue::Text("System".to_owned()),
        min: None,
        max: None,
        step: None,
        labels: Vec::new(),
        group: Some("General".to_owned()),
        tooltip: None,
        aspect_lock: None,
        preview_source: None,
        binding: None,
    });
    metadata
}

async fn insert_test_display_face_effect(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = test_display_face_effect_metadata(name);
    let entry = hypercolor_core::effect::EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{name}.html").into(),
        modified: std::time::SystemTime::now(),
        state: hypercolor_types::effect::EffectState::Loading,
    };
    let _ = state.domains.effects.register(entry).await;
    metadata
}

async fn insert_test_effect(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} ambient effect"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
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
            tooltip: Some("Animation speed".to_owned()),
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }],
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    };
    let entry = hypercolor_core::effect::EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{name}.rs").into(),
        modified: std::time::SystemTime::now(),
        state: hypercolor_types::effect::EffectState::Loading,
    };
    let _ = state.domains.effects.register(entry).await;
    metadata
}

fn test_device_zone(id: &str) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: format!("mock:{id}"),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.2, 0.2),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 1,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn test_layout(id: &str, zones: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: id.to_owned(),
        name: id.to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

async fn seed_multi_zone_primary_assignment(
    state: &Arc<AppState>,
    metadata: &EffectMetadata,
) -> SpatialLayout {
    let primary_layout = test_layout("primary-layout", vec![test_device_zone("primary-zone")]);
    let custom_zone = test_device_zone("custom-zone");
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .upsert_primary_zone(
            metadata,
            HashMap::new(),
            None,
            primary_layout.clone(),
            hypercolor_types::event::ChangeTrigger::System,
            None,
        )
        .expect("primary zone should be seeded");
    let custom_id = mutation
        .create_zone(SceneId::DEFAULT, "Custom".to_owned(), None, (320, 200))
        .expect("custom zone should be created");
    mutation
        .assign_output(
            SceneId::DEFAULT,
            custom_id,
            custom_zone,
            OutputPlacement::AutoGrid,
        )
        .expect("custom zone should claim a zone");
    hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
        .await
        .expect("multi-zone scene should commit");
    primary_layout
}

fn scenes_path(state: &AppState) -> PathBuf {
    state.data_dir.join("scenes.json")
}

#[derive(Debug, PartialEq)]
struct McpMutationSnapshot {
    power: hypercolor_daemon::output_power::OutputPowerState,
    active_scene_id: Option<SceneId>,
    revision: u64,
    scenes: Value,
}

async fn mcp_mutation_snapshot(state: &AppState) -> McpMutationSnapshot {
    let manager = state.scene_manager.snapshot().await;
    McpMutationSnapshot {
        power: state.output_power.snapshot(),
        active_scene_id: manager.active_scene_id().copied(),
        revision: state.scene_manager.revision(),
        scenes: serde_json::to_value(manager.list()).expect("scenes should serialize"),
    }
}

async fn assert_schema_refusal_preserves_state(
    state: &AppState,
    tool: &str,
    params: Value,
    parameter: &str,
) {
    let before = mcp_mutation_snapshot(state).await;
    let error = execute_tool_with_state(tool, &params, state)
        .await
        .expect_err(&format!("{tool} should reject malformed {parameter}"));
    assert_eq!(error.error_code(), -32602);
    match error {
        ToolError::InvalidParam { param, .. } => assert_eq!(param, parameter),
        other => panic!("expected invalid {parameter} error, got {other:?}"),
    }
    assert_eq!(mcp_mutation_snapshot(state).await, before);
}

async fn post_raw(client: &Client, url: &str, body: &str, session_id: Option<&str>) -> Response {
    let mut request = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body.to_owned());

    if let Some(session_id) = session_id {
        request = request.header("Mcp-Session-Id", session_id);
    }

    request.send().await.expect("send MCP request")
}

async fn post_json(client: &Client, url: &str, body: Value, session_id: Option<&str>) -> Response {
    post_raw(
        client,
        url,
        &serde_json::to_string(&body).expect("serialize json-rpc body"),
        session_id,
    )
    .await
}

async fn parse_jsonrpc_response(response: Response) -> (Option<String>, Value, String, String) {
    let headers = response.headers().clone();
    let session_id = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = response.text().await.expect("read response body");
    let payload = extract_jsonrpc_payload(&body);
    (session_id, payload, content_type, body)
}

fn extract_jsonrpc_payload(body: &str) -> Value {
    if let Ok(json) = serde_json::from_str(body) {
        return json;
    }

    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        if let Ok(json) = serde_json::from_str::<Value>(data.trim()) {
            return json;
        }
    }

    panic!("response body did not contain a JSON-RPC payload: {body}");
}

#[tokio::test]
async fn mcp_http_initialize_returns_json_in_stateless_mode() {
    let (state, _tempdir) = isolated_state_with_tempdir();

    let state = Arc::new(state);
    let router = mcp::build_router(Arc::clone(&state), &stateless_mcp_config()).with_state(state);
    let (client, base_url) = spawn_router(router).await;

    let response = post_raw(&client, &format!("{base_url}/mcp"), INIT_BODY, None).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let (_session_id, payload, content_type, _body) = parse_jsonrpc_response(response).await;
    assert!(
        content_type.contains("application/json"),
        "expected application/json, got {content_type}"
    );

    let result = payload.get("result").expect("initialize result");
    let latest_protocol = serde_json::to_value(rmcp::model::ProtocolVersion::LATEST)
        .expect("serialize protocol version");
    assert_eq!(result["protocolVersion"], latest_protocol);
    assert!(result["capabilities"]["tools"].is_object());
    assert!(result["capabilities"]["resources"].is_object());
    assert!(result["capabilities"]["prompts"].is_object());
    assert_eq!(result["serverInfo"]["name"], "hypercolor");
    assert!(
        result["capabilities"].get("logging").is_none(),
        "server should not advertise unsupported logging"
    );
}

#[tokio::test]
async fn mcp_http_tools_list_and_call_return_structured_results() {
    let (state, _tempdir) = isolated_state_with_tempdir();

    let state = Arc::new(state);
    insert_test_effect(&state, "Aurora").await;
    insert_test_effect(&state, "Aurora Glow").await;
    let router = mcp::build_router(Arc::clone(&state), &stateless_mcp_config()).with_state(state);
    let (client, base_url) = spawn_router(router).await;
    let mcp_url = format!("{base_url}/mcp");

    let list_response = post_json(
        &client,
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }),
        None,
    )
    .await;
    let (_session_id, list_payload, _content_type, _body) =
        parse_jsonrpc_response(list_response).await;
    let tools = list_payload["result"]["tools"]
        .as_array()
        .expect("tools list array");
    assert_eq!(tools.len(), 17);
    assert!(tools.iter().all(|tool| tool["outputSchema"].is_object()));
    assert!(tools.iter().any(|tool| tool["name"] == "set_display_face"));

    let call_response = post_json(
        &client,
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "get_status",
                "arguments": {}
            }
        }),
        None,
    )
    .await;
    let (_session_id, call_payload, _content_type, _body) =
        parse_jsonrpc_response(call_response).await;
    let result = call_payload.get("result").expect("tool call result");
    assert_eq!(result["isError"], false);
    assert!(result["structuredContent"]["devices"].is_object());
    assert!(result["structuredContent"]["uptime_seconds"].is_number());
    assert!(result["content"].is_array());

    let error_response = post_json(
        &client,
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "set_color",
                "arguments": {}
            }
        }),
        None,
    )
    .await;
    let (_session_id, error_payload, _content_type, _body) =
        parse_jsonrpc_response(error_response).await;
    let error_result = error_payload.get("result").expect("tool error result");
    assert_eq!(error_result["isError"], true);
    assert_eq!(error_result["structuredContent"]["code"], -32602);

    let selector_response = post_json(
        &client,
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 41,
            "method": "tools/call",
            "params": {
                "name": "set_effect",
                "arguments": { "query": "auro" }
            }
        }),
        None,
    )
    .await;
    let (_session_id, selector_payload, _content_type, _body) =
        parse_jsonrpc_response(selector_response).await;
    let selector_error = &selector_payload["result"]["structuredContent"];
    assert_eq!(selector_error["code"], -32602);
    assert_eq!(selector_error["details"]["kind"], "ambiguous");
    assert_eq!(selector_error["details"]["parameter"], "query");
    assert_eq!(selector_error["details"]["query"], "auro");
    assert_eq!(selector_error["details"]["candidates"][0]["name"], "Aurora");
    assert_eq!(
        selector_error["details"]["candidates"][1]["name"],
        "Aurora Glow"
    );
}

#[tokio::test]
async fn mcp_http_resources_and_prompts_roundtrip() {
    let (state, _tempdir) = isolated_state_with_tempdir();

    let state = Arc::new(state);
    let router = mcp::build_router(Arc::clone(&state), &stateless_mcp_config()).with_state(state);
    let (client, base_url) = spawn_router(router).await;
    let mcp_url = format!("{base_url}/mcp");

    let resources_response = post_json(
        &client,
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "resources/list"
        }),
        None,
    )
    .await;
    let (_session_id, resources_payload, _content_type, _body) =
        parse_jsonrpc_response(resources_response).await;
    let resources = resources_payload["result"]["resources"]
        .as_array()
        .expect("resource list array");
    assert_eq!(resources.len(), 5);
    assert!(
        resources
            .iter()
            .any(|resource| resource["uri"] == "hypercolor://scenes")
    );

    let read_response = post_json(
        &client,
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "resources/read",
            "params": {
                "uri": "hypercolor://state"
            }
        }),
        None,
    )
    .await;
    let (_session_id, read_payload, _content_type, _body) =
        parse_jsonrpc_response(read_response).await;
    let contents = read_payload["result"]["contents"]
        .as_array()
        .expect("resource contents array");
    assert_eq!(contents[0]["uri"], "hypercolor://state");
    assert_eq!(contents[0]["mimeType"], "application/json");

    let prompts_response = post_json(
        &client,
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "prompts/list"
        }),
        None,
    )
    .await;
    let (_session_id, prompts_payload, _content_type, _body) =
        parse_jsonrpc_response(prompts_response).await;
    let prompts = prompts_payload["result"]["prompts"]
        .as_array()
        .expect("prompt list array");
    assert_eq!(prompts.len(), 3);

    let prompt_get_response = post_json(
        &client,
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "prompts/get",
            "params": {
                "name": "mood_lighting",
                "arguments": {
                    "mood": "cozy evening"
                }
            }
        }),
        None,
    )
    .await;
    let (_session_id, prompt_result_payload, _content_type, _body) =
        parse_jsonrpc_response(prompt_get_response).await;
    let prompt_result = prompt_result_payload.get("result").expect("prompt result");
    assert!(prompt_result["messages"].is_array());
    assert_eq!(
        prompt_result["description"],
        "Configure Hypercolor RGB lighting to match a mood"
    );
}

#[tokio::test]
async fn mcp_http_stateful_mode_uses_session_headers_and_sse() {
    let config = McpConfig {
        enabled: true,
        stateful_mode: true,
        json_response: true,
        ..McpConfig::default()
    };
    let (state, _tempdir) = isolated_state_with_tempdir();

    let state = Arc::new(state);
    let router = mcp::build_router(Arc::clone(&state), &config).with_state(state);
    let (client, base_url) = spawn_router(router).await;
    let mcp_url = format!("{base_url}/mcp");

    let init_response = post_raw(&client, &mcp_url, INIT_BODY, None).await;
    assert_eq!(init_response.status(), reqwest::StatusCode::OK);
    let session_id = init_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let content_type = init_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = init_response.text().await.expect("read init SSE body");
    assert!(
        content_type.contains("text/event-stream"),
        "expected SSE response, got {content_type}"
    );
    assert!(
        body.contains("retry: 3000"),
        "expected SSE priming event, got {body}"
    );

    let session_id = session_id.expect("stateful initialize should return session id");
    let list_response = post_json(
        &client,
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/list"
        }),
        Some(&session_id),
    )
    .await;
    let list_content_type = list_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let list_body = list_response.text().await.expect("read list SSE body");
    assert!(list_content_type.contains("text/event-stream"));
    assert!(
        list_body.contains("data:"),
        "expected SSE framing, got {list_body}"
    );
}

#[tokio::test]
async fn api_router_mounts_mcp_when_enabled_in_config() {
    let tempdir = TempDir::new().expect("create temp dir");
    let config_path = tempdir.path().join("hypercolor.toml");
    std::fs::write(
        &config_path,
        format!(
            "schema_version = {CURRENT_SCHEMA_VERSION}\n[mcp]\nenabled = true\nstateful_mode = false\njson_response = true\n"
        ),
    )
    .expect("write config file");

    let manager = Arc::new(ConfigManager::new(config_path).expect("load config manager"));
    let (builder, _state_tempdir) = isolated_state_builder_with_tempdir();
    let state = builder.with_config_manager(manager).build();

    let router = api::build_router(Arc::new(state), None);
    let (client, base_url) = spawn_router(router).await;

    let response = post_raw(&client, &format!("{base_url}/mcp"), INIT_BODY, None).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let (_session_id, payload, content_type, _body) = parse_jsonrpc_response(response).await;
    assert!(content_type.contains("application/json"));
    assert_eq!(payload["result"]["serverInfo"]["name"], "hypercolor");
}

#[tokio::test]
async fn stateful_scene_tools_persist_named_scenes_and_activation_state() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);

    let create_result = execute_tool_with_state(
        "create_scene",
        &json!({
            "name": "Focus",
            "description": "Deep work lighting"
        }),
        state.as_ref(),
    )
    .await
    .expect("scene creation should succeed");
    let scene_id = create_result["scene_id"]
        .as_str()
        .expect("scene id should be returned")
        .to_owned();

    let list_result = execute_tool_with_state("list_scenes", &json!({}), state.as_ref())
        .await
        .expect("scene list should succeed");
    assert_eq!(list_result["total"], 1);
    assert_eq!(list_result["scenes"][0]["name"], "Focus");
    assert_eq!(list_result["scenes"][0]["active"], false);

    let store = scene_store::load(&scenes_path(state.as_ref())).expect("scene store should load");
    assert_eq!(store.len(), 1);
    let stored_scene = store.list().next().expect("named scene should persist");
    assert!(stored_scene.metadata.is_empty());

    let mut events = state.event_bus.subscribe_all();
    let activate_result = execute_tool_with_state(
        "activate_scene",
        &json!({
            "name": "Focus",
            "transition_ms": 250.0
        }),
        state.as_ref(),
    )
    .await
    .expect("scene activation should succeed");
    assert_eq!(activate_result["activated"], true);
    assert_eq!(activate_result["scene"]["id"], scene_id);
    assert_eq!(activate_result["transition_ms"], 250);

    let snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(snapshot.active_scene_id, Some(scene_id.clone()));

    let mut saw_active_scene_event = false;
    while let Ok(timestamped) = events.try_recv() {
        if let HypercolorEvent::ActiveSceneChanged {
            previous,
            current,
            current_name,
            current_snapshot_locked,
            reason,
            ..
        } = timestamped.event
        {
            assert_eq!(previous, Some(SceneId::DEFAULT));
            assert_eq!(current.to_string(), scene_id);
            assert_eq!(current_name, "Focus");
            assert!(!current_snapshot_locked);
            assert_eq!(reason, SceneChangeReason::UserActivate);
            saw_active_scene_event = true;
        }
    }
    assert!(saw_active_scene_event, "expected active-scene MCP event");
}

#[tokio::test]
async fn stateful_display_face_tool_assigns_and_clears_face_zones() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;

    let mut assign_events = state.event_bus.subscribe_all();
    let assign_result = execute_tool_with_state(
        "set_display_face",
        &json!({
            "device": display_id.to_string(),
            "effect_id": face.id.to_string(),
            "scope": "scene",
            "controls": {
                "title": "CPU"
            }
        }),
        state.as_ref(),
    )
    .await
    .expect("display face assignment should succeed");
    assert_eq!(assign_result["scene_id"], SceneId::DEFAULT.to_string());
    assert_eq!(assign_result["effect"]["id"], face.id.to_string());
    assert_eq!(
        assign_result["zone"]["display_target"]["device_id"],
        display_id.to_string()
    );
    assert_eq!(assign_result["device"]["width"], 320);
    assert_eq!(
        assign_result["zone"]["layers"][0]["source"]["controls"]["title"]["value"],
        "CPU"
    );

    let assign_snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(
        assign_snapshot.active_scene_id,
        Some(SceneId::DEFAULT.to_string())
    );
    assert_eq!(assign_snapshot.default_scene_zones.len(), 2);

    let mut saw_assign_event = false;
    while let Ok(timestamped) = assign_events.try_recv() {
        if let HypercolorEvent::ZoneChanged {
            scene_id,
            kind,
            role,
            ..
        } = timestamped.event
        {
            assert_eq!(scene_id, SceneId::DEFAULT);
            assert_eq!(role, hypercolor_types::scene::ZoneRole::Display);
            assert_eq!(kind, ZoneChangeKind::Created);
            saw_assign_event = true;
        }
    }
    assert!(saw_assign_event, "expected display-face assign event");

    let mut clear_events = state.event_bus.subscribe_all();
    let clear_result = execute_tool_with_state(
        "set_display_face",
        &json!({
            "device": display_id.to_string(),
            "scope": "scene",
            "clear": true
        }),
        state.as_ref(),
    )
    .await
    .expect("display face clear should succeed");
    assert_eq!(clear_result["scene_id"], SceneId::DEFAULT.to_string());
    assert_eq!(clear_result["cleared"], true);
    assert_eq!(
        clear_result["zone"]["display_target"]["device_id"],
        display_id.to_string()
    );
    assert_eq!(
        clear_result["zone"]["layers"].as_array().map(Vec::len),
        Some(0)
    );

    let clear_snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(clear_snapshot.default_scene_zones.len(), 2);
    let display_zone = clear_snapshot
        .default_scene_zones
        .iter()
        .find(|zone| zone.role == hypercolor_types::scene::ZoneRole::Display)
        .expect("display screen surface should survive face clear");
    assert_eq!(display_zone.effect_ids().next(), None);
    assert!(display_zone.layers.is_empty());

    let mut saw_clear_event = false;
    while let Ok(timestamped) = clear_events.try_recv() {
        if let HypercolorEvent::ZoneChanged {
            scene_id,
            kind,
            role,
            ..
        } = timestamped.event
        {
            assert_eq!(scene_id, SceneId::DEFAULT);
            assert_eq!(role, hypercolor_types::scene::ZoneRole::Display);
            assert_eq!(kind, ZoneChangeKind::Updated);
            saw_clear_event = true;
        }
    }
    assert!(saw_clear_event, "expected display-face clear event");
}

#[tokio::test]
async fn stateful_set_effect_rejects_display_faces() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let face = insert_test_display_face_effect(&state, "System Monitor").await;

    let error = execute_tool_with_state(
        "set_effect",
        &json!({
            "query": face.name,
        }),
        state.as_ref(),
    )
    .await
    .expect_err("display faces should not be applied as LED effects");

    assert!(format!("{error}").contains("display face"));
}

#[test]
fn set_effect_advertises_only_the_closed_cut_transition() {
    let tools = build_tool_definitions();
    let set_effect = tools
        .iter()
        .find(|tool| tool.name == "set_effect")
        .expect("set_effect should be registered");

    let properties = set_effect.input_schema["properties"]
        .as_object()
        .expect("set_effect should declare properties");
    let mut declared = properties.keys().cloned().collect::<Vec<_>>();
    declared.sort();
    assert_eq!(
        declared,
        vec![
            "controls".to_owned(),
            "query".to_owned(),
            "transition".to_owned()
        ]
    );
    let transition = &set_effect.input_schema["properties"]["transition"];
    assert_eq!(transition["additionalProperties"], json!(false));
    assert_eq!(transition["properties"]["type"]["enum"], json!(["cut"]));
    assert_eq!(
        set_effect.input_schema["additionalProperties"],
        json!(false),
        "the closed shape is what stops a client sending a deleted parameter"
    );
}

#[test]
fn adjust_controls_advertises_recursive_canonical_values() {
    let tools = build_tool_definitions();
    let adjust = tools
        .iter()
        .find(|tool| tool.name == "adjust_controls")
        .expect("adjust_controls should be registered");
    assert_eq!(
        adjust.input_schema["properties"]["values"]["additionalProperties"]["$ref"],
        "#/$defs/controlValue"
    );

    let variants = adjust.input_schema["$defs"]["controlValue"]["oneOf"]
        .as_array()
        .expect("ControlValue should be a tagged union");
    let tags = variants
        .iter()
        .map(|variant| {
            variant["properties"]["kind"]["const"]
                .as_str()
                .expect("every variant should pin its tag")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tags,
        vec![
            "null",
            "bool",
            "int",
            "float",
            "text",
            "secret_ref",
            "ip",
            "mac",
            "duration",
            "color_rgb",
            "color_rgba",
            "color_linear",
            "gradient",
            "rect",
            "enum",
            "flags",
            "list",
            "map",
            "unknown",
        ]
    );
    let variant = |kind: &str| {
        variants
            .iter()
            .find(|variant| variant["properties"]["kind"]["const"] == kind)
            .unwrap_or_else(|| panic!("{kind} should be advertised"))
    };
    assert_eq!(
        variant("list")["properties"]["value"]["items"]["$ref"],
        "#/$defs/controlValue"
    );
    assert_eq!(
        variant("map")["properties"]["value"]["additionalProperties"]["$ref"],
        "#/$defs/controlValue"
    );

    let validator = jsonschema::validator_for(&adjust.input_schema)
        .expect("adjust_controls schema should compile");
    let valid = json!({
        "zone": "primary",
        "layer": "layer-1",
        "values": {
            "nested": {
                "kind": "map",
                "value": {
                    "items": {
                        "kind": "list",
                        "value": [
                            { "kind": "float", "value": 0.5 },
                            { "kind": "unknown" }
                        ]
                    }
                }
            }
        }
    });
    assert!(validator.is_valid(&valid));
    for invalid in [
        json!({
            "zone": "primary",
            "layer": "layer-1",
            "values": { "speed": 0.5 }
        }),
        json!({
            "zone": "primary",
            "layer": "layer-1",
            "values": { "future": { "kind": "vector3", "value": [0, 0, 0] } }
        }),
    ] {
        assert!(!validator.is_valid(&invalid));
    }
}

#[test]
fn adjust_controls_schema_matches_canonical_value_boundaries() {
    let tools = build_tool_definitions();
    let adjust = tools
        .iter()
        .find(|tool| tool.name == "adjust_controls")
        .expect("adjust_controls should be registered");
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(&adjust.input_schema)
        .expect("adjust_controls schema should compile with format assertions");
    let assert_parity = |label: &str, value: Value, expected: bool| {
        let input = json!({
            "zone": "primary",
            "layer": "layer-1",
            "values": { "candidate": value.clone() }
        });
        let schema_accepts = validator.is_valid(&input);
        let canonical_accepts = serde_json::from_value::<ControlValue>(value).is_ok();
        assert_eq!(schema_accepts, expected, "schema result for {label}");
        assert_eq!(canonical_accepts, expected, "canonical result for {label}");
        assert_eq!(
            schema_accepts, canonical_accepts,
            "schema and canonical admission diverged for {label}"
        );
    };

    for (label, value) in [
        ("i64 minimum", json!({ "kind": "int", "value": i64::MIN })),
        ("i64 maximum", json!({ "kind": "int", "value": i64::MAX })),
        ("IPv4", json!({ "kind": "ip", "value": "192.0.2.1" })),
        ("IPv6", json!({ "kind": "ip", "value": "2001:db8::1" })),
        (
            "colon MAC",
            json!({ "kind": "mac", "value": "aa:bb:cc:dd:ee:ff" }),
        ),
        (
            "hyphen MAC",
            json!({ "kind": "mac", "value": "AA-BB-CC-DD-EE-FF" }),
        ),
        (
            "bare MAC",
            json!({ "kind": "mac", "value": "aabbccddeeff" }),
        ),
        (
            "dotted MAC",
            json!({ "kind": "mac", "value": "aabb.ccdd.eeff" }),
        ),
        (
            "gradient channel bounds",
            json!({
                "kind": "gradient",
                "value": [
                    { "position": 0.0, "color": [0.0, 0.0, 0.0, 0.0] },
                    { "position": 1.0, "color": [1.0, 1.0, 1.0, 1.0] }
                ]
            }),
        ),
        (
            "recursive list and map",
            json!({
                "kind": "map",
                "value": {
                    "items": {
                        "kind": "list",
                        "value": [
                            { "kind": "ip", "value": "::1" },
                            { "kind": "unknown" }
                        ]
                    }
                }
            }),
        ),
        (
            "maximum duration",
            json!({ "kind": "duration", "value": u64::MAX }),
        ),
        (
            "maximum finite f32 channel",
            json!({
                "kind": "color_linear",
                "value": {
                    "r": f64::from(f32::MAX),
                    "g": 0.0,
                    "b": 0.0,
                    "a": 1.0
                }
            }),
        ),
    ] {
        assert_parity(label, value, true);
    }

    let above_i64 = serde_json::from_str::<Value>(r#"{"kind":"int","value":9223372036854775808}"#)
        .expect("above-i64 fixture should parse as JSON");
    let below_i64 = serde_json::from_str::<Value>(r#"{"kind":"int","value":-9223372036854777856}"#)
        .expect("below-i64 fixture should parse as JSON");
    let above_u64 =
        serde_json::from_str::<Value>(r#"{"kind":"duration","value":18446744073709551616}"#)
            .expect("above-u64 fixture should parse as JSON");
    for (label, value) in [
        ("above i64", above_i64),
        ("below i64", below_i64),
        (
            "invalid IPv4",
            json!({ "kind": "ip", "value": "999.1.2.3" }),
        ),
        ("invalid IPv6", json!({ "kind": "ip", "value": "2001:::1" })),
        (
            "mixed MAC separators",
            json!({ "kind": "mac", "value": "aa:bb-cc:dd:ee:ff" }),
        ),
        (
            "short MAC",
            json!({ "kind": "mac", "value": "aa:bb:cc:dd:ee" }),
        ),
        (
            "gradient channel below zero",
            json!({
                "kind": "gradient",
                "value": [
                    { "position": 0.0, "color": [-0.001, 0.0, 0.0, 1.0] },
                    { "position": 1.0, "color": [1.0, 1.0, 1.0, 1.0] }
                ]
            }),
        ),
        (
            "gradient channel above one",
            json!({
                "kind": "gradient",
                "value": [
                    { "position": 0.0, "color": [0.0, 0.0, 0.0, 1.0] },
                    { "position": 1.0, "color": [1.001, 1.0, 1.0, 1.0] }
                ]
            }),
        ),
        (
            "invalid nested IP",
            json!({
                "kind": "map",
                "value": { "address": { "kind": "ip", "value": "nope" } }
            }),
        ),
        (
            "channel above f32 range",
            json!({
                "kind": "color_linear",
                "value": { "r": 1.0e40, "g": 0.0, "b": 0.0, "a": 1.0 }
            }),
        ),
        ("above u64 duration", above_u64),
        (
            "unknown payload",
            json!({ "kind": "unknown", "value": null }),
        ),
        (
            "unknown tag",
            json!({ "kind": "vector3", "value": [0.0, 0.0, 0.0] }),
        ),
        (
            "unknown color payload field",
            json!({
                "kind": "color_rgb",
                "value": { "r": 1, "g": 2, "b": 3, "future": 4 }
            }),
        ),
        (
            "unknown gradient stop field",
            json!({
                "kind": "gradient",
                "value": [
                    {
                        "position": 0.0,
                        "color": [0.0, 0.0, 0.0, 1.0],
                        "future": true
                    },
                    { "position": 1.0, "color": [1.0, 1.0, 1.0, 1.0] }
                ]
            }),
        ),
    ] {
        assert_parity(label, value, false);
    }
}

#[test]
fn gradient_order_remains_a_semantic_admission_invariant() {
    let tools = build_tool_definitions();
    let adjust = tools
        .iter()
        .find(|tool| tool.name == "adjust_controls")
        .expect("adjust_controls should be registered");
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(&adjust.input_schema)
        .expect("adjust_controls schema should compile");
    let descending = json!({
        "kind": "gradient",
        "value": [
            { "position": 0.8, "color": [1.0, 0.0, 0.0, 1.0] },
            { "position": 0.2, "color": [0.0, 0.0, 1.0, 1.0] }
        ]
    });
    let input = json!({
        "zone": "primary",
        "layer": "layer-1",
        "values": { "palette": descending.clone() }
    });

    assert!(
        validator.is_valid(&input),
        "JSON Schema cannot express ordering across adjacent array items"
    );
    let error = serde_json::from_value::<ControlValue>(descending)
        .expect_err("canonical admission must reject descending gradient stops");
    assert!(error.to_string().contains("nondecreasing order"));
    assert!(
        adjust.input_schema.to_string().contains(
            "JSON Schema validates each stop shape and range; canonical value admission enforces ordering"
        ),
        "the published schema must identify the semantic ordering boundary"
    );
}

/// A deleted parameter is refused, not quietly dropped.
///
/// `additionalProperties: false` is enforced in the dispatch path
/// because nothing under `rmcp` validates a call against the schema.
/// Without that, a caller who kept sending `transition_ms` would get a
/// cut and no indication the request had been ignored, which is the
/// same silence the phantom-parameter deletion exists to end.
#[tokio::test]
async fn deleted_parameters_are_refused_rather_than_dropped() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    insert_test_effect(&state, "Aurora").await;

    for (tool, params, phantom) in [
        (
            "set_effect",
            json!({ "query": "aurora", "transition_ms": 500 }),
            "transition_ms",
        ),
        (
            "set_effect",
            json!({ "query": "aurora", "devices": ["strip-1"] }),
            "devices",
        ),
        (
            "set_color",
            json!({ "color": "#ff6ac1", "transition_ms": 300 }),
            "transition_ms",
        ),
        (
            "set_brightness",
            json!({ "brightness": 42, "device_id": "strip-1" }),
            "device_id",
        ),
        (
            "clear_zone",
            json!({ "transition_ms": 300 }),
            "transition_ms",
        ),
        ("diagnose", json!({ "checks": ["connectivity"] }), "checks"),
    ] {
        let result = execute_tool_with_state(tool, &params, state.as_ref()).await;
        let error = result.expect_err(&format!(
            "{tool} must refuse the deleted parameter '{phantom}' rather than drop it"
        ));
        assert!(
            format!("{error}").contains(phantom),
            "{tool}'s refusal should name '{phantom}': {error}"
        );
    }
}

#[tokio::test]
async fn every_tool_validates_the_root_argument_shape_before_dispatch() {
    let (state, _tmp) = isolated_state_with_tempdir();

    for tool in build_tool_definitions() {
        let error = execute_tool_with_state(&tool.name, &json!([]), &state)
            .await
            .expect_err(&format!("{} should reject array arguments", tool.name));
        assert_eq!(error.error_code(), -32602);
        match error {
            ToolError::InvalidParam { param, .. } => assert_eq!(param, "arguments"),
            other => panic!("expected invalid arguments error, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn malformed_declared_arguments_never_reach_mutating_handlers() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let current = insert_test_effect(&state, "Current").await;
    let next = insert_test_effect(&state, "Aurora").await;
    insert_test_effect(&state, "Solid Color").await;

    execute_tool_with_state(
        "set_effect",
        &json!({ "query": current.id.to_string() }),
        state.as_ref(),
    )
    .await
    .expect("baseline effect should apply");

    let (zone_id, layer_id) = {
        let manager = state.scene_manager.snapshot().await;
        let zone = manager
            .active_scene()
            .and_then(|scene| scene.primary_zone())
            .expect("baseline primary zone should exist");
        let layer = zone.layers.first().expect("baseline layer should exist");
        (zone.id.to_string(), layer.id.to_string())
    };

    let created =
        execute_tool_with_state("create_scene", &json!({ "name": "Focus" }), state.as_ref())
            .await
            .expect("activation target should be created");
    let focus_id = created["scene_id"]
        .as_str()
        .expect("created scene id should be a string");

    for (tool, params, parameter) in [
        (
            "set_effect",
            json!({ "query": next.id.to_string(), "controls": [] }),
            "controls",
        ),
        (
            "set_effect",
            json!({ "query": next.id.to_string(), "transition": { "type": 1 } }),
            "transition.type",
        ),
        (
            "set_color",
            json!({ "color": "coral", "brightness": "bright" }),
            "brightness",
        ),
        ("set_output_power", json!({ "state": false }), "state"),
        ("clear_zone", json!({ "zone": false }), "zone"),
        (
            "adjust_controls",
            json!({ "zone": zone_id, "layer": layer_id, "values": [] }),
            "values",
        ),
        (
            "adjust_controls",
            json!({ "zone": zone_id, "layer": layer_id, "clear_bindings": {} }),
            "clear_bindings",
        ),
        ("set_brightness", json!({ "brightness": -1 }), "brightness"),
        (
            "activate_scene",
            json!({ "name": focus_id, "transition_ms": "instant" }),
            "transition_ms",
        ),
        (
            "create_scene",
            json!({ "name": "Invalid description", "description": [] }),
            "description",
        ),
        (
            "create_scene",
            json!({ "name": "Invalid enabled", "enabled": "yes" }),
            "enabled",
        ),
        (
            "create_scene",
            json!({ "name": "Invalid mode", "mutation_mode": false }),
            "mutation_mode",
        ),
    ] {
        assert_schema_refusal_preserves_state(state.as_ref(), tool, params, parameter).await;
    }

    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    for (params, parameter) in [
        (
            json!({
                "device": display_id.to_string(),
                "effect_id": face.id.to_string(),
                "clear": "yes"
            }),
            "clear",
        ),
        (
            json!({
                "device": display_id.to_string(),
                "effect_id": face.id.to_string(),
                "scope": null
            }),
            "scope",
        ),
        (
            json!({
                "device": display_id.to_string(),
                "effect_id": face.id.to_string(),
                "controls": []
            }),
            "controls",
        ),
    ] {
        assert_schema_refusal_preserves_state(
            state.as_ref(),
            "set_display_face",
            params,
            parameter,
        )
        .await;
        assert!(
            state
                .domains
                .display
                .preferences()
                .read()
                .await
                .get(display_id)
                .is_none()
        );
    }
}

/// A refused call changes nothing.
#[tokio::test]
async fn a_refused_deleted_parameter_leaves_the_scene_untouched() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    insert_test_effect(&state, "Aurora").await;

    let error = execute_tool_with_state(
        "set_effect",
        &json!({ "query": "aurora", "transition_ms": 500 }),
        state.as_ref(),
    )
    .await
    .expect_err("set_effect no longer accepts a transition argument");
    assert!(
        format!("{error}").contains("transition_ms"),
        "the refusal names the parameter: {error}"
    );

    let manager = state.scene_manager.snapshot().await;
    assert!(
        manager
            .active_scene()
            .and_then(|scene| scene.primary_zone())
            .and_then(|zone| zone.effect_ids().next())
            .is_none(),
        "a refused call must not load the effect"
    );
}

#[tokio::test]
async fn stateful_set_effect_echoes_the_transition_it_actually_applied() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    insert_test_effect(&state, "Aurora").await;

    let result =
        execute_tool_with_state("set_effect", &json!({ "query": "aurora" }), state.as_ref())
            .await
            .expect("an apply with no transition should succeed");

    assert_eq!(result["transition"]["type"], "cut");
    assert!(result["zone"]["layers"].is_array());
    assert_eq!(result["output"]["applied"], true);
}

#[tokio::test]
async fn adjust_controls_resolves_the_zone_and_requires_an_id_for_unnamed_layers() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    insert_test_effect(&state, "Aurora").await;

    let applied =
        execute_tool_with_state("set_effect", &json!({ "query": "aurora" }), state.as_ref())
            .await
            .expect("set_effect should succeed");
    let zone_name = applied["zone"]["name"]
        .as_str()
        .expect("the canonical zone carries its name")
        .to_owned();
    let layer_id = applied["zone"]["layers"][0]["id"]
        .as_str()
        .expect("the canonical zone carries the real layer id")
        .to_owned();

    let unnamed_error = execute_tool_with_state(
        "adjust_controls",
        &json!({
            "zone": zone_name,
            "layer": "aurora",
            "values": { "speed": { "kind": "float", "value": 8.5 } }
        }),
        state.as_ref(),
    )
    .await
    .expect_err("an unnamed layer must not resolve through its effect name");
    assert_eq!(
        unnamed_error.details().expect("selector details")["kind"],
        "no_match"
    );

    let adjusted = execute_tool_with_state(
        "adjust_controls",
        &json!({
            "zone": zone_name,
            "layer": layer_id,
            "values": { "speed": { "kind": "float", "value": 8.5 } }
        }),
        state.as_ref(),
    )
    .await
    .expect("the canonical control patch should succeed");
    assert!(adjusted["revision"].is_number());
    assert_eq!(
        adjusted["zone"]["layers"][0]["source"]["controls"]["speed"]["value"],
        json!(8.5)
    );

    let cleared =
        execute_tool_with_state("clear_zone", &json!({ "zone": zone_name }), state.as_ref())
            .await
            .expect("a selected non-display zone should clear");
    assert_eq!(cleared["zones"][0]["layers"], json!([]));
}

#[tokio::test]
async fn stateful_set_effect_rejects_unknown_transition_fields_and_types() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    insert_test_effect(&state, "Aurora").await;

    let applied = execute_tool_with_state(
        "set_effect",
        &json!({ "query": "aurora", "transition": { "type": "cut" } }),
        state.as_ref(),
    )
    .await
    .expect("the explicit cut transition should succeed");
    assert_eq!(applied["transition"]["type"], "cut");

    for transition in [
        json!({ "type": "fade" }),
        json!({ "type": "cut", "duration_ms": 400 }),
        json!("cut"),
    ] {
        let error = execute_tool_with_state(
            "set_effect",
            &json!({ "query": "aurora", "transition": transition }),
            state.as_ref(),
        )
        .await
        .expect_err("only the closed cut transition is accepted");
        assert!(format!("{error}").contains("transition"));
    }
}

#[tokio::test]
async fn stateful_set_effect_conflicts_when_snapshot_scene_is_active() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    insert_test_effect(&state, "Aurora").await;

    let create_result = execute_tool_with_state(
        "create_scene",
        &json!({
            "name": "Focus",
            "mutation_mode": "snapshot"
        }),
        state.as_ref(),
    )
    .await
    .expect("scene creation should succeed");
    assert_eq!(create_result["mutation_mode"], "snapshot");

    execute_tool_with_state(
        "activate_scene",
        &json!({
            "name": "Focus"
        }),
        state.as_ref(),
    )
    .await
    .expect("scene activation should succeed");

    let error = execute_tool_with_state(
        "set_effect",
        &json!({
            "query": "aurora",
        }),
        state.as_ref(),
    )
    .await
    .expect_err("snapshot scenes should reject MCP effect mutation");

    match error {
        ToolError::Conflict(message) => {
            assert!(message.contains("snapshot mode"));
            assert_eq!(ToolError::Conflict(message).error_code(), -32000);
        }
        other => panic!("expected snapshot conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn stateful_set_effect_and_clear_zone_sync_scene_runtime_and_events() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let effect = insert_test_effect(&state, "Aurora").await;

    let mut start_events = state.event_bus.subscribe_all();
    let apply_result = execute_tool_with_state(
        "set_effect",
        &json!({
            "query": "aurora",
            "controls": {
                "speed": 7.5
            }
        }),
        state.as_ref(),
    )
    .await
    .expect("set_effect should succeed");
    assert_eq!(apply_result["transition"]["type"], "cut");
    assert_eq!(apply_result["output"]["applied"], true);
    assert_eq!(
        apply_result["zone"]["layers"][0]["source"]["effect_id"],
        effect.id.to_string()
    );

    let (scene_id, active_zone) = {
        let manager = state.scene_manager.snapshot().await;
        (
            manager
                .active_scene_id()
                .copied()
                .expect("default scene should stay active"),
            manager
                .active_scene()
                .and_then(|scene| scene.primary_zone())
                .cloned()
                .expect("primary zone should exist after MCP set_effect"),
        )
    };
    assert_eq!(active_zone.effect_ids().next(), Some(effect.id));
    assert_eq!(
        effect_controls(&active_zone).and_then(|controls| controls.get("speed")),
        Some(&ControlValue::Float(7.5))
    );

    let active_snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(active_snapshot.default_scene_zones.len(), 1);
    assert_eq!(
        active_snapshot.default_scene_zones[0].effect_ids().next(),
        Some(effect.id)
    );
    assert_eq!(
        effect_controls(&active_snapshot.default_scene_zones[0])
            .and_then(|controls| controls.get("speed")),
        Some(&ControlValue::Float(7.5))
    );

    let status = execute_tool_with_state("get_status", &json!({}), state.as_ref())
        .await
        .expect("get_status should succeed");
    assert_eq!(status["effect"]["id"], effect.id.to_string());
    assert_eq!(status["effect"]["name"], effect.name);

    let resource_state = read_resource_with_state("hypercolor://state", state.as_ref())
        .await
        .expect("state resource should exist");
    assert_eq!(resource_state["effect"]["id"], effect.id.to_string());
    assert_eq!(resource_state["effect"]["name"], effect.name);

    let mut saw_started_event = false;
    let mut saw_zone_event = false;
    while let Ok(timestamped) = start_events.try_recv() {
        match timestamped.event {
            HypercolorEvent::EffectStarted {
                effect: started,
                trigger,
                ..
            } => {
                assert_eq!(started.id, effect.id.to_string());
                assert_eq!(trigger, ChangeTrigger::Mcp);
                saw_started_event = true;
            }
            HypercolorEvent::ZoneChanged {
                scene_id: event_scene_id,
                kind,
                role,
                ..
            } => {
                assert_eq!(event_scene_id, scene_id);
                assert_eq!(role, hypercolor_types::scene::ZoneRole::Primary);
                assert_eq!(kind, ZoneChangeKind::Updated);
                saw_zone_event = true;
            }
            _ => {}
        }
    }
    assert!(saw_started_event, "expected MCP effect-start event");
    assert!(saw_zone_event, "expected MCP render-zone event");

    let mut stop_events = state.event_bus.subscribe_all();
    let clear_result = execute_tool_with_state("clear_zone", &json!({}), state.as_ref())
        .await
        .expect("clear_zone should succeed");
    assert_eq!(clear_result["id"], scene_id.to_string());
    assert!(clear_result["revision"].is_number());

    let stopped_snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(stopped_snapshot.default_scene_zones.len(), 1);
    assert!(stopped_snapshot.default_scene_zones[0].layers.is_empty());

    let cleared_zone = {
        let manager = state.scene_manager.snapshot().await;
        manager
            .active_scene()
            .and_then(|scene| scene.primary_zone())
            .cloned()
            .expect("primary zone should remain present after stop")
    };
    assert!(cleared_zone.layers.is_empty());

    let mut saw_stopped_event = false;
    let mut saw_updated_zone = false;
    while let Ok(timestamped) = stop_events.try_recv() {
        match timestamped.event {
            HypercolorEvent::EffectStopped {
                effect: stopped,
                reason,
                ..
            } => {
                assert_eq!(stopped.id, effect.id.to_string());
                assert_eq!(reason, EffectStopReason::Stopped);
                saw_stopped_event = true;
            }
            HypercolorEvent::ZoneChanged { kind, role, .. } => {
                assert_eq!(role, hypercolor_types::scene::ZoneRole::Primary);
                assert_eq!(kind, ZoneChangeKind::Updated);
                saw_updated_zone = true;
            }
            _ => {}
        }
    }
    assert!(saw_stopped_event, "expected MCP effect-stop event");
    assert!(saw_updated_zone, "expected MCP zone-clear event");
}

#[tokio::test]
async fn stateful_set_effect_preserves_primary_assignment_when_custom_zones_exist() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let existing = insert_test_effect(&state, "Current").await;
    let next = insert_test_effect(&state, "Aurora").await;
    let expected_layout = seed_multi_zone_primary_assignment(&state, &existing).await;

    execute_tool_with_state(
        "set_effect",
        &json!({
            "query": "aurora",
            "controls": {
                "speed": 7.5
            }
        }),
        state.as_ref(),
    )
    .await
    .expect("set_effect should succeed");

    let active_zone = {
        let manager = state.scene_manager.snapshot().await;
        manager
            .active_scene()
            .and_then(|scene| scene.primary_zone())
            .cloned()
            .expect("primary zone should exist after MCP set_effect")
    };
    assert_eq!(active_zone.effect_ids().next(), Some(next.id));
    assert_eq!(active_zone.layout, expected_layout);
}

#[tokio::test]
async fn stateful_set_color_syncs_scene_runtime_state() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let solid_effect = insert_test_effect(&state, "Solid Color").await;

    let result = execute_tool_with_state(
        "set_color",
        &json!({
            "color": "#ff6ac1",
            "brightness": 50.0
        }),
        state.as_ref(),
    )
    .await
    .expect("set_color should succeed");
    assert_eq!(result["transition"]["type"], "cut");
    assert_eq!(result["output"]["applied"], true);
    assert_eq!(
        result["zone"]["layers"][0]["source"]["effect_id"],
        solid_effect.id.to_string()
    );

    let snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(snapshot.default_scene_zones.len(), 1);
    assert_eq!(
        snapshot.default_scene_zones[0].effect_ids().next(),
        Some(solid_effect.id)
    );
    assert_eq!(
        effect_controls(&snapshot.default_scene_zones[0])
            .and_then(|controls| controls.get("brightness")),
        Some(&ControlValue::Float(0.5))
    );
    match effect_controls(&snapshot.default_scene_zones[0])
        .and_then(|controls| controls.get("color"))
    {
        Some(ControlValue::ColorLinear(color)) => {
            assert_eq!(
                (color.r, color.g, color.b, color.a),
                (1.0, 106.0 / 255.0, 193.0 / 255.0, 1.0)
            );
        }
        other => panic!("expected RGBA control value, got {other:?}"),
    }
}

#[tokio::test]
async fn stateful_set_color_preserves_primary_assignment_when_custom_zones_exist() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let existing = insert_test_effect(&state, "Current").await;
    let solid_effect = insert_test_effect(&state, "Solid Color").await;
    let expected_layout = seed_multi_zone_primary_assignment(&state, &existing).await;

    execute_tool_with_state(
        "set_color",
        &json!({
            "color": "#ff6ac1",
            "brightness": 50
        }),
        state.as_ref(),
    )
    .await
    .expect("set_color should succeed");

    let active_zone = {
        let manager = state.scene_manager.snapshot().await;
        manager
            .active_scene()
            .and_then(|scene| scene.primary_zone())
            .cloned()
            .expect("primary zone should exist after MCP set_color")
    };
    assert_eq!(active_zone.effect_ids().next(), Some(solid_effect.id));
    assert_eq!(active_zone.layout, expected_layout);
}

#[tokio::test]
async fn read_only_tool_results_match_their_declared_schemas() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    insert_test_effect(&state, "Aurora").await;

    for (name, params) in [
        ("list_effects", json!({})),
        ("get_audio_state", json!({})),
        ("get_sensor_data", json!({})),
        ("get_layout", json!({})),
    ] {
        execute_tool_with_state(name, &params, state.as_ref())
            .await
            .unwrap_or_else(|error| panic!("{name} should match its output schema: {error}"));
    }
}

#[test]
fn tool_definitions_have_valid_schemas() {
    let tools = build_tool_definitions();
    assert_eq!(tools.len(), 17);
    assert!(
        tools
            .iter()
            .all(|tool| tool.input_schema["type"] == "object")
    );
    for tool in &tools {
        assert!(tool.output_schema.is_object(), "{} output", tool.name);
        assert!(
            jsonschema::validator_for(&tool.output_schema).is_ok(),
            "{} must publish a valid, self-contained output schema",
            tool.name
        );
        assert_eq!(
            tool.output_schema["additionalProperties"],
            json!(false),
            "{} must close its typed output shape",
            tool.name
        );
        assert!(
            tool.output_schema["properties"].is_object(),
            "{} must publish field-level output properties",
            tool.name
        );
        assert!(
            !tool
                .output_schema
                .to_string()
                .contains("intentionally broad"),
            "{} still advertises the deleted fallback schema",
            tool.name
        );
    }
    assert!(tools.iter().any(|tool| tool.name == "set_display_face"));
    assert!(tools.iter().any(|tool| tool.name == "clear_zone"));
    assert!(tools.iter().any(|tool| tool.name == "adjust_controls"));
    assert!(tools.iter().all(|tool| tool.name != "stop_effect"));
    let diagnose = tools
        .iter()
        .find(|tool| tool.name == "diagnose")
        .expect("diagnose tool should be registered");
    assert_eq!(
        diagnose.input_schema,
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    );
    assert!(
        diagnose.output_schema["properties"]
            .get("overall_status")
            .is_none()
    );
}

/// Every tool's top-level argument set is closed.
///
/// The dispatch gate only refuses undeclared arguments for tools whose
/// schema says `additionalProperties: false`, so a tool without the
/// marker silently drops whatever it is handed. That is the same
/// decoration-instead-of-enforcement failure the phantom deletions
/// exist to end, one layer up, and it is why this sweeps all of them
/// rather than naming the ones that were fixed: tool eighteen cannot
/// ship open.
///
/// Nested objects are deliberately exempt. `set_effect.controls` and
/// the display-face payload carry per-effect keys the schema cannot
/// enumerate, so they stay open on purpose.
#[test]
fn every_tool_closes_its_top_level_argument_set() {
    for tool in build_tool_definitions() {
        assert_eq!(
            tool.input_schema["additionalProperties"],
            json!(false),
            "{} must declare additionalProperties: false, or the dispatch \
             gate will silently drop arguments it does not declare",
            tool.name
        );
    }
}

/// The gate refuses an undeclared argument on every tool.
///
/// Closing the schemas and enforcing them are two different things, so
/// this drives the real dispatch path rather than reading the schema.
#[tokio::test]
async fn undeclared_arguments_are_refused_on_every_tool() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);

    for tool in build_tool_definitions() {
        let mut params = json!({ "hypercolor_not_a_real_argument": 1 });
        // Satisfy required arguments so the refusal is provably about
        // the undeclared key rather than a missing one.
        if let Some(required) = tool.input_schema["required"].as_array() {
            let object = params.as_object_mut().expect("params is an object");
            for name in required.iter().filter_map(Value::as_str) {
                object.insert(name.to_owned(), json!("placeholder"));
            }
        }

        let error = execute_tool_with_state(&tool.name, &params, state.as_ref())
            .await
            .expect_err(&format!(
                "{} should refuse an undeclared argument",
                tool.name
            ));
        assert!(
            format!("{error}").contains("hypercolor_not_a_real_argument"),
            "{}'s refusal should name the undeclared argument: {error}",
            tool.name
        );
    }
}

/// A parameter exists only when its behavior does (Spec 78 §6.1).
///
/// Each entry below was advertised in a tool's schema while the handler
/// either never read it or read it only to echo it back. They are named
/// here rather than described so that reintroducing one fails loudly.
#[test]
fn deleted_phantom_parameters_stay_deleted() {
    let phantoms: &[(&str, &str)] = &[
        ("set_effect", "devices"),
        ("set_effect", "transition_ms"),
        ("set_color", "devices"),
        ("set_brightness", "device_id"),
        ("set_brightness", "transition_ms"),
        ("diagnose", "device_id"),
        ("diagnose", "checks"),
        ("clear_zone", "transition_ms"),
        ("create_scene", "transition_ms"),
        ("create_scene", "profile_id"),
        ("create_scene", "trigger"),
    ];

    let tools = build_tool_definitions();
    for (tool_name, param) in phantoms {
        let tool = tools
            .iter()
            .find(|tool| tool.name == *tool_name)
            .unwrap_or_else(|| panic!("{tool_name} should be registered"));
        let mut node = &tool.input_schema["properties"];
        for (depth, segment) in param.split('.').enumerate() {
            if depth > 0 {
                node = &node["properties"];
            }
            let Some(next) = node.get(segment) else {
                node = &Value::Null;
                break;
            };
            node = next;
        }
        assert!(
            node.is_null(),
            "{tool_name} must not advertise the phantom parameter {param}"
        );
    }
}

/// The category filter's advertised vocabulary comes from the type.
#[test]
fn list_effects_advertises_the_real_effect_categories() {
    let tools = build_tool_definitions();
    let list_effects = tools
        .iter()
        .find(|tool| tool.name == "list_effects")
        .expect("list_effects should be registered");

    let advertised = list_effects.input_schema["properties"]["category"]["enum"]
        .as_array()
        .expect("the category filter should advertise an enum")
        .iter()
        .map(|value| value.as_str().expect("categories are strings").to_owned())
        .collect::<Vec<_>>();

    assert_eq!(advertised, EffectCategory::VARIANTS);
    for fabricated in ["reactive", "gaming", "productivity"] {
        assert!(
            !advertised.iter().any(|value| value == fabricated),
            "'{fabricated}' is not an EffectCategory and must not be advertised"
        );
    }
}

/// `destructive` is a per-tool fact, not a hardcoded false.
#[test]
fn tool_annotations_report_what_each_tool_actually_does() {
    let tools = build_tool_definitions();
    let annotation = |name: &str| {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} should be registered"));
        (tool.read_only, tool.destructive, tool.idempotent)
    };

    // Tools that discard state the caller cannot recover.
    for name in [
        "clear_zone",
        "set_effect",
        "set_color",
        "activate_scene",
        "set_display_face",
    ] {
        let expected_idempotent = !matches!(name, "set_effect" | "set_color" | "set_display_face");
        assert_eq!(
            annotation(name),
            (false, true, expected_idempotent),
            "{name}"
        );
    }

    // Reversible value writes and pure creations.
    for name in [
        "set_brightness",
        "set_output_power",
        "adjust_controls",
        "create_scene",
    ] {
        let expected_idempotent = name != "create_scene";
        assert_eq!(
            annotation(name),
            (false, false, expected_idempotent),
            "{name}"
        );
    }

    // Read-only tools never claim to destroy anything.
    for tool in tools.iter().filter(|tool| tool.read_only) {
        assert!(!tool.destructive, "{} is read-only", tool.name);
    }
}

#[tokio::test]
async fn set_color_tool_rejects_missing_color() {
    let (state, _tempdir) = isolated_state_with_tempdir();

    let error = execute_tool_with_state("set_color", &json!({}), &state)
        .await
        .expect_err("missing color should return an error");
    assert!(matches!(error, ToolError::MissingParam(_)));
}

#[test]
fn fuzzy_color_shorthand_hex_requires_an_explicit_hash() {
    let word = mcp::fuzzy::resolve_color("bed").expect("a hex-digit word reaches the name matcher");
    assert_ne!(
        word.hex, "#bbeedd",
        "hashless shorthand must not shadow named colors"
    );

    let shorthand = mcp::fuzzy::resolve_color("#bed").expect("hash-prefixed shorthand is hex");
    assert_eq!(shorthand.hex, "#bbeedd");
    assert_eq!((shorthand.r, shorthand.g, shorthand.b), (0xbb, 0xee, 0xdd));

    let hashless = mcp::fuzzy::resolve_color("ff8800").expect("hashless six-digit hex is hex");
    assert_eq!(hashless.hex, "#ff8800");
    assert_eq!((hashless.r, hashless.g, hashless.b), (0xff, 0x88, 0x00));
}

#[tokio::test]
async fn set_output_power_tool_validates_desired_state() {
    let (state, _tempdir) = isolated_state_with_tempdir();

    let error = execute_tool_with_state("set_output_power", &json!({ "state": "off" }), &state)
        .await
        .expect_err("unknown output state should be rejected");
    assert!(matches!(error, ToolError::InvalidParam { .. }));
}

#[tokio::test]
async fn stateful_set_output_power_is_reversible_and_idempotent() {
    let (state, _tempdir) = isolated_state_with_tempdir();

    let state = Arc::new(state);

    let paused = execute_tool_with_state(
        "set_output_power",
        &json!({ "state": "paused" }),
        state.as_ref(),
    )
    .await
    .expect("pause should succeed");
    assert_eq!(paused["state"], "paused");
    assert!(state.output_power.snapshot().manually_paused());

    let paused_again = execute_tool_with_state(
        "set_output_power",
        &json!({ "state": "paused" }),
        state.as_ref(),
    )
    .await
    .expect("repeated pause should succeed");
    assert_eq!(paused_again["state"], "paused");

    let running = execute_tool_with_state(
        "set_output_power",
        &json!({ "state": "running" }),
        state.as_ref(),
    )
    .await
    .expect("resume should succeed");
    assert_eq!(running["state"], "running");
    assert!(!state.output_power.snapshot().sleeping());
}

/// `set_brightness` is a projection of the output service, so the tool
/// moves the same live state `GET /output` reports and persists the
/// same store the REST route does.
#[tokio::test]
async fn set_brightness_tool_projects_the_output_service() {
    let (state, _tmp) = isolated_state_with_tempdir();

    let response =
        execute_tool_with_state("set_brightness", &json!({ "brightness": 35.0 }), &state)
            .await
            .expect("brightness should be accepted");
    assert_eq!(response["brightness"], 35);
    assert_eq!(response["previous_brightness"], 100);
    assert!((state.output_power.global_brightness() - 0.35).abs() < 1e-6);
    assert_eq!(
        DeviceSettingsStore::load(&state.state_dir.join("device-settings.json"))
            .expect("device settings should reload")
            .global_brightness(),
        0.35,
        "the tool must persist through the same store the REST route writes"
    );

    let error = execute_tool_with_state("set_brightness", &json!({ "brightness": 150 }), &state)
        .await
        .expect_err("out-of-range brightness should be rejected");
    assert!(matches!(error, ToolError::InvalidParam { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_brightness_tools_report_serialized_predecessors() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let first_state = Arc::clone(&state);
    let second_state = Arc::clone(&state);

    let first = tokio::spawn(async move {
        execute_tool_with_state(
            "set_brightness",
            &json!({ "brightness": 25 }),
            first_state.as_ref(),
        )
        .await
        .expect("first brightness should succeed")
    });
    let second = tokio::spawn(async move {
        execute_tool_with_state(
            "set_brightness",
            &json!({ "brightness": 75 }),
            second_state.as_ref(),
        )
        .await
        .expect("second brightness should succeed")
    });
    let first = first.await.expect("first brightness task should join");
    let second = second.await.expect("second brightness task should join");
    let transitions = [
        (
            first["previous_brightness"]
                .as_u64()
                .expect("first predecessor should be numeric"),
            first["brightness"]
                .as_u64()
                .expect("first brightness should be numeric"),
        ),
        (
            second["previous_brightness"]
                .as_u64()
                .expect("second predecessor should be numeric"),
            second["brightness"]
                .as_u64()
                .expect("second brightness should be numeric"),
        ),
    ];

    assert!(
        transitions.contains(&(100, 25)) && transitions.contains(&(25, 75))
            || transitions.contains(&(100, 75)) && transitions.contains(&(75, 25))
    );
}

#[test]
fn resource_definitions_match_live_uri_validation() {
    let resources = build_resource_definitions();
    assert_eq!(resources.len(), 5);
    assert!(
        resources
            .iter()
            .all(|resource| is_valid_resource_uri(&resource.uri))
    );
    assert!(is_valid_resource_uri("hypercolor://state"));
    assert!(is_valid_resource_uri("hypercolor://scenes"));
    assert!(!is_valid_resource_uri("hypercolor://profiles"));
}

#[tokio::test]
async fn mcp_device_inventory_surfaces_are_exact_and_filterable() {
    let (state, _tempdir) = isolated_state_with_tempdir();

    let state = Arc::new(state);
    let device_id = insert_test_display_device(&state, "Case Display").await;

    let resource = read_resource_with_state("hypercolor://devices", state.as_ref())
        .await
        .expect("devices resource should exist");
    let tool = execute_tool_with_state("get_devices", &json!({}), state.as_ref())
        .await
        .expect("get_devices should succeed");
    assert_eq!(tool, resource, "tool and resource payloads must be exact");
    let resource_device = &resource["devices"][0];
    assert_eq!(resource_device["id"], device_id.to_string());
    assert_eq!(resource_device["origin"]["driver_id"], "wled");
    assert_eq!(resource_device["origin"]["backend_id"], "usb");
    assert_eq!(resource_device["origin"]["transport"], "usb");
    assert_eq!(resource_device["transport"], "usb");
    assert!(resource_device.get("connection_type").is_none());
    assert_eq!(resource_device["presentation"]["label"], "WLED");

    let filtered = execute_tool_with_state(
        "get_devices",
        &json!({
            "driver_id": "wled",
            "backend_id": "usb",
            "status": "disconnected"
        }),
        state.as_ref(),
    )
    .await
    .expect("get_devices should support driver and backend filters");
    assert_eq!(filtered["summary"]["total"], 1);
    assert_eq!(filtered["devices"][0]["origin"]["driver_id"], "wled");
    assert_eq!(filtered["devices"][0]["transport"], "usb");

    let filtered_out = execute_tool_with_state(
        "get_devices",
        &json!({
            "driver_id": "hue",
            "backend_id": "usb"
        }),
        state.as_ref(),
    )
    .await
    .expect("get_devices should handle unmatched filters");
    assert_eq!(filtered_out["summary"]["total"], 0);
}

#[test]
fn prompt_definitions_and_messages_are_valid() {
    let prompts = build_prompt_definitions();
    assert_eq!(prompts.len(), 3);
    assert!(is_valid_prompt("mood_lighting"));
    let messages = get_prompt_messages(
        "mood_lighting",
        &json!({ "mood": "cozy evening", "audio_reactive": "no" }),
    )
    .expect("prompt should build messages");
    assert!(messages["messages"].is_array());
    let mood = messages.to_string();
    assert!(mood.contains("Exclude catalog effects marked audio_reactive"));
    assert!(mood.contains("Call set_effect exactly once"));
    assert!(mood.contains("adjust_controls"));
    assert!(!mood.contains("top 2-3"));

    let troubleshoot = get_prompt_messages("troubleshoot", &json!({ "issue": "offline" }))
        .expect("troubleshoot prompt should build messages")
        .to_string();
    assert!(troubleshoot.contains("canonical safe diagnostic report"));
    assert!(!troubleshoot.contains("reconnecting a device"));
    assert!(!troubleshoot.contains("adjusting settings"));

    let automation = get_prompt_messages("setup_automation", &json!({}))
        .expect("automation prompt should build messages");
    let encoded = automation.to_string();
    assert!(encoded.contains("hypercolor://scenes"));
    assert!(encoded.contains("hypercolor://effects"));
    assert!(!encoded.contains("hypercolor://profiles"));
    assert!(encoded.contains("does not schedule or trigger scenes"));
    assert!(encoded.contains("create_scene"));
    assert!(encoded.contains("Activate that scene"));
    assert!(encoded.contains("call set_effect once"));
    assert!(encoded.contains("adjust_controls"));
    assert!(encoded.contains("does not capture the current output"));
}

#[tokio::test]
async fn display_face_assignment_agrees_across_rest_and_mcp() {
    let (rest_state, _rest_tmp) = isolated_state_with_tempdir();
    let rest_state = Arc::new(rest_state);
    let rest_display = insert_test_display_device(&rest_state, "Pump LCD").await;
    let rest_face = insert_test_display_face_effect(&rest_state, "System Monitor").await;

    let (client, base) = spawn_router(api::build_router(Arc::clone(&rest_state), None)).await;
    let response = client
        .put(format!("{base}/api/v1/displays/{rest_display}/face"))
        .json(&json!({
            "effect_id": rest_face.id.to_string(),
            "scope": "default",
        }))
        .send()
        .await
        .expect("REST default-face assignment should send");
    assert_eq!(
        response.status().as_u16(),
        200,
        "REST default-face assignment should succeed"
    );
    let rest_body: Value = response
        .json()
        .await
        .expect("REST face response should be JSON");
    let rest_face_payload = &rest_body["data"];

    let (mcp_state, _mcp_tmp) = isolated_state_with_tempdir();
    let mcp_state = Arc::new(mcp_state);
    let mcp_display = insert_test_display_device(&mcp_state, "Pump LCD").await;
    let mcp_face = insert_test_display_face_effect(&mcp_state, "System Monitor").await;
    let mcp_face_payload = execute_tool_with_state(
        "set_display_face",
        &json!({
            "device": mcp_display.to_string(),
            "effect_id": mcp_face.id.to_string(),
        }),
        mcp_state.as_ref(),
    )
    .await
    .expect("MCP default-face assignment should succeed");

    let rest_preference = rest_state
        .domains
        .display
        .preferences()
        .read()
        .await
        .get(rest_display)
        .cloned()
        .expect("REST assignment should store a preference");
    let mcp_preference = mcp_state
        .domains
        .display
        .preferences()
        .read()
        .await
        .get(mcp_display)
        .cloned()
        .expect("MCP assignment should store a preference");
    assert_eq!(rest_preference.blend_mode, mcp_preference.blend_mode);
    assert!((rest_preference.opacity - mcp_preference.opacity).abs() <= f32::EPSILON);
    assert_eq!(rest_preference.controls, mcp_preference.controls);
    assert_eq!(rest_preference.effect_id, rest_face.id);
    assert_eq!(mcp_preference.effect_id, mcp_face.id);

    assert_eq!(
        rest_face_payload["live_scope"],
        mcp_face_payload["live_scope"]
    );
    for field in ["role", "brightness", "enabled"] {
        assert_eq!(
            rest_face_payload["zone"][field], mcp_face_payload["zone"][field],
            "overlay zone field {field} diverged between REST and MCP"
        );
    }
    for field in ["blend_mode", "opacity"] {
        assert_eq!(
            rest_face_payload["zone"]["display_target"][field],
            mcp_face_payload["zone"]["display_target"][field],
            "overlay composition field {field} diverged between REST and MCP"
        );
    }

    let rest_zones = rest_state
        .scene_manager
        .snapshot()
        .await
        .resolved_zones()
        .iter()
        .filter(|zone| zone.has_effect(rest_face.id))
        .count();
    let mcp_zones = mcp_state
        .scene_manager
        .snapshot()
        .await
        .resolved_zones()
        .iter()
        .filter(|zone| zone.has_effect(mcp_face.id))
        .count();
    assert_eq!(
        rest_zones, mcp_zones,
        "both transports should materialize the same overlay zone count"
    );
    assert_eq!(rest_zones, 1);
}

#[tokio::test]
async fn stateful_display_face_tool_defaults_to_the_persistent_scope() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;

    let assign_result = execute_tool_with_state(
        "set_display_face",
        &json!({
            "device": display_id.to_string(),
            "effect_id": face.id.to_string(),
        }),
        state.as_ref(),
    )
    .await
    .expect("default-scope face assignment should succeed");
    assert_eq!(assign_result["scope"], "default");
    assert_eq!(assign_result["live_scope"], "default");
    assert_eq!(assign_result["effect"]["id"], face.id.to_string());
    assert_eq!(
        assign_result["zone"]["display_target"]["device_id"],
        display_id.to_string()
    );

    // The preference persists and the overlay reaches the render zones.
    assert!(
        state
            .domains
            .display
            .preferences()
            .read()
            .await
            .get(display_id)
            .is_some()
    );
    assert!(
        state
            .scene_manager
            .snapshot()
            .await
            .resolved_zones()
            .iter()
            .any(|zone| zone.has_effect(face.id))
    );

    let clear_result = execute_tool_with_state(
        "set_display_face",
        &json!({
            "device": display_id.to_string(),
            "clear": true
        }),
        state.as_ref(),
    )
    .await
    .expect("default-scope face clear should succeed");
    assert_eq!(clear_result["scope"], "default");
    assert_eq!(clear_result["cleared"], true);
    assert!(clear_result["live_scope"].is_null());
    assert!(
        state
            .domains
            .display
            .preferences()
            .read()
            .await
            .get(display_id)
            .is_none()
    );
}
