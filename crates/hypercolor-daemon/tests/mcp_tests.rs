//! Integration tests for the MCP server module.
//!
//! Covers protocol compliance, tool definitions, resource URIs, prompt templates,
//! fuzzy matching, and JSON-RPC error handling.

use hypercolor_daemon::mcp::McpServer;
use hypercolor_daemon::mcp::fuzzy::{match_effect, resolve_color};
use hypercolor_daemon::mcp::prompts::{
    build_prompt_definitions, get_prompt_messages, is_valid_prompt,
};
use hypercolor_daemon::mcp::resources::{
    build_resource_definitions, is_valid_resource_uri, read_resource,
};
use hypercolor_daemon::mcp::tools::{ToolError, build_tool_definitions, execute_tool};
use serde_json::{Value, json};

// ── Helper ─────────────────────────────────────────────────────────────────

/// Send a JSON-RPC request to a server and parse the response.
fn rpc_call(server: &mut McpServer, method: &str, params: &Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    });
    let input = serde_json::to_string(&request).expect("request serialization should succeed");
    let output = server
        .handle_message(&input)
        .expect("non-notification request should produce a response");
    serde_json::from_str(&output).expect("response should be valid JSON")
}

/// Initialize a server (required before most method calls).
fn initialized_server() -> McpServer {
    let mut server = McpServer::new();
    let response = rpc_call(&mut server, "initialize", &json!({}));
    assert!(
        response.get("result").is_some(),
        "initialize should succeed"
    );
    server
}

// ── JSON-RPC Protocol Compliance ───────────────────────────────────────────

#[test]
fn test_parse_error_on_invalid_json() {
    let mut server = McpServer::new();
    let response = server
        .handle_message("not valid json {{{")
        .expect("parse error should still produce a response");
    let parsed: Value = serde_json::from_str(&response).expect("response should be valid JSON");
    let error = parsed.get("error").expect("should have error field");
    assert_eq!(error["code"], -32700, "should be PARSE_ERROR code");
}

#[test]
fn test_invalid_jsonrpc_version() {
    let mut server = McpServer::new();
    let request = json!({
        "jsonrpc": "1.0",
        "id": 1,
        "method": "initialize"
    });
    let response = rpc_call_raw(&mut server, &request);
    let error = response.get("error").expect("should have error field");
    assert_eq!(error["code"], -32600, "should be INVALID_REQUEST code");
}

#[test]
fn test_method_not_found() {
    let mut server = initialized_server();
    let response = rpc_call(&mut server, "nonexistent/method", &json!({}));
    let error = response.get("error").expect("should have error field");
    assert_eq!(error["code"], -32601, "should be METHOD_NOT_FOUND code");
}

#[test]
fn test_notification_returns_none() {
    let mut server = McpServer::new();
    // Notification = no "id" field
    let request = json!({
        "jsonrpc": "2.0",
        "method": "initialized"
    });
    let input = serde_json::to_string(&request).expect("serialization should succeed");
    let result = server.handle_message(&input);
    assert!(
        result.is_none(),
        "notifications should not produce a response"
    );
}

#[test]
fn test_requires_initialization() {
    let mut server = McpServer::new();
    let response = rpc_call(&mut server, "tools/list", &json!({}));
    let error = response.get("error").expect("should reject pre-init call");
    assert_eq!(error["code"], -32600);
    assert!(
        error["message"]
            .as_str()
            .unwrap_or("")
            .contains("not initialized"),
        "error message should mention initialization"
    );
}

#[test]
fn test_ping_always_works() {
    let mut server = McpServer::new();
    // ping should work even without initialization
    let response = rpc_call(&mut server, "ping", &json!({}));
    assert!(response.get("result").is_some());
}

// ── Initialize Handshake ───────────────────────────────────────────────────

