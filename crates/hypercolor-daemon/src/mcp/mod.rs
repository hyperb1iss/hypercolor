//! MCP (Model Context Protocol) server for Hypercolor.
//!
//! Implements the MCP protocol as a JSON-RPC 2.0 server over stdio.
//! AI assistants communicate with Hypercolor through 14 tools, 5 resources,
//! and 3 prompt templates — translating natural language intent into
//! precise RGB hardware commands.

pub mod fuzzy;
pub mod prompts;
pub mod resources;
pub mod tools;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::prompts::build_prompt_definitions;
use self::resources::build_resource_definitions;
use self::tools::build_tool_definitions;

// ── JSON-RPC Types ────────────────────────────────────────────────────────

/// An incoming JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Request ID — `null` for notifications.
    pub id: Option<Value>,
    /// The method name to invoke.
    pub method: String,
    /// Optional method parameters.
    pub params: Option<Value>,
}

/// An outgoing JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Mirrors the request ID.
    pub id: Value,
    /// Present on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Present on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    /// Numeric error code.
    pub code: i64,
    /// Short error description.
    pub message: String,
    /// Optional additional data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// ── Standard JSON-RPC Error Codes ─────────────────────────────────────────

/// Request body is not valid JSON.
pub const PARSE_ERROR: i64 = -32700;
/// The JSON sent is not a valid Request object.
pub const INVALID_REQUEST: i64 = -32600;
/// The method does not exist or is not available.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// Invalid method parameter(s).
pub const INVALID_PARAMS: i64 = -32602;
/// Internal JSON-RPC error.
pub const INTERNAL_ERROR: i64 = -32603;

// ── MCP Server ────────────────────────────────────────────────────────────

/// The Hypercolor MCP server. Holds tool, resource, and prompt registrations.
///
/// This is the stateless protocol layer. Actual state access would come
/// from a shared `DaemonState` reference wired in at construction time.
pub struct McpServer {
    /// Registered tool definitions.
    tools: Vec<tools::ToolDefinition>,
    /// Registered resource definitions.
    resources: Vec<resources::ResourceDefinition>,
    /// Registered prompt definitions.
    prompts: Vec<prompts::PromptDefinition>,
    /// Whether the client has completed the initialize handshake.
    initialized: bool,
}

