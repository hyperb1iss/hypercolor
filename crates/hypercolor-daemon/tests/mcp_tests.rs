//! Integration tests for the MCP HTTP surface and its reusable domain helpers.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::mcp;
use hypercolor_daemon::mcp::prompts::{
    build_prompt_definitions, get_prompt_messages, is_valid_prompt,
};
use hypercolor_daemon::mcp::resources::{
    build_resource_definitions, is_valid_resource_uri, read_resource, read_resource_with_state,
};
use hypercolor_daemon::mcp::tools::{
    ToolError, build_tool_definitions, execute_tool, execute_tool_with_state,
};
use hypercolor_daemon::profile_store::{Profile, ProfilePrimary};
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
    ChangeTrigger, EffectStopReason, HypercolorEvent, RenderGroupChangeKind, SceneChangeReason,
};
use hypercolor_types::scene::SceneId;
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

fn fresh_app_state() -> AppState {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    AppState::new()
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

async fn insert_test_profile(
    state: &Arc<AppState>,
    id: &str,
    name: &str,
    effect: Option<&EffectMetadata>,
) {
    let mut profiles = state.profiles.write().await;
    let mut controls = HashMap::new();
    if effect.is_some() {
        controls.insert("speed".to_owned(), ControlValue::Float(12.0));
    }
    let mut profile = Profile::named(id, name);
    profile.description = Some(format!("{name} profile"));
    profile.brightness = Some(75);
    profile.primary = effect.map(|metadata| ProfilePrimary {
        effect_id: metadata.id,
        controls,
        active_preset_id: None,
    });
    profile.layout_id = None;
    profiles.insert(profile);
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
    insert_test_profile(&state, "focus-profile", "Focus Profile", None).await;

    let create_result = execute_tool_with_state(
        "create_scene",
        &json!({
            "name": "Focus",
            "description": "Deep work lighting",
            "profile_id": "focus-profile",
            "trigger": {
                "type": "schedule"
            }
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
    assert_eq!(
        stored_scene.metadata.get("profile_id"),
        Some(&"focus-profile".to_owned())
    );
    assert_eq!(
        stored_scene.metadata.get("trigger_type"),
        Some(&"schedule".to_owned())
    );

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
        assign_result["group"]["display_target"]["device_id"],
        display_id.to_string()
    );
    assert_eq!(assign_result["group"]["layout"]["canvas_width"], 320);
    assert_eq!(assign_result["group"]["controls"]["title"]["text"], "CPU");

    let assign_snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(
        assign_snapshot.active_scene_id,
        Some(SceneId::DEFAULT.to_string())
    );
    assert_eq!(assign_snapshot.default_scene_groups.len(), 1);

    let mut saw_assign_event = false;
    while let Ok(timestamped) = assign_events.try_recv() {
        if let HypercolorEvent::RenderGroupChanged {
            scene_id,
            kind,
            role,
            ..
        } = timestamped.event
        {
            assert_eq!(scene_id, SceneId::DEFAULT);
            assert_eq!(role, hypercolor_types::scene::RenderGroupRole::Display);
            assert_eq!(kind, RenderGroupChangeKind::Created);
            saw_assign_event = true;
        }
    }
    assert!(saw_assign_event, "expected display-face assign event");

    let mut clear_events = state.event_bus.subscribe_all();
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
    assert_eq!(clear_result["scene_id"], SceneId::DEFAULT.to_string());
    assert_eq!(clear_result["cleared"], true);

    let clear_snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert!(clear_snapshot.default_scene_groups.is_empty());

    let mut saw_clear_event = false;
    while let Ok(timestamped) = clear_events.try_recv() {
        if let HypercolorEvent::RenderGroupChanged {
            scene_id,
            kind,
            role,
            ..
        } = timestamped.event
        {
            assert_eq!(scene_id, SceneId::DEFAULT);
            assert_eq!(role, hypercolor_types::scene::RenderGroupRole::Display);
            assert_eq!(kind, RenderGroupChangeKind::Removed);
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

#[tokio::test]
async fn stateful_set_effect_conflicts_when_snapshot_scene_is_active() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    insert_test_profile(&state, "focus-profile", "Focus Profile", None).await;
    insert_test_effect(&state, "Aurora").await;

    let create_result = execute_tool_with_state(
        "create_scene",
        &json!({
            "name": "Focus",
            "profile_id": "focus-profile",
            "trigger": {
                "type": "schedule"
            },
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
            HypercolorEvent::RenderGroupChanged {
                scene_id: event_scene_id,
                kind,
                role,
                ..
            } => {
                assert_eq!(event_scene_id, scene_id);
                assert_eq!(role, hypercolor_types::scene::RenderGroupRole::Primary);
                assert_eq!(kind, RenderGroupChangeKind::Created);
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
            } => {
                assert_eq!(stopped.id, effect.id.to_string());
                assert_eq!(reason, EffectStopReason::Stopped);
                saw_stopped_event = true;
            }
            HypercolorEvent::RenderGroupChanged { kind, role, .. } => {
                assert_eq!(role, hypercolor_types::scene::RenderGroupRole::Primary);
                assert_eq!(kind, RenderGroupChangeKind::Updated);
                saw_updated_group = true;
            }
            _ => {}
        }
    }
    assert!(saw_stopped_event, "expected MCP effect-stop event");
    assert!(saw_updated_group, "expected MCP group-clear event");
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
async fn stateful_set_profile_persists_runtime_snapshot() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let effect = insert_test_effect(&state, "Movie Night").await;
    insert_test_profile(&state, "movie-profile", "Movie Profile", Some(&effect)).await;

    let result = execute_tool_with_state(
        "set_profile",
        &json!({
            "query": "movie profile"
        }),
        state.as_ref(),
    )
    .await
    .expect("set_profile should succeed");
    assert_eq!(result["applied"], true);
    assert_eq!(result["profile"]["id"], "movie-profile");

    let snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("runtime snapshot should exist");
    assert_eq!(snapshot.default_scene_groups.len(), 1);
    assert_eq!(snapshot.default_scene_groups[0].effect_id, Some(effect.id));
    assert_eq!(
        snapshot.default_scene_groups[0].controls.get("speed"),
        Some(&ControlValue::Float(12.0))
    );
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

#[tokio::test]
async fn mcp_device_inventory_exposes_driver_origin_and_presentation() {
    let state = Arc::new(fresh_app_state());
    let device_id = insert_test_display_device(&state, "Case Display").await;

    let resource = read_resource_with_state("hypercolor://devices", state.as_ref())
        .await
        .expect("devices resource should exist");
    let resource_device = &resource["devices"][0];
    assert_eq!(resource_device["id"], device_id.to_string());
    assert_eq!(resource_device["backend"], "usb");
    assert_eq!(resource_device["origin"]["driver_id"], "wled");
    assert_eq!(resource_device["origin"]["backend_id"], "usb");
    assert_eq!(resource_device["origin"]["transport"], "usb");
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
}