#[test]
fn test_initialize_response_shape() {
    let mut server = McpServer::new();
    let response = rpc_call(&mut server, "initialize", &json!({}));
    let result = response.get("result").expect("should have result");

    assert_eq!(result["protocolVersion"], "2025-11-25");
    assert!(result["capabilities"]["tools"].is_object());
    assert!(result["capabilities"]["resources"].is_object());
    assert!(result["capabilities"]["prompts"].is_object());
    assert_eq!(result["serverInfo"]["name"], "hypercolor");
    assert!(result["instructions"].is_string());
}

// ── Tools ──────────────────────────────────────────────────────────────────

#[test]
fn test_tool_definitions_count() {
    let tools = build_tool_definitions();
    assert_eq!(tools.len(), 14, "should have exactly 14 tools");
}

#[test]
fn test_tool_definitions_have_valid_schemas() {
    let tools = build_tool_definitions();
    for tool in &tools {
        assert!(!tool.name.is_empty(), "tool name should not be empty");
        assert!(
            !tool.description.is_empty(),
            "tool {}: description should not be empty",
            tool.name
        );
        assert_eq!(
            tool.input_schema["type"], "object",
            "tool {}: input_schema root type should be 'object'",
            tool.name
        );
    }
}

#[test]
fn test_tool_names_are_snake_case() {
    let tools = build_tool_definitions();
    for tool in &tools {
        assert!(
            tool.name.chars().all(|c| c.is_lowercase() || c == '_'),
            "tool name '{}' should be snake_case",
            tool.name
        );
    }
}

#[test]
fn test_tools_list_via_protocol() {
    let mut server = initialized_server();
    let response = rpc_call(&mut server, "tools/list", &json!({}));
    let result = response.get("result").expect("should have result");
    let tools = result["tools"]
        .as_array()
        .expect("tools should be an array");
    assert_eq!(tools.len(), 14);

    // Every tool should have name, description, inputSchema
    for tool in tools {
        assert!(tool["name"].is_string());
        assert!(tool["description"].is_string());
        assert!(tool["inputSchema"].is_object());
    }
}

#[test]
fn test_tools_call_missing_name() {
    let mut server = initialized_server();
    let response = rpc_call(&mut server, "tools/call", &json!({}));
    // Missing 'name' parameter triggers a JSON-RPC-level error (not a tool-level isError)
    assert!(
        response.get("error").is_some(),
        "tools/call without 'name' should return a JSON-RPC error"
    );
}

#[test]
fn test_tools_call_unknown_tool() {
    let mut server = initialized_server();
    let response = rpc_call(
        &mut server,
        "tools/call",
        &json!({ "name": "nonexistent_tool", "arguments": {} }),
    );
    let result = response.get("result").expect("should have result");
    assert_eq!(result["isError"], true);
}

#[test]
fn test_set_color_tool_valid_hex() {
    let result = execute_tool("set_color", &json!({ "color": "#ff6ac1" }));
    let value = result.expect("set_color with valid hex should succeed");
    assert_eq!(value["applied"], true);
    assert_eq!(value["resolved_color"]["hex"], "#ff6ac1");
    assert_eq!(value["resolved_color"]["rgb"]["r"], 255);
    assert_eq!(value["resolved_color"]["rgb"]["g"], 106);
    assert_eq!(value["resolved_color"]["rgb"]["b"], 193);
}

#[test]
fn test_set_color_tool_named_color() {
    let result = execute_tool("set_color", &json!({ "color": "coral" }));
    let value = result.expect("set_color with named color should succeed");
    assert_eq!(value["applied"], true);
    assert_eq!(value["resolved_color"]["name"], "coral");
}

#[test]
fn test_set_color_missing_param() {
    let result = execute_tool("set_color", &json!({}));
    assert!(result.is_err(), "set_color without 'color' should fail");
    let err = result.expect_err("set_color without 'color' should produce error");
    assert!(matches!(err, ToolError::MissingParam(_)));
}

