//! Integration tests for the MCP HTTP surface and its reusable domain helpers.

use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use hypercolor_core::config::ConfigManager;
use hypercolor_core::effect::{EffectRenderer, FrameInput};
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::mcp;
use hypercolor_daemon::mcp::prompts::{
    build_prompt_definitions, get_prompt_messages, is_valid_prompt,
};
use hypercolor_daemon::mcp::resources::{
    build_resource_definitions, is_valid_resource_uri, read_resource,
};
use hypercolor_daemon::mcp::tools::{
    ToolError, build_tool_definitions, execute_tool, execute_tool_with_state,
};
use hypercolor_types::canvas::Canvas;
use hypercolor_types::config::{CURRENT_SCHEMA_VERSION, McpConfig};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::effect::{
    ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource,
};
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, Scene, SceneId, ScenePriority, SceneScope, TransitionSpec,
    UnassignedBehavior,
};
use reqwest::{Client, Response};
use serde_json::{Value, json};
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

async fn insert_test_display_device(state: &Arc<AppState>, name: &str) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::Wled,
        model: Some("LCD".to_owned()),
        connection_type: ConnectionType::Usb,
        zones: vec![ZoneInfo {
            name: "LCD".to_owned(),
            led_count: 320 * 320,
            topology: DeviceTopologyHint::Display {
                width: 320,
                height: 320,
                circular: true,
            },
            color_format: DeviceColorFormat::Rgb,
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

struct TestHtmlRenderer;

impl EffectRenderer for TestHtmlRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        if canvas.width() != input.canvas_width || canvas.height() != input.canvas_height {
            *canvas = Canvas::new(input.canvas_width, input.canvas_height);
        }
        Ok(())
    }

    fn set_control(&mut self, _name: &str, _value: &ControlValue) {}

    fn destroy(&mut self) {}
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

async fn activate_test_html_effect(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = test_html_effect_metadata(name);
    let mut engine = state.effect_engine.lock().await;
    engine
        .activate(Box::new(TestHtmlRenderer), metadata.clone())
        .expect("html test effect should activate");
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

async fn activate_empty_test_scene(state: &Arc<AppState>, name: &str) -> SceneId {
    let scene = Scene {
        id: SceneId::new(),
        name: name.to_owned(),
        description: None,
        scope: SceneScope::Full,
        zone_assignments: Vec::new(),
        groups: Vec::new(),
        transition: TransitionSpec {
            duration_ms: 0,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
    };

    let mut manager = state.scene_manager.write().await;
    manager
        .create(scene.clone())
        .expect("test scene should be created");
    manager
        .activate(&scene.id, None)
        .expect("test scene should activate");
    scene.id
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
    let state = Arc::new(AppState::new());
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
    assert_eq!(result["protocolVersion"], "2025-06-18");
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
    let state = Arc::new(AppState::new());
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
    assert_eq!(tools.len(), 18);
    assert!(tools.iter().all(|tool| tool["outputSchema"].is_object()));
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "list_display_overlays")
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "set_display_overlay")
    );
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let mut state = AppState::new();
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
async fn stateful_overlay_tools_manage_display_configs() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let display_id = insert_test_display_device(&state, "Pump LCD").await;

    let create_result = execute_tool_with_state(
        "set_display_overlay",
        &json!({
            "device": display_id.to_string(),
            "slot": {
                "name": "CPU Temp",
                "source": {
                    "type": "text",
                    "text": "CPU {sensor:cpu_temp}\u{00b0}C",
                    "font_size": 20,
                    "color": "#ffffff",
                    "align": "center"
                },
                "position": "full_screen",
                "opacity": 0.85
            }
        }),
        state.as_ref(),
    )
    .await
    .expect("append overlay should succeed");
    assert_eq!(create_result["applied"], true);
    assert_eq!(create_result["overlay_count"], 1);
    let slot_id = create_result["affected_slot"]["id"]
        .as_str()
        .expect("slot id should be returned")
        .to_owned();

    let list_result = execute_tool_with_state(
        "list_display_overlays",
        &json!({
            "device": display_id.to_string(),
        }),
        state.as_ref(),
    )
    .await
    .expect("list overlays should succeed");
    assert_eq!(list_result["total"], 1);
    assert_eq!(
        list_result["displays"][0]["device"]["id"],
        display_id.to_string()
    );
    assert_eq!(
        list_result["displays"][0]["overlays"][0]["slot"]["id"],
        slot_id
    );
    assert_eq!(
        list_result["displays"][0]["overlays"][0]["runtime"]["status"],
        "active"
    );

    let update_result = execute_tool_with_state(
        "set_display_overlay",
        &json!({
            "device": display_id.to_string(),
            "operation": "update",
            "slot_id": slot_id,
            "slot": {
                "name": "CPU Label",
                "opacity": 0.5
            }
        }),
        state.as_ref(),
    )
    .await
    .expect("update overlay should succeed");
    assert_eq!(update_result["affected_slot"]["name"], "CPU Label");
    assert_eq!(
        update_result["config"]["overlays"][0]["opacity"],
        json!(0.5)
    );
}