impl McpServer {
    /// Create a new MCP server with all tools, resources, and prompts registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: build_tool_definitions(),
            resources: build_resource_definitions(),
            prompts: build_prompt_definitions(),
            initialized: false,
        }
    }

    /// Process a raw JSON string and return the response JSON string.
    ///
    /// Returns `None` for notifications (requests without an `id`).
    pub fn handle_message(&mut self, input: &str) -> Option<String> {
        let request: JsonRpcRequest = match serde_json::from_str(input) {
            Ok(req) => req,
            Err(e) => {
                let response = JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: Value::Null,
                    result: None,
                    error: Some(JsonRpcError {
                        code: PARSE_ERROR,
                        message: format!("Parse error: {e}"),
                        data: None,
                    }),
                };
                return Some(
                    serde_json::to_string(&response)
                        .expect("JsonRpcResponse serialization should not fail"),
                );
            }
        };

        // Validate jsonrpc version
        if request.jsonrpc != "2.0" {
            let response = JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: request.id.unwrap_or(Value::Null),
                result: None,
                error: Some(JsonRpcError {
                    code: INVALID_REQUEST,
                    message: "Invalid JSON-RPC version (must be \"2.0\")".into(),
                    data: None,
                }),
            };
            return Some(
                serde_json::to_string(&response)
                    .expect("JsonRpcResponse serialization should not fail"),
            );
        }

        // Route the request
        let id = request.id.clone().unwrap_or(Value::Null);
        let result = self.route_method(&request.method, &request.params.unwrap_or(Value::Null));

        // If this was a notification (no id), don't send a response
        request.id.as_ref()?;

        let response = match result {
            Ok(value) => JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: Some(value),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: None,
                error: Some(err),
            },
        };

        Some(
            serde_json::to_string(&response)
                .expect("JsonRpcResponse serialization should not fail"),
        )
    }

    /// Route a method call to the appropriate handler.
    fn route_method(&mut self, method: &str, params: &Value) -> Result<Value, JsonRpcError> {
        match method {
            "initialize" => self.handle_initialize(params),
            "initialized" => {
                // Client acknowledgment notification — no response needed
                Ok(Value::Null)
            }
            "tools/list" => {
                self.require_initialized()?;
                Ok(self.handle_tools_list())
            }
            "tools/call" => {
                self.require_initialized()?;
                self.handle_tools_call(params)
            }
            "resources/list" => {
                self.require_initialized()?;
                Ok(self.handle_resources_list())
            }
            "resources/read" => {
                self.require_initialized()?;
                self.handle_resources_read(params)
            }
            "prompts/list" => {
                self.require_initialized()?;
                Ok(self.handle_prompts_list())
            }
            "prompts/get" => {
                self.require_initialized()?;
                self.handle_prompts_get(params)
            }
            "ping" => Ok(json!({})),
            _ => Err(JsonRpcError {
                code: METHOD_NOT_FOUND,
                message: format!("Method not found: {method}"),
                data: None,
            }),
        }
    }

    /// Ensure the client has completed initialization.
    fn require_initialized(&self) -> Result<(), JsonRpcError> {
        if self.initialized {
            Ok(())
        } else {
            Err(JsonRpcError {
                code: INVALID_REQUEST,
                message: "Server not initialized. Send 'initialize' first.".into(),
                data: None,
            })
        }
    }

    // ── Method Handlers ───────────────────────────────────────────────

    #[expect(
        clippy::unnecessary_wraps,
        reason = "will validate params when protocol grows"
    )]
    fn handle_initialize(&mut self, _params: &Value) -> Result<Value, JsonRpcError> {
        self.initialized = true;

        Ok(json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {
                "tools": { "listChanged": true },
                "resources": {
                    "subscribe": true,
                    "listChanged": true
                },
                "prompts": { "listChanged": true },
                "logging": {},
                "completions": {}
            },
            "serverInfo": {
                "name": "hypercolor",
                "title": "Hypercolor RGB Lighting Controller",
                "version": env!("CARGO_PKG_VERSION"),
                "description": "AI-powered RGB lighting control for Linux. Manage effects, devices, profiles, audio reactivity, screen capture, and automation through natural language.",
                "websiteUrl": "https://github.com/hyperb1iss/hypercolor"
            },
            "instructions": "You are controlling Hypercolor, an RGB lighting system. Use get_status to understand the current setup before making changes. Use list_effects to discover available effects before applying them. Natural language queries work for effect names — you don't need exact IDs. When the user describes a mood or activity, consider using suggest_lighting first, then apply the recommendation."
        }))
    }

    fn handle_tools_list(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "title": t.title,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                    "annotations": {
                        "readOnlyHint": t.read_only,
                        "destructiveHint": false,
                        "idempotentHint": t.idempotent,
                        "openWorldHint": false
                    }
                })
            })
            .collect();

        json!({ "tools": tools })
    }

    #[expect(
        clippy::unused_self,
        reason = "will use self when wired to daemon state"
    )]
    fn handle_tools_call(&self, params: &Value) -> Result<Value, JsonRpcError> {
        let tool_name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| JsonRpcError {
                code: INVALID_PARAMS,
                message: "Missing 'name' parameter in tools/call".into(),
                data: None,
            })?;

        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        match tools::execute_tool(tool_name, &arguments) {
            Ok(result) => Ok(json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| result.to_string())
                }],
                "isError": false
            })),
            Err(e) => Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Error: {e}")
                }],
                "isError": true
            })),
        }
    }

    fn handle_resources_list(&self) -> Value {
        let resources: Vec<Value> = self
            .resources
            .iter()
            .map(|r| {
                json!({
                    "uri": r.uri,
                    "name": r.name,
                    "description": r.description,
                    "mimeType": r.mime_type,
                    "annotations": {
                        "audience": ["assistant"],
                        "priority": r.priority
                    }
                })
            })
            .collect();

        json!({ "resources": resources })
    }

    #[expect(
        clippy::unused_self,
        reason = "will use self when wired to daemon state"
    )]
    fn handle_resources_read(&self, params: &Value) -> Result<Value, JsonRpcError> {
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| JsonRpcError {
                code: INVALID_PARAMS,
                message: "Missing 'uri' parameter in resources/read".into(),
                data: None,
            })?;

        match resources::read_resource(uri) {
            Some(content) => Ok(json!({
                "contents": [{
                    "uri": uri,
                    "mimeType": "application/json",
                    "text": serde_json::to_string(&content)
                        .unwrap_or_else(|_| content.to_string())
                }]
            })),
            None => Err(JsonRpcError {
                code: INVALID_PARAMS,
                message: format!("Resource not found: {uri}"),
                data: None,
            }),
        }
    }

    fn handle_prompts_list(&self) -> Value {
        let prompts: Vec<Value> = self
            .prompts
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "title": p.title,
                    "description": p.description,
                    "arguments": p.arguments.iter().map(|a| json!({
                        "name": a.name,
                        "description": a.description,
                        "required": a.required
                    })).collect::<Vec<_>>()
                })
            })
            .collect();

        json!({ "prompts": prompts })
    }

    #[expect(
        clippy::unused_self,
        reason = "will use self when wired to daemon state"
    )]
    fn handle_prompts_get(&self, params: &Value) -> Result<Value, JsonRpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| JsonRpcError {
                code: INVALID_PARAMS,
                message: "Missing 'name' parameter in prompts/get".into(),
                data: None,
            })?;

        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        prompts::get_prompt_messages(name, &arguments).ok_or_else(|| JsonRpcError {
            code: INVALID_PARAMS,
            message: format!("Prompt not found: {name}"),
            data: None,
        })
    }
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Run the MCP server's stdio loop.
///
/// Reads JSON-RPC messages from stdin (one per line), processes them,
/// and writes responses to stdout. Logs go to stderr.
///
/// # Errors
///
/// Returns an error if reading from stdin or writing to stdout fails.
pub async fn run_stdio_server() -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut server = McpServer::new();
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    tracing::info!("MCP stdio server ready");

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }

        tracing::debug!(input = %line, "MCP request");

        if let Some(response) = server.handle_message(&line) {
            tracing::debug!(output = %response, "MCP response");
            stdout.write_all(response.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }

    tracing::info!("MCP stdio server shutting down");
    Ok(())
}
