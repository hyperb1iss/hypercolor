//! Integration tests for the MCP HTTP surface and its reusable domain helpers.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use hypercolor_core::config::ConfigManager;
use hypercolor_core::input::{
    InputData, InputSource, SourceIssue, SourceKind, SourceStatusHandle, SourceStatusReporter,
};
use hypercolor_core::scene::OutputPlacement;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::mcp;
use hypercolor_daemon::mcp::prompts::{
    build_prompt_definitions, get_prompt_messages, is_valid_prompt,
};
use hypercolor_daemon::mcp::resources::{
    build_resource_definitions, is_valid_resource_uri, read_resource, read_resource_with_state,
};
use hypercolor_daemon::mcp::tools::{ToolError, build_tool_definitions, execute_tool_with_state};
use hypercolor_daemon::runtime_state;
use hypercolor_daemon::scene_store::SceneStore;
use hypercolor_types::config::{CURRENT_SCHEMA_VERSION, McpConfig};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource,
};
use hypercolor_types::event::{
    ChangeTrigger, EffectStopReason, HypercolorEvent, SceneChangeReason, ZoneChangeKind,
};
use hypercolor_types::scene::SceneId;
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
static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = TempDir::new().expect("create temp dir");
    let data_dir = tempdir.path().join("data");
    fs::create_dir_all(&data_dir).expect("create temp data dir");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    (state, tempdir)
}