#[test]
fn test_set_brightness_validation() {
    // Valid brightness
    let result = execute_tool("set_brightness", &json!({ "brightness": 50 }));
    assert!(result.is_ok());
    assert_eq!(result.expect("should succeed")["brightness"], 50);

    // Missing brightness
    let result = execute_tool("set_brightness", &json!({}));
    assert!(result.is_err());

    // Out of range
    let result = execute_tool("set_brightness", &json!({ "brightness": 200 }));
    assert!(result.is_err());
}

#[test]
fn test_get_status_tool() {
    let result = execute_tool("get_status", &json!({}));
    let value = result.expect("get_status should succeed");
    assert_eq!(value["running"], true);
    assert!(value["brightness"].is_number());
    assert!(value["devices"].is_object());
    assert!(value["uptime_seconds"].is_number());
}

#[test]
fn test_create_scene_required_params() {
    // Missing name
    let result = execute_tool(
        "create_scene",
        &json!({
            "profile_id": "test",
            "trigger": { "type": "schedule" }
        }),
    );
    assert!(result.is_err());

    // Missing profile_id
    let result = execute_tool(
        "create_scene",
        &json!({
            "name": "Test Scene",
            "trigger": { "type": "schedule" }
        }),
    );
    assert!(result.is_err());

    // Missing trigger
    let result = execute_tool(
        "create_scene",
        &json!({
            "name": "Test Scene",
            "profile_id": "test"
        }),
    );
    assert!(result.is_err());

    // All required params present
    let result = execute_tool(
        "create_scene",
        &json!({
            "name": "Test Scene",
            "profile_id": "test",
            "trigger": { "type": "schedule" }
        }),
    );
    assert!(result.is_ok());
    let value = result.expect("should succeed");
    assert_eq!(value["name"], "Test Scene");
    assert!(value["scene_id"].is_string());
}

#[test]
fn test_diagnose_tool() {
    let result = execute_tool("diagnose", &json!({}));
    let value = result.expect("diagnose should succeed");
    assert_eq!(value["overall_status"], "healthy");
    assert!(value["findings"].is_array());
}

#[test]
fn test_set_effect_missing_query() {
    let result = execute_tool("set_effect", &json!({}));
    assert!(result.is_err());
}

#[test]
fn test_activate_scene_missing_name() {
    let result = execute_tool("activate_scene", &json!({}));
    assert!(result.is_err());
}

#[test]
fn test_set_profile_missing_query() {
    let result = execute_tool("set_profile", &json!({}));
    assert!(result.is_err());
}

// ── Resources ──────────────────────────────────────────────────────────────

#[test]
fn test_resource_definitions_count() {
    let resources = build_resource_definitions();
    assert_eq!(resources.len(), 5, "should have exactly 5 resources");
}

#[test]
fn test_resource_uris_use_hypercolor_scheme() {
    let resources = build_resource_definitions();
    for resource in &resources {
        assert!(
            resource.uri.starts_with("hypercolor://"),
            "resource URI '{}' should use hypercolor:// scheme",
            resource.uri
        );
    }
}

#[test]
fn test_valid_resource_uri_check() {
    assert!(is_valid_resource_uri("hypercolor://state"));
    assert!(is_valid_resource_uri("hypercolor://devices"));
    assert!(is_valid_resource_uri("hypercolor://effects"));
    assert!(is_valid_resource_uri("hypercolor://profiles"));
    assert!(is_valid_resource_uri("hypercolor://audio"));
    assert!(!is_valid_resource_uri("hypercolor://nonexistent"));
    assert!(!is_valid_resource_uri("http://state"));
}

#[test]
fn test_read_all_resources() {
    let resources = build_resource_definitions();
    for resource in &resources {
        let content = read_resource(&resource.uri);
        assert!(
            content.is_some(),
            "resource '{}' should return content",
            resource.uri
        );
        let value = content.expect("should have content");
        assert!(
            value.is_object(),
            "resource content should be a JSON object"
        );
    }
}

#[test]
fn test_read_unknown_resource() {
    let content = read_resource("hypercolor://unknown");
    assert!(content.is_none());
}

