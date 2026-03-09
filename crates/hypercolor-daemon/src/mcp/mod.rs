//! MCP (Model Context Protocol) HTTP server for Hypercolor.
//!
//! Exposes Hypercolor tools, resources, and prompt templates over the MCP
//! Streamable HTTP transport using `rmcp`.

pub mod fuzzy;
pub mod prompts;
pub mod resources;
pub mod tools;

use std::future::ready;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use hypercolor_types::config::McpConfig;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{
    ErrorData, ServerHandler,
    model::{
        AnnotateAble, CallToolRequestParams, CallToolResult, GetPromptRequestParams,
        GetPromptResult, Implementation, JsonObject, ListPromptsResult,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        Prompt, PromptArgument, PromptMessage, PromptMessageContent, PromptMessageRole,
        RawResource, ReadResourceRequestParams, ReadResourceResult, ResourceContents, Role,
        ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
    service::{RequestContext, RoleServer},
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::api::AppState;

/// Build the MCP HTTP router mounted at the configured base path.
#[allow(
    clippy::needless_pass_by_value,
    reason = "the router takes shared ownership of app state through cloned Arcs"
)]
pub fn build_router(state: Arc<AppState>, config: &McpConfig) -> Router<Arc<AppState>> {
    let path = normalize_base_path(&config.base_path);
    let service_state = Arc::clone(&state);
    let service: StreamableHttpService<HypercolorMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(HypercolorMcpServer::new(Arc::clone(&service_state))),
            Arc::default(),
            http_config(config),
        );

    Router::new().nest_service(&path, service)
}

fn http_config(config: &McpConfig) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig {
        sse_keep_alive: (config.sse_keep_alive_secs > 0)
            .then_some(Duration::from_secs(config.sse_keep_alive_secs)),
        stateful_mode: config.stateful_mode,
        json_response: config.json_response,
        cancellation_token: CancellationToken::new(),
        ..Default::default()
    }
}

fn normalize_base_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/mcp".to_owned();
    }

    let normalized = if trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        format!("/{trimmed}")
    };

    if normalized.len() > 1 {
        normalized.trim_end_matches('/').to_owned()
    } else {
        normalized
    }
}

#[derive(Clone)]
struct HypercolorMcpServer {
    state: Arc<AppState>,
    tools: Vec<tools::ToolDefinition>,
    resources: Vec<resources::ResourceDefinition>,
    prompts: Vec<prompts::PromptDefinition>,
}

impl HypercolorMcpServer {
    fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            tools: tools::build_tool_definitions(),
            resources: resources::build_resource_definitions(),
            prompts: prompts::build_prompt_definitions(),
        }
    }
}

impl ServerHandler for HypercolorMcpServer {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_prompts()
            .build();

        ServerInfo::new(capabilities)
            .with_server_info(
                Implementation::new("hypercolor", env!("CARGO_PKG_VERSION"))
                    .with_title("Hypercolor RGB Lighting Controller")
                    .with_description(
                        "AI-powered RGB lighting control for Linux with effects, devices, layouts, profiles, scenes, and diagnostics.",
                    )
                    .with_website_url("https://github.com/hyperb1iss/hypercolor"),
            )
            .with_instructions(
                "You are controlling Hypercolor, an RGB lighting system. Start with get_status or the hypercolor://state resource before making changes. Use list_effects to discover the effect catalog before applying visuals. Prefer structured tool arguments and resource reads over guessing current state.",
            )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        ready(Ok(ListToolsResult::with_all_items(
            self.tools.iter().map(tool_to_mcp).collect(),
        )))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools
            .iter()
            .find(|tool| tool.name == name)
            .map(tool_to_mcp)
    }

    #[allow(
        clippy::manual_async_fn,
        reason = "the rmcp trait requires returning an opaque future from the handler"
    )]
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, ErrorData>> + Send + '_ {
        async move {
            let arguments = Value::Object(request.arguments.unwrap_or_default());
            match tools::execute_tool_with_state(request.name.as_ref(), &arguments, &self.state)
                .await
            {
                Ok(payload) => Ok(CallToolResult::structured(payload)),
                Err(error) => Ok(CallToolResult::structured_error(json!({
                    "code": error.error_code(),
                    "message": error.to_string()
                }))),
            }
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        ready(Ok(ListResourcesResult::with_all_items(
            self.resources.iter().map(resource_to_mcp).collect(),
        )))
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, ErrorData>> + Send + '_ {
        ready(Ok(ListResourceTemplatesResult::with_all_items(Vec::new())))
    }

    #[allow(
        clippy::manual_async_fn,
        reason = "the rmcp trait requires returning an opaque future from the handler"
    )]
    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, ErrorData>> + Send + '_ {
        async move {
            let uri = request.uri;
            let Some(definition) = self.resources.iter().find(|resource| resource.uri == uri)
            else {
                return Err(ErrorData::resource_not_found(
                    "Resource not found",
                    Some(json!({ "uri": uri })),
                ));
            };

            let Some(payload) = resources::read_resource_with_state(&uri, &self.state).await else {
                return Err(ErrorData::resource_not_found(
                    "Resource not found",
                    Some(json!({ "uri": uri })),
                ));
            };

            let text = serde_json::to_string(&payload).map_err(|error| {
                ErrorData::internal_error(
                    "Failed to serialize resource payload",
                    Some(json!({
                        "uri": uri,
                        "reason": error.to_string()
                    })),
                )
            })?;

            Ok(ReadResourceResult::new(vec![
                ResourceContents::text(text, uri).with_mime_type(definition.mime_type.clone()),
            ]))
        }
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, ErrorData>> + Send + '_ {
        ready(Ok(ListPromptsResult::with_all_items(
            self.prompts.iter().map(prompt_to_mcp).collect(),
        )))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResult, ErrorData>> + Send + '_ {
        ready(build_prompt_result(request))
    }
}