fn fresh_app_state() -> AppState {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    AppState::new()
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

#[tokio::test]
async fn diagnose_exposes_capacity_and_delivered_fps_separately() {
    let state = fresh_app_state();
    let result = execute_tool_with_state("diagnose", &json!({}), &state)
        .await
        .expect("diagnose should succeed");

    assert_eq!(result["metrics"]["fps"], result["metrics"]["capacity_fps"]);
    assert!(result["metrics"]["capacity_fps"].is_number());
    assert!(result["metrics"]["delivered_fps"].is_number());
}

#[tokio::test]
async fn diagnose_reports_demanded_input_failure_as_unhealthy() {
    let state = fresh_app_state();
    {
        let mut manager = state.input_manager.lock().await;
        manager.add_source(Box::new(FailedInputSource::new()));
        manager.start_all().expect("test input graph should start");
    }

    let result = execute_tool_with_state("diagnose", &json!({}), &state)
        .await
        .expect("diagnose should succeed");

    assert_eq!(result["overall_status"], "unhealthy");
    assert!(result["findings"].as_array().is_some_and(|findings| {
        findings.iter().any(|finding| {
            finding["severity"] == "error"
                && finding["source_id"] == "failed_mcp_audio"
                && finding["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("capture_worker_exited"))
        })
    }));
}

#[tokio::test]
async fn mcp_input_status_surfaces_do_not_wait_for_input_manager() {
    let state = fresh_app_state();
    state
        .input_manager
        .lock()
        .await
        .start_all()
        .expect("browser input source should start");
    let manager_guard = state.input_manager.lock().await;

    let status = tokio::time::timeout(
        Duration::from_secs(1),
        execute_tool_with_state("get_status", &json!({}), &state),
    )
    .await
    .expect("get_status must not wait for the input manager")
    .expect("get_status should succeed");
    assert_eq!(status["inputs"]["sources"][0]["source_id"], "browser_input");
    assert!(status["inputs"]["source_graph_generation"].is_number());

    let resource = tokio::time::timeout(
        Duration::from_secs(1),
        read_resource_with_state("hypercolor://state", &state),
    )
    .await
    .expect("state resource must not wait for the input manager")
    .expect("state resource should exist");
    assert_eq!(
        resource["inputs"]["input"]["sources"][0]["source_id"],
        "browser_input"
    );

    let diagnose = tokio::time::timeout(
        Duration::from_secs(1),
        execute_tool_with_state("diagnose", &json!({}), &state),
    )
    .await
    .expect("diagnose must not wait for the input manager")
    .expect("diagnose should succeed");
    drop(manager_guard);

    assert_eq!(
        diagnose["metrics"]["inputs"]["sources"][0]["source_id"],
        "browser_input"
    );
    assert!(diagnose["findings"].as_array().is_some_and(|findings| {
        findings
            .iter()
            .all(|finding| finding["source_id"] != "browser_input")
    }));
}

#[tokio::test]
async fn mcp_status_surfaces_report_effective_session_pause() {
    let state = fresh_app_state();
    state
        .power_state
        .send_modify(|power| power.session_sleeping = true);

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

    state.power_state.send_modify(|power| {
        power.output_override = hypercolor_daemon::session::OutputOverride::Stopped;
    });

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
        zones: vec![ZoneInfo {
            name: "LCD".to_owned(),
            led_count: 320 * 320,
            topology: DeviceTopologyHint::Display {
                width: 320,
                height: 320,
                circular: true,
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
    let mut registry = state.effect_registry.write().await;
    let _ = registry.register(entry);
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
    let mut registry = state.effect_registry.write().await;
    let _ = registry.register(entry);
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
        spaces: None,
        version: 1,
    }
}

async fn seed_multi_zone_primary_assignment(
    state: &Arc<AppState>,
    metadata: &EffectMetadata,
) -> SpatialLayout {
    let primary_layout = test_layout("primary-layout", vec![test_device_zone("primary-zone")]);
    let custom_zone = test_device_zone("custom-zone");
    let mut manager = state.scene_manager.write().await;
    manager
        .upsert_primary_group(metadata, HashMap::new(), None, primary_layout.clone())
        .expect("primary group should be seeded");
    let custom_id = manager
        .create_render_group(&SceneId::DEFAULT, "Custom".to_owned(), None, (320, 200))
        .expect("custom group should be created");
    manager
        .assign_device_zone(
            &SceneId::DEFAULT,
            custom_id,
            custom_zone,
            OutputPlacement::AutoGrid,
        )
        .expect("custom group should claim a zone");
    primary_layout
}

fn scenes_path(state: &AppState) -> PathBuf {
    state
        .runtime_state_path
        .parent()
        .expect("runtime-state.json should live under a data dir")
        .join("scenes.json")
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
    let state = Arc::new(fresh_app_state());
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
    let state = Arc::new(fresh_app_state());
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
    assert_eq!(tools.len(), 16);
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
}

#[tokio::test]
async fn mcp_http_resources_and_prompts_roundtrip() {
    let state = Arc::new(fresh_app_state());
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
    let state = Arc::new(fresh_app_state());
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
    let mut state = fresh_app_state();
    state.config_manager = Some(manager);

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

    let store = SceneStore::load(&scenes_path(state.as_ref())).expect("scene store should load");
    assert_eq!(store.len(), 1);
    let stored_scene = store.list().next().expect("named scene should persist");
    assert!(stored_scene.metadata.is_empty());

    let mut events = state.event_bus.subscribe_all();
    let activate_result = execute_tool_with_state(
        "activate_scene",
        &json!({
            "name": "Focus",
            "transition_ms": 250
        }),
        state.as_ref(),
    )
    .await
    .expect("scene activation should succeed");
    assert_eq!(activate_result["activated"], true);
    assert_eq!(activate_result["scene"]["id"], scene_id);

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
async fn stateful_display_face_tool_assigns_and_clears_face_groups() {
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
    assert_eq!(assign_result["zone"]["layout"]["canvas_width"], 320);
    assert_eq!(assign_result["zone"]["controls"]["title"]["text"], "CPU");

    let assign_snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(
        assign_snapshot.active_scene_id,
        Some(SceneId::DEFAULT.to_string())
    );
    assert_eq!(assign_snapshot.default_scene_groups.len(), 2);

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
    assert!(clear_result["zone"]["effect_id"].is_null());
    assert_eq!(
        clear_result["zone"]["layers"].as_array().map(Vec::len),
        Some(0)
    );

    let clear_snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(clear_snapshot.default_scene_groups.len(), 2);
    let display_group = clear_snapshot
        .default_scene_groups
        .iter()
        .find(|group| group.role == hypercolor_types::scene::ZoneRole::Display)
        .expect("display screen surface should survive face clear");
    assert_eq!(display_group.effect_id, None);
    assert!(display_group.layers.is_empty());

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

/// `set_effect` takes no transition argument at all.
///
/// The parameter's only accepted value was its no-op, which fails the
/// same rule that deletes an ignored parameter (Spec 78 §6.1). The
/// shared transition vocabulary arrives with the 78.1 contract; until
/// then the tool advertises a closed shape with no transition in it.
#[test]
fn set_effect_advertises_no_transition_argument() {
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
    assert_eq!(declared, vec!["controls".to_owned(), "query".to_owned()]);
    assert_eq!(
        set_effect.input_schema["additionalProperties"],
        json!(false),
        "the closed shape is what stops a client sending a deleted parameter"
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
            "set_brightness",
            json!({ "brightness": 42, "device_id": "strip-1" }),
            "device_id",
        ),
        (
            "stop_effect",
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

    let manager = state.scene_manager.read().await;
    assert!(
        manager
            .active_scene()
            .and_then(|scene| scene.primary_group())
            .and_then(|zone| zone.effect_id)
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

    assert_eq!(result["applied"], true);
    assert_eq!(
        result["transition_ms"], 0,
        "the echoed duration is the one the daemon applied, not a default it ignored"
    );
}

#[tokio::test]
async fn stateful_set_color_refuses_a_transition_it_cannot_apply() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    insert_test_effect(&state, "Solid Color").await;

    let error = execute_tool_with_state(
        "set_color",
        &json!({
            "color": "#ff6ac1",
            "transition_ms": 400
        }),
        state.as_ref(),
    )
    .await
    .expect_err("effect transitions are not implemented");
    assert!(
        format!("{error}").contains("not implemented yet"),
        "unexpected error: {error}"
    );
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
async fn stateful_set_effect_and_stop_effect_sync_scene_runtime_and_events() {
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
    assert_eq!(apply_result["applied"], true);
    assert_eq!(apply_result["matched_effect"]["id"], effect.id.to_string());
    assert_eq!(
        apply_result["applied_controls"]["speed"]["float"],
        json!(7.5)
    );
    assert_eq!(apply_result["rejected_controls"], json!([]));

    let (scene_id, active_group) = {
        let manager = state.scene_manager.read().await;
        (
            manager
                .active_scene_id()
                .copied()
                .expect("default scene should stay active"),
            manager
                .active_scene()
                .and_then(|scene| scene.primary_group())
                .cloned()
                .expect("primary group should exist after MCP set_effect"),
        )
    };
    assert_eq!(active_group.effect_id, Some(effect.id));
    assert_eq!(
        active_group.controls.get("speed"),
        Some(&ControlValue::Float(7.5))
    );

    let active_snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(active_snapshot.default_scene_groups.len(), 1);
    assert_eq!(
        active_snapshot.default_scene_groups[0].effect_id,
        Some(effect.id)
    );
    assert_eq!(
        active_snapshot.default_scene_groups[0]
            .controls
            .get("speed"),
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
    let mut saw_group_event = false;
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
                saw_group_event = true;
            }
            _ => {}
        }
    }
    assert!(saw_started_event, "expected MCP effect-start event");
    assert!(saw_group_event, "expected MCP render-group event");

    let mut stop_events = state.event_bus.subscribe_all();
    let stop_result = execute_tool_with_state("stop_effect", &json!({}), state.as_ref())
        .await
        .expect("stop_effect should succeed");
    assert_eq!(stop_result["stopped"], true);
    assert_eq!(stop_result["effect"]["id"], effect.id.to_string());

    let stopped_snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(stopped_snapshot.default_scene_groups.len(), 1);
    assert_eq!(stopped_snapshot.default_scene_groups[0].effect_id, None);
    assert!(stopped_snapshot.default_scene_groups[0].controls.is_empty());

    let cleared_group = {
        let manager = state.scene_manager.read().await;
        manager
            .active_scene()
            .and_then(|scene| scene.primary_group())
            .cloned()
            .expect("primary group should remain present after stop")
    };
    assert_eq!(cleared_group.effect_id, None);
    assert!(cleared_group.controls.is_empty());

    let mut saw_stopped_event = false;
    let mut saw_updated_group = false;
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
                saw_updated_group = true;
            }
            _ => {}
        }
    }
    assert!(saw_stopped_event, "expected MCP effect-stop event");
    assert!(saw_updated_group, "expected MCP group-clear event");
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

    let active_group = {
        let manager = state.scene_manager.read().await;
        manager
            .active_scene()
            .and_then(|scene| scene.primary_group())
            .cloned()
            .expect("primary group should exist after MCP set_effect")
    };
    assert_eq!(active_group.effect_id, Some(next.id));
    assert_eq!(active_group.layout, expected_layout);
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
            "brightness": 50
        }),
        state.as_ref(),
    )
    .await
    .expect("set_color should succeed");
    assert_eq!(result["applied"], true);
    assert_eq!(result["resolved_color"]["hex"], "#ff6ac1");

    let snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(snapshot.default_scene_groups.len(), 1);
    assert_eq!(
        snapshot.default_scene_groups[0].effect_id,
        Some(solid_effect.id)
    );
    assert_eq!(
        snapshot.default_scene_groups[0].controls.get("brightness"),
        Some(&ControlValue::Float(0.5))
    );
    match snapshot.default_scene_groups[0].controls.get("color") {
        Some(ControlValue::Color([r, g, b, a])) => {
            assert_eq!((*r, *g, *b, *a), (1.0, 106.0 / 255.0, 193.0 / 255.0, 1.0));
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

    let active_group = {
        let manager = state.scene_manager.read().await;
        manager
            .active_scene()
            .and_then(|scene| scene.primary_group())
            .cloned()
            .expect("primary group should exist after MCP set_color")
    };
    assert_eq!(active_group.effect_id, Some(solid_effect.id));
    assert_eq!(active_group.layout, expected_layout);
}

#[test]
fn tool_definitions_have_valid_schemas() {
    let tools = build_tool_definitions();
    assert_eq!(tools.len(), 16);
    assert!(
        tools
            .iter()
            .all(|tool| tool.input_schema["type"] == "object")
    );
    assert!(tools.iter().all(|tool| tool.output_schema.is_object()));
    assert!(tools.iter().any(|tool| tool.name == "set_display_face"));
    let diagnose = tools
        .iter()
        .find(|tool| tool.name == "diagnose")
        .expect("diagnose tool should be registered");
    assert_eq!(
        diagnose.output_schema["properties"]["overall_status"]["enum"],
        json!(["healthy", "warning", "unhealthy"])
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
        ("stop_effect", "transition_ms"),
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
        (tool.read_only, tool.destructive)
    };

    // Tools that discard state the caller cannot recover.
    for name in [
        "stop_effect",
        "set_effect",
        "set_color",
        "activate_scene",
        "set_display_face",
    ] {
        assert_eq!(annotation(name), (false, true), "{name}");
    }

    // Reversible value writes and pure creations.
    for name in ["set_brightness", "set_output_power", "create_scene"] {
        assert_eq!(annotation(name), (false, false), "{name}");
    }

    // Read-only tools never claim to destroy anything.
    for tool in tools.iter().filter(|tool| tool.read_only) {
        assert!(!tool.destructive, "{} is read-only", tool.name);
    }
}

#[tokio::test]
async fn set_color_tool_rejects_missing_color() {
    let state = fresh_app_state();

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
    let state = fresh_app_state();

    let error = execute_tool_with_state("set_output_power", &json!({ "state": "off" }), &state)
        .await
        .expect_err("unknown output state should be rejected");
    assert!(matches!(error, ToolError::InvalidParam { .. }));
}

#[tokio::test]
async fn stateful_set_output_power_is_reversible_and_idempotent() {
    let state = Arc::new(fresh_app_state());

    let paused = execute_tool_with_state(
        "set_output_power",
        &json!({ "state": "paused" }),
        state.as_ref(),
    )
    .await
    .expect("pause should succeed");
    assert_eq!(paused["state"], "paused");
    assert!(state.power_state.borrow().manually_paused());

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
    assert!(!state.power_state.borrow().sleeping());
}

/// `set_brightness` is a projection of the output service, so the tool
/// moves the same live state `GET /output` reports and persists the
/// same store the REST route does.
#[tokio::test]
async fn set_brightness_tool_projects_the_output_service() {
    let (state, _tmp) = isolated_state_with_tempdir();

    let response = execute_tool_with_state("set_brightness", &json!({ "brightness": 35 }), &state)
        .await
        .expect("brightness should be accepted");
    assert_eq!(response["brightness"], 35);
    assert_eq!(response["previous_brightness"], 100);
    assert!((state.power_state.borrow().global_brightness - 0.35).abs() < 1e-6);
    assert!(
        (state.device_settings.read().await.global_brightness() - 0.35).abs() < 1e-6,
        "the tool must persist through the same store the REST route writes"
    );

    let error = execute_tool_with_state("set_brightness", &json!({ "brightness": 150 }), &state)
        .await
        .expect_err("out-of-range brightness should be rejected");
    assert!(matches!(error, ToolError::InvalidParam { .. }));
}

#[test]
fn resource_definitions_are_readable() {
    let resources = build_resource_definitions();
    assert_eq!(resources.len(), 5);
    assert!(
        resources
            .iter()
            .all(|resource| resource.uri.starts_with("hypercolor://"))
    );
    assert!(is_valid_resource_uri("hypercolor://state"));
    assert!(is_valid_resource_uri("hypercolor://scenes"));
    assert!(!is_valid_resource_uri("hypercolor://profiles"));
    assert!(read_resource("hypercolor://state").is_some());
    assert!(read_resource("hypercolor://scenes").is_some());
    assert!(read_resource("hypercolor://profiles").is_none());
    assert!(read_resource("hypercolor://nope").is_none());
}

#[tokio::test]
async fn mcp_device_inventory_exposes_driver_origin_and_presentation() {
    let state = Arc::new(fresh_app_state());
    let device_id = insert_test_display_device(&state, "Case Display").await;

    let resource = read_resource_with_state("hypercolor://devices", state.as_ref())
        .await
        .expect("devices resource should exist");
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
    let messages = get_prompt_messages("mood_lighting", &json!({ "mood": "cozy evening" }))
        .expect("prompt should build messages");
    assert!(messages["messages"].is_array());
    let automation = get_prompt_messages("setup_automation", &json!({}))
        .expect("automation prompt should build messages");
    let encoded = automation.to_string();
    assert!(encoded.contains("hypercolor://scenes"));
    assert!(!encoded.contains("hypercolor://profiles"));
    assert!(encoded.contains("does not schedule or trigger scenes"));
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

    // The preference persists and the overlay reaches the render groups.
    assert!(
        state
            .display_preferences
            .read()
            .await
            .get(display_id)
            .is_some()
    );
    assert!(
        state
            .scene_manager
            .read()
            .await
            .active_render_groups()
            .iter()
            .any(|zone| zone.effect_id == Some(face.id))
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
            .display_preferences
            .read()
            .await
            .get(display_id)
            .is_none()
    );
}