#[test]
fn test_resources_list_via_protocol() {
    let mut server = initialized_server();
    let response = rpc_call(&mut server, "resources/list", &json!({}));
    let result = response.get("result").expect("should have result");
    let resources = result["resources"]
        .as_array()
        .expect("resources should be an array");
    assert_eq!(resources.len(), 5);

    for resource in resources {
        assert!(resource["uri"].is_string());
        assert!(resource["name"].is_string());
        assert!(resource["mimeType"].is_string());
    }
}

#[test]
fn test_resources_read_via_protocol() {
    let mut server = initialized_server();
    let response = rpc_call(
        &mut server,
        "resources/read",
        &json!({ "uri": "hypercolor://state" }),
    );
    let result = response.get("result").expect("should have result");
    let contents = result["contents"]
        .as_array()
        .expect("contents should be an array");
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["uri"], "hypercolor://state");
    assert!(contents[0]["text"].is_string());
}

#[test]
fn test_resources_read_invalid_uri() {
    let mut server = initialized_server();
    let response = rpc_call(
        &mut server,
        "resources/read",
        &json!({ "uri": "hypercolor://nope" }),
    );
    assert!(response.get("error").is_some());
}

// ── Prompts ────────────────────────────────────────────────────────────────

#[test]
fn test_prompt_definitions_count() {
    let prompts = build_prompt_definitions();
    assert_eq!(prompts.len(), 3, "should have exactly 3 prompts");
}

#[test]
fn test_prompt_names() {
    let prompts = build_prompt_definitions();
    let names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"mood_lighting"));
    assert!(names.contains(&"troubleshoot"));
    assert!(names.contains(&"setup_automation"));
}

#[test]
fn test_valid_prompt_check() {
    assert!(is_valid_prompt("mood_lighting"));
    assert!(is_valid_prompt("troubleshoot"));
    assert!(is_valid_prompt("setup_automation"));
    assert!(!is_valid_prompt("nonexistent"));
}

#[test]
fn test_mood_lighting_prompt_messages() {
    let messages = get_prompt_messages("mood_lighting", &json!({ "mood": "cozy evening" }));
    let value = messages.expect("mood_lighting should produce messages");
    assert!(value["messages"].is_array());
    let msgs = value["messages"].as_array().expect("should be array");
    assert!(msgs.len() >= 3, "should have multiple message turns");

    // First message should include the mood
    let first_text = msgs[0]["content"]["text"]
        .as_str()
        .expect("first message should have text");
    assert!(
        first_text.contains("cozy evening"),
        "first message should include the mood"
    );
}

#[test]
fn test_troubleshoot_prompt_messages() {
    let messages = get_prompt_messages(
        "troubleshoot",
        &json!({ "issue": "WLED strip not responding" }),
    );
    let value = messages.expect("troubleshoot should produce messages");
    let msgs = value["messages"].as_array().expect("should be array");
    let first_text = msgs[0]["content"]["text"]
        .as_str()
        .expect("should have text");
    assert!(first_text.contains("WLED strip not responding"));
}

#[test]
fn test_setup_automation_prompt_messages() {
    let messages = get_prompt_messages(
        "setup_automation",
        &json!({ "description": "dim lights at 10pm" }),
    );
    let value = messages.expect("setup_automation should produce messages");
    let msgs = value["messages"].as_array().expect("should be array");
    let first_text = msgs[0]["content"]["text"]
        .as_str()
        .expect("should have text");
    assert!(first_text.contains("dim lights at 10pm"));
}

#[test]
fn test_unknown_prompt() {
    let messages = get_prompt_messages("nonexistent", &json!({}));
    assert!(messages.is_none());
}

#[test]
fn test_prompts_list_via_protocol() {
    let mut server = initialized_server();
    let response = rpc_call(&mut server, "prompts/list", &json!({}));
    let result = response.get("result").expect("should have result");
    let prompts = result["prompts"]
        .as_array()
        .expect("prompts should be an array");
    assert_eq!(prompts.len(), 3);

    for prompt in prompts {
        assert!(prompt["name"].is_string());
        assert!(prompt["description"].is_string());
        assert!(prompt["arguments"].is_array());
    }
}