fn tool_to_mcp(tool: &tools::ToolDefinition) -> Tool {
    Tool::new_with_raw(
        tool.name.clone(),
        Some(tool.description.clone().into()),
        Arc::new(schema_object(&tool.input_schema)),
    )
    .with_title(tool.title.clone())
    .with_raw_output_schema(Arc::new(schema_object(&tool.output_schema)))
    .with_annotations(
        ToolAnnotations::new()
            .read_only(tool.read_only)
            .destructive(false)
            .idempotent(tool.idempotent)
            .open_world(false),
    )
}

fn resource_to_mcp(resource: &resources::ResourceDefinition) -> rmcp::model::Resource {
    RawResource::new(resource.uri.clone(), resource.name.clone())
        .with_title(resource.name.clone())
        .with_description(resource.description.clone())
        .with_mime_type(resource.mime_type.clone())
        .with_audience(vec![Role::Assistant])
        .with_priority(resource.priority)
}

fn prompt_to_mcp(prompt: &prompts::PromptDefinition) -> Prompt {
    let arguments = prompt
        .arguments
        .iter()
        .map(|argument| {
            let mut prompt_argument = PromptArgument::new(argument.name.clone())
                .with_description(argument.description.clone());
            if argument.required {
                prompt_argument = prompt_argument.with_required(true);
            }
            prompt_argument
        })
        .collect::<Vec<_>>();

    Prompt::new(
        prompt.name.clone(),
        Some(prompt.description.clone()),
        Some(arguments),
    )
    .with_title(prompt.title.clone())
}

fn build_prompt_result(request: GetPromptRequestParams) -> Result<GetPromptResult, ErrorData> {
    let name = request.name;
    let arguments = Value::Object(request.arguments.unwrap_or_default());

    let payload = prompts::get_prompt_messages(&name, &arguments).ok_or_else(|| {
        ErrorData::invalid_params("Prompt not found", Some(json!({ "name": name })))
    })?;

    let description = payload
        .get("description")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ErrorData::internal_error(
                "Prompt payload missing messages array",
                Some(json!({ "name": name })),
            )
        })?
        .iter()
        .map(prompt_message_from_value)
        .collect::<Result<Vec<_>, _>>()?;

    let mut result = GetPromptResult::new(messages);
    if let Some(description) = description {
        result = result.with_description(description);
    }
    Ok(result)
}

fn prompt_message_from_value(value: &Value) -> Result<PromptMessage, ErrorData> {
    let Some(message) = value.as_object() else {
        return Err(ErrorData::internal_error(
            "Prompt message was not an object",
            Some(json!({ "message": value })),
        ));
    };

    let role = match message.get("role").and_then(Value::as_str) {
        Some("user") => PromptMessageRole::User,
        Some("assistant") => PromptMessageRole::Assistant,
        Some(other) => {
            return Err(ErrorData::internal_error(
                "Prompt message used an unsupported role",
                Some(json!({ "role": other })),
            ));
        }
        None => {
            return Err(ErrorData::internal_error(
                "Prompt message missing role",
                Some(json!({ "message": value })),
            ));
        }
    };

    let Some(content) = message.get("content").and_then(Value::as_object) else {
        return Err(ErrorData::internal_error(
            "Prompt message missing content object",
            Some(json!({ "message": value })),
        ));
    };

    match content.get("type").and_then(Value::as_str) {
        Some("text") => {
            let Some(text) = content.get("text").and_then(Value::as_str) else {
                return Err(ErrorData::internal_error(
                    "Prompt text content missing text field",
                    Some(json!({ "message": value })),
                ));
            };
            Ok(PromptMessage::new_text(role, text))
        }
        Some("resource") => {
            let Some(resource) = content.get("resource").and_then(Value::as_object) else {
                return Err(ErrorData::internal_error(
                    "Prompt resource content missing resource object",
                    Some(json!({ "message": value })),
                ));
            };
            let Some(uri) = resource.get("uri").and_then(Value::as_str) else {
                return Err(ErrorData::internal_error(
                    "Prompt resource content missing uri",
                    Some(json!({ "message": value })),
                ));
            };

            let mut link = RawResource::new(uri.to_owned(), uri.to_owned()).with_title(uri);
            if let Some(mime_type) = resource.get("mimeType").and_then(Value::as_str) {
                link = link.with_mime_type(mime_type);
            }

            Ok(PromptMessage::new(
                role,
                PromptMessageContent::resource_link(link.no_annotation()),
            ))
        }
        Some(other) => Err(ErrorData::internal_error(
            "Prompt message used an unsupported content type",
            Some(json!({ "type": other })),
        )),
        None => Err(ErrorData::internal_error(
            "Prompt message content missing type",
            Some(json!({ "message": value })),
        )),
    }
}

fn schema_object(value: &Value) -> JsonObject {
    match value.as_object() {
        Some(object) => object.clone(),
        None => JsonObject::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_base_path;

    #[test]
    fn normalize_base_path_uses_default_for_empty_values() {
        assert_eq!(normalize_base_path(""), "/mcp");
        assert_eq!(normalize_base_path("   "), "/mcp");
        assert_eq!(normalize_base_path("/"), "/mcp");
    }

    #[test]
    fn normalize_base_path_adds_leading_slash_and_trims_trailing_slash() {
        assert_eq!(normalize_base_path("mcp"), "/mcp");
        assert_eq!(normalize_base_path("/mcp/"), "/mcp");
        assert_eq!(normalize_base_path("/nested/mcp///"), "/nested/mcp");
    }
}