#[tokio::test]
async fn stateful_display_face_tool_assigns_and_clears_face_groups() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    let scene_id = activate_empty_test_scene(&state, "Desk Scene").await;

    let assign_result = execute_tool_with_state(
        "set_display_face",
        &json!({
            "device": display_id.to_string(),
            "effect_id": face.id.to_string(),
            "controls": {
                "title": "CPU"
            }
        }),
        state.as_ref(),
    )
    .await
    .expect("display face assignment should succeed");
    assert_eq!(assign_result["scene_id"], scene_id.to_string());
    assert_eq!(assign_result["effect"]["id"], face.id.to_string());
    assert_eq!(
        assign_result["group"]["display_target"]["device_id"],
        display_id.to_string()
    );
    assert_eq!(assign_result["group"]["layout"]["canvas_width"], 320);
    assert_eq!(assign_result["group"]["controls"]["title"]["text"], "CPU");

    let clear_result = execute_tool_with_state(
        "set_display_face",
        &json!({
            "device": display_id.to_string(),
            "clear": true
        }),
        state.as_ref(),
    )
    .await
    .expect("display face clear should succeed");
    assert_eq!(clear_result["scene_id"], scene_id.to_string());
    assert_eq!(clear_result["cleared"], true);
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

#[tokio::test]
async fn list_display_overlays_reports_html_gated_runtime() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let display_id = insert_test_display_device(&state, "Pump LCD").await;

    let create_result = execute_tool_with_state(
        "set_display_overlay",
        &json!({
            "device": display_id.to_string(),
            "slot": {
                "name": "HTML Face",
                "source": {
                    "type": "html",
                    "path": "/tmp/face.html",
                    "properties": {
                        "label": "cpu"
                    },
                    "render_interval_ms": 1000
                },
                "position": "full_screen"
            }
        }),
        state.as_ref(),
    )
    .await
    .expect("append html overlay should succeed");
    assert_eq!(create_result["applied"], true);

    let active_effect = activate_test_html_effect(&state, "Servo Aurora").await;
    let warnings = hypercolor_daemon::api::displays::auto_disable_html_overlays_for_effect(
        state.as_ref(),
        &active_effect,
    )
    .await;
    assert_eq!(warnings.len(), 1);

    let list_result = execute_tool_with_state(
        "list_display_overlays",
        &json!({
            "device": display_id.to_string(),
        }),
        state.as_ref(),
    )
    .await
    .expect("list overlays should succeed");
    assert_eq!(
        list_result["displays"][0]["overlays"][0]["slot"]["enabled"],
        false
    );
    assert_eq!(
        list_result["displays"][0]["overlays"][0]["runtime"]["status"],
        "html_gated"
    );
}

#[test]
fn tool_definitions_have_valid_schemas() {
    let tools = build_tool_definitions();
    assert_eq!(tools.len(), 18);
    assert!(
        tools
            .iter()
            .all(|tool| tool.input_schema["type"] == "object")
    );
    assert!(tools.iter().all(|tool| tool.output_schema.is_object()));
    assert!(tools.iter().any(|tool| tool.name == "set_display_face"));
}

#[test]
fn set_color_tool_executes_and_validates() {
    let result = execute_tool("set_color", &json!({ "color": "#ff6ac1" }))
        .expect("set_color should succeed");
    assert_eq!(result["resolved_color"]["hex"], "#ff6ac1");

    let error =
        execute_tool("set_color", &json!({})).expect_err("missing color should return an error");
    assert!(matches!(error, ToolError::MissingParam(_)));
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
    assert!(read_resource("hypercolor://state").is_some());
    assert!(read_resource("hypercolor://nope").is_none());
}

#[test]
fn prompt_definitions_and_messages_are_valid() {
    let prompts = build_prompt_definitions();
    assert_eq!(prompts.len(), 3);
    assert!(is_valid_prompt("mood_lighting"));
    let messages = get_prompt_messages("mood_lighting", &json!({ "mood": "cozy evening" }))
        .expect("prompt should build messages");
    assert!(messages["messages"].is_array());
}