#[test]
fn test_prompts_get_via_protocol() {
    let mut server = initialized_server();
    let response = rpc_call(
        &mut server,
        "prompts/get",
        &json!({
            "name": "mood_lighting",
            "arguments": { "mood": "chill" }
        }),
    );
    let result = response.get("result").expect("should have result");
    assert!(result["messages"].is_array());
}

// ── Fuzzy Matching: Effects ────────────────────────────────────────────────

#[test]
fn test_match_effect_empty_catalog() {
    let results = match_effect("aurora", &[]);
    assert!(results.is_empty());
}

#[test]
fn test_match_effect_exact() {
    let effects = test_effects();
    let results = match_effect("Aurora Borealis", &effects);
    assert!(!results.is_empty(), "exact match should produce results");
    assert_eq!(results[0].effect.name, "Aurora Borealis");
    assert!(
        (results[0].score - 1.0).abs() < f32::EPSILON,
        "exact match score should be 1.0"
    );
}

#[test]
fn test_match_effect_fuzzy() {
    let effects = test_effects();
    let results = match_effect("aurra", &effects);
    // Should still match Aurora Borealis with a decent score
    assert!(!results.is_empty(), "fuzzy match should produce results");
    assert!(results[0].score > 0.3);
}

#[test]
fn test_match_effect_by_tag() {
    let effects = test_effects();
    let results = match_effect("calm nature", &effects);
    // Should match effects tagged with "calm" or "nature"
    assert!(!results.is_empty(), "tag match should produce results");
}

#[test]
fn test_match_effect_below_threshold_excluded() {
    let effects = test_effects();
    let results = match_effect("xyzzy quantum flux capacitor", &effects);
    // With completely unrelated query, most results should be filtered out
    for m in &results {
        assert!(
            m.score > 0.3,
            "all returned matches should be above threshold"
        );
    }
}

// ── Fuzzy Matching: Colors ─────────────────────────────────────────────────

#[test]
fn test_resolve_color_hex() {
    let result = resolve_color("#ff6ac1").expect("should resolve hex");
    assert_eq!(result.r, 255);
    assert_eq!(result.g, 106);
    assert_eq!(result.b, 193);
    assert_eq!(result.hex, "#ff6ac1");
    assert!((result.confidence - 1.0).abs() < f32::EPSILON);
}

#[test]
fn test_resolve_color_hex_no_hash() {
    let result = resolve_color("ff6ac1").expect("should resolve hex without #");
    assert_eq!(result.r, 255);
    assert_eq!(result.g, 106);
    assert_eq!(result.b, 193);
}

#[test]
fn test_resolve_color_rgb_function() {
    let result = resolve_color("rgb(255, 106, 193)").expect("should resolve rgb()");
    assert_eq!(result.r, 255);
    assert_eq!(result.g, 106);
    assert_eq!(result.b, 193);
    assert!((result.confidence - 1.0).abs() < f32::EPSILON);
}

#[test]
fn test_resolve_color_hsl_function() {
    let result = resolve_color("hsl(0, 100%, 50%)").expect("should resolve hsl()");
    // hsl(0, 100%, 50%) = pure red
    assert_eq!(result.r, 255);
    assert_eq!(result.g, 0);
    assert_eq!(result.b, 0);
}

#[test]
fn test_resolve_color_named() {
    let result = resolve_color("coral").expect("should resolve named color");
    assert_eq!(result.name, "coral");
    assert_eq!(result.r, 255);
    assert_eq!(result.g, 127);
    assert_eq!(result.b, 80);
    assert!((result.confidence - 1.0).abs() < f32::EPSILON);
}

#[test]
fn test_resolve_color_named_case_insensitive() {
    let result = resolve_color("CORAL").expect("should resolve uppercase");
    assert_eq!(result.name, "coral");
}

#[test]
fn test_resolve_color_blue() {
    let result = resolve_color("blue").expect("should resolve 'blue'");
    assert_eq!(result.name, "blue");
    assert_eq!(result.r, 0);
    assert_eq!(result.g, 0);
    assert_eq!(result.b, 255);
}

#[test]
fn test_resolve_color_warm_white() {
    let result = resolve_color("warm white").expect("should resolve compound name");
    assert_eq!(result.name, "warm white");
}

#[test]
fn test_resolve_color_natural_language() {
    let result = resolve_color("ocean blue").expect("should resolve 'ocean blue'");
    assert_eq!(result.name, "ocean blue");
}

#[test]
fn test_resolve_color_empty_string() {
    let result = resolve_color("");
    assert!(result.is_none(), "empty string should not resolve");
}

#[test]
fn test_resolve_color_invalid_hex() {
    // "gggggg" is not valid hex, should fall through to name matching
    let result = resolve_color("#gggggg");
    // May or may not match a fuzzy color name — the key is it doesn't panic
    // and doesn't return a bogus hex parse.
    if let Some(color) = result {
        assert!(
            color.confidence < 1.0,
            "invalid hex should not be a perfect match"
        );
    }
}

// ── Test Helpers ───────────────────────────────────────────────────────────

/// Build a small catalog of test effects.
fn test_effects() -> Vec<hypercolor_types::effect::EffectMetadata> {
    use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
    use std::path::PathBuf;

    vec![
        EffectMetadata {
            id: EffectId::new(uuid::Uuid::now_v7()),
            name: "Aurora Borealis".into(),
            author: "Hypercolor".into(),
            version: "1.0.0".into(),
            description: "Northern lights shimmer with calm, flowing colors".into(),
            category: EffectCategory::Ambient,
            tags: vec![
                "calm".into(),
                "nature".into(),
                "aurora".into(),
                "northern-lights".into(),
            ],
            source: EffectSource::Native {
                path: PathBuf::from("aurora.wgsl"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: EffectId::new(uuid::Uuid::now_v7()),
            name: "Fire Blaze".into(),
            author: "Hypercolor".into(),
            version: "1.0.0".into(),
            description: "Realistic fire simulation with flickering warmth".into(),
            category: EffectCategory::Ambient,
            tags: vec!["fire".into(), "warm".into(), "flickering".into()],
            source: EffectSource::Native {
                path: PathBuf::from("fire.wgsl"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: EffectId::new(uuid::Uuid::now_v7()),
            name: "Beat Pulse".into(),
            author: "Hypercolor".into(),
            version: "1.0.0".into(),
            description: "Pulse colors to the beat of your music".into(),
            category: EffectCategory::Audio,
            tags: vec![
                "beat".into(),
                "music".into(),
                "pulse".into(),
                "reactive".into(),
            ],
            source: EffectSource::Native {
                path: PathBuf::from("beat_pulse.wgsl"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: EffectId::new(uuid::Uuid::now_v7()),
            name: "Rainbow Wave".into(),
            author: "Hypercolor".into(),
            version: "1.0.0".into(),
            description: "Classic rainbow gradient flowing across LEDs".into(),
            category: EffectCategory::Generative,
            tags: vec![
                "rainbow".into(),
                "gradient".into(),
                "wave".into(),
                "colorful".into(),
            ],
            source: EffectSource::Native {
                path: PathBuf::from("rainbow.wgsl"),
            },
            license: Some("Apache-2.0".into()),
        },
    ]
}

/// Send a raw JSON value as a request.
fn rpc_call_raw(server: &mut McpServer, request: &Value) -> Value {
    let input = serde_json::to_string(request).expect("serialization should succeed");
    let output = server
        .handle_message(&input)
        .expect("should produce a response");
    serde_json::from_str(&output).expect("response should be valid JSON")
}
