#![allow(clippy::needless_for_each)]

use axum::routing::MethodRouter;
use utoipa::openapi::path::{OperationBuilder, Parameter, ParameterBuilder, ParameterIn, Paths};
use utoipa::openapi::request_body::RequestBodyBuilder;
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{
    Content, HttpMethod, OpenApi as OpenApiDocument, Ref, RefOr, Required, Response,
    ResponseBuilder,
};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::UtoipaMethodRouter;
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    components(
        schemas(
            hypercolor_types::api::envelope::ResponseMeta,
            hypercolor_types::api::envelope::ApiErrorDetail,
            hypercolor_types::api::envelope::ApiErrorBody,
        )
    ),
    tags(
        (name = "system", description = "Daemon identity, health, and status"),
        (name = "drivers", description = "Driver module inventory and capabilities"),
        (name = "devices", description = "Tracked device inventory"),
        (name = "controls", description = "Generic control surfaces and typed value mutation"),
        (name = "effects", description = "Effect catalog and runtime control"),
        (name = "assets", description = "Uploaded media assets"),
        (name = "displays", description = "Display devices, faces, and simulators"),
        (name = "attachments", description = "Physical attachment templates and bindings"),
        (name = "output", description = "Global output power and brightness"),
        (name = "scenes", description = "Scene CRUD and activation"),
        (name = "layouts", description = "Spatial layout CRUD and preview"),
        (name = "library", description = "Favorites, presets, and playlists"),
        (name = "capture", description = "Protected host input and screen-capture actions"),
        (name = "config", description = "Daemon configuration inspection and mutation"),
        (name = "diagnostics", description = "Daemon diagnostics"),
        (name = "websocket", description = "Realtime WebSocket endpoint"),
    ),
    modifiers(&SecurityAddon)
)]
pub(crate) struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi
            .components
            .get_or_insert_with(utoipa::openapi::Components::new);
        components.add_security_scheme(
            "bearer_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("API key")
                    .build(),
            ),
        );
    }
}

type SchemaName = fn() -> std::borrow::Cow<'static, str>;
type SchemaRegistrar = fn(&mut Vec<(String, utoipa::openapi::RefOr<Schema>)>);
type ParameterProvider = fn() -> Vec<utoipa::openapi::path::Parameter>;

fn schema_name<T: utoipa::ToSchema>() -> std::borrow::Cow<'static, str> {
    let name = T::name();
    match name.as_ref() {
        "Vec" => generic_schema_name::<T>("List"),
        "ListResponse" => generic_schema_name::<T>("ListResponse"),
        "Option" => generic_schema_name::<T>("Optional"),
        _ => name,
    }
}

fn register_schema<T: utoipa::ToSchema>(
    schemas: &mut Vec<(String, utoipa::openapi::RefOr<Schema>)>,
) {
    schemas.push((schema_name::<T>().into_owned(), T::schema()));
    T::schemas(schemas);
}

fn query_parameters<T: utoipa::IntoParams>() -> Vec<utoipa::openapi::path::Parameter> {
    T::into_params(|| Some(ParameterIn::Query))
}

fn generic_schema_name<T>(suffix: &str) -> std::borrow::Cow<'static, str> {
    let rust_name = std::any::type_name::<T>();
    let inner = rust_name
        .split_once('<')
        .and_then(|(_, inner)| inner.strip_suffix('>'))
        .unwrap_or(rust_name);
    let item = inner
        .rsplit("::")
        .next()
        .unwrap_or(inner)
        .trim_end_matches('>');
    std::borrow::Cow::Owned(format!("{item}{suffix}"))
}

#[derive(Clone, Copy)]
pub(crate) enum ResponseShape {
    Enveloped(SchemaName),
    Json(SchemaName),
    /// Raw bytes in any of these media types.
    Binary(&'static [&'static str]),
    Empty,
}

#[derive(Clone)]
pub(crate) struct OperationDoc {
    method: HttpMethod,
    operation_id: &'static str,
    tag: &'static str,
    summary: &'static str,
    success_status: &'static str,
    alternate_success_status: Option<&'static str>,
    /// Answers `304 Not Modified` with no body to a matching validator.
    not_modified: bool,
    success: ResponseShape,
    request: Option<(SchemaName, bool)>,
    request_media_type: &'static str,
    request_binary: bool,
    response_schema: SchemaRegistrar,
    request_schema: Option<SchemaRegistrar>,
    additional_schemas: Vec<SchemaRegistrar>,
    query: Option<ParameterProvider>,
    headers: Vec<Parameter>,
    idempotent: bool,
}

impl OperationDoc {
    pub(crate) fn get<T: utoipa::ToSchema>(
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new::<T>(HttpMethod::Get, operation_id, tag, summary)
    }

    pub(crate) fn get_list<T: utoipa::ToSchema + utoipa::__dev::ComposeSchema>(
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new::<hypercolor_types::api::ListResponse<T>>(
            HttpMethod::Get,
            operation_id,
            tag,
            summary,
        )
        .component::<T>()
    }

    pub(crate) fn get_vec<T: utoipa::ToSchema + utoipa::__dev::ComposeSchema>(
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new::<Vec<T>>(HttpMethod::Get, operation_id, tag, summary).component::<T>()
    }

    pub(crate) fn post<T: utoipa::ToSchema>(
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new::<T>(HttpMethod::Post, operation_id, tag, summary)
    }

    pub(crate) fn put<T: utoipa::ToSchema>(
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new::<T>(HttpMethod::Put, operation_id, tag, summary)
    }

    pub(crate) fn patch<T: utoipa::ToSchema>(
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new::<T>(HttpMethod::Patch, operation_id, tag, summary)
    }

    pub(crate) fn delete<T: utoipa::ToSchema>(
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new::<T>(HttpMethod::Delete, operation_id, tag, summary)
    }

    fn new<T: utoipa::ToSchema>(
        method: HttpMethod,
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self {
            method,
            operation_id,
            tag,
            summary,
            success_status: "200",
            alternate_success_status: None,
            not_modified: false,
            success: ResponseShape::Enveloped(schema_name::<T>),
            request: None,
            request_media_type: "application/json",
            request_binary: false,
            response_schema: register_schema::<T>,
            request_schema: None,
            additional_schemas: Vec::new(),
            query: None,
            headers: Vec::new(),
            idempotent: false,
        }
    }

    pub(crate) fn raw(mut self) -> Self {
        self.success = ResponseShape::Json(match self.success {
            ResponseShape::Enveloped(schema) | ResponseShape::Json(schema) => schema,
            ResponseShape::Binary(_) | ResponseShape::Empty => schema_name::<serde_json::Value>,
        });
        self
    }

    pub(crate) fn binary(mut self, content_types: &'static [&'static str]) -> Self {
        self.success = ResponseShape::Binary(content_types);
        self
    }

    pub(crate) fn empty(mut self) -> Self {
        self.success = ResponseShape::Empty;
        self
    }

    pub(crate) fn status(mut self, status: &'static str) -> Self {
        self.success_status = status;
        self
    }

    pub(crate) fn also_status(mut self, status: &'static str) -> Self {
        self.alternate_success_status = Some(status);
        self
    }

    /// Document the bodiless `304` a conditional `GET` can get.
    pub(crate) fn not_modified(mut self) -> Self {
        self.not_modified = true;
        self
    }

    pub(crate) fn body<T: utoipa::ToSchema>(mut self) -> Self {
        self.request = Some((schema_name::<T>, true));
        self.request_schema = Some(register_schema::<T>);
        self
    }

    /// A required `multipart/form-data` body whose parts `T` describes.
    pub(crate) fn multipart_body<T: utoipa::ToSchema>(mut self) -> Self {
        self = self.body::<T>();
        self.request_media_type = "multipart/form-data";
        self
    }

    /// A streamed archive body, kept outside JSON and multipart buffering.
    pub(crate) fn binary_body(mut self) -> Self {
        self = self.body::<String>();
        self.request_media_type = "application/octet-stream";
        self.request_binary = true;
        self
    }

    pub(crate) fn optional_body<T: utoipa::ToSchema>(mut self) -> Self {
        self.request = Some((schema_name::<T>, false));
        self.request_schema = Some(register_schema::<T>);
        self
    }

    pub(crate) fn query<T: utoipa::IntoParams>(mut self) -> Self {
        self.query = Some(query_parameters::<T>);
        self
    }

    /// Document the optional scene fence this operation honors as
    /// `If-Match`.
    pub(crate) fn fenced(mut self) -> Self {
        self.headers
            .push(fence_header(Required::False, OPTIONAL_FENCE));
        self
    }

    /// Document the scene fence this operation requires as `If-Match`.
    pub(crate) fn requires_fence(mut self) -> Self {
        self.headers
            .push(fence_header(Required::True, REQUIRED_FENCE));
        self
    }

    /// Honor an optional `Idempotency-Key` on this operation. Declaring the
    /// header here is what turns the behavior on (`api::idempotency`), so
    /// the document and the router cannot disagree about which writes
    /// replay instead of applying twice.
    pub(crate) fn idempotent(mut self) -> Self {
        self.headers.push(
            ParameterBuilder::new()
                .name(super::idempotency::DOCUMENTED_HEADER)
                .parameter_in(ParameterIn::Header)
                .required(Required::False)
                .description(Some(
                    "Client-generated opaque key (a UUID) naming one logical write. A repeat \
                     with the same key and the same request is not applied again: it answers \
                     with the stored status and body plus `Idempotency-Replayed: true`, waiting \
                     for the original when it is still running. The same key on a different \
                     request answers 422 `idempotency_key_mismatch`. Keys are kept in memory \
                     for one daemon run, which the `hypercolor-run-epoch` response header names.",
                ))
                .schema(Some(
                    ObjectBuilder::new()
                        .schema_type(Type::String)
                        .max_length(Some(
                            hypercolor_types::api::envelope::IDEMPOTENCY_KEY_MAX_LEN,
                        )),
                ))
                .build(),
        );
        self.idempotent = true;
        self
    }

    pub(crate) fn component<T: utoipa::ToSchema>(mut self) -> Self {
        self.additional_schemas.push(register_schema::<T>);
        self
    }
}

/// What an optional scene `If-Match` fence is and how it is checked.
const OPTIONAL_FENCE: &str = "Scene fence guarding the write: the ETag a read or a write served, \
\"<run_epoch>:<revision>\". It passes only while both the daemon run and the scene revision are \
current, so a fence from an earlier daemon run is always stale. A bare revision (\"42\" or 42) \
is still accepted and checks the revision alone; only the epoch-qualified form is safe across \
daemon restarts. `*` or no header applies without a precondition.";

/// [`OPTIONAL_FENCE`], for an operation that cannot run unfenced.
const REQUIRED_FENCE: &str = "Scene fence guarding the write: the ETag a read or a write served, \
\"<run_epoch>:<revision>\". It passes only while both the daemon run and the scene revision are \
current, so a fence from an earlier daemon run is always stale. A bare revision (\"42\" or 42) \
is still accepted and checks the revision alone; only the epoch-qualified form is safe across \
daemon restarts. Required; the wildcard * is not accepted.";

fn fence_header(required: Required, description: &'static str) -> Parameter {
    ParameterBuilder::new()
        .name("If-Match")
        .parameter_in(ParameterIn::Header)
        .required(required)
        .description(Some(description))
        .schema(Some(ObjectBuilder::new().schema_type(Type::String)))
        .build()
}

pub(crate) fn documented_route<S, const N: usize>(
    path: &'static str,
    method_router: MethodRouter<S>,
    operations: [OperationDoc; N],
) -> UtoipaMethodRouter<S>
where
    S: Send + Sync + Clone + 'static,
{
    let mut paths = Paths::new();
    let mut schemas = Vec::new();
    for document in operations {
        (document.response_schema)(&mut schemas);
        if let Some(register) = document.request_schema {
            register(&mut schemas);
        }
        for register in &document.additional_schemas {
            register(&mut schemas);
        }
        let method = document.method.clone();
        paths.add_path_operation(path, vec![method], operation(path, document));
    }
    (schemas, paths, method_router)
}

fn operation(path: &str, document: OperationDoc) -> utoipa::openapi::path::Operation {
    let replayed = document.idempotent;
    let mut builder = OperationBuilder::new()
        .tag(document.tag)
        .summary(Some(document.summary))
        .operation_id(Some(document.operation_id))
        .response(
            document.success_status,
            with_replay_header(
                success_response(document.summary, document.success),
                replayed,
            ),
        );
    if let Some(status) = document.alternate_success_status {
        builder = builder.response(
            status,
            with_replay_header(
                success_response(document.summary, document.success),
                replayed,
            ),
        );
    }
    if document.not_modified {
        builder = builder.response(
            "304",
            ResponseBuilder::new()
                .description("Not modified: the copy the client holds is current")
                .build(),
        );
    }

    for status in [
        "400", "401", "403", "404", "409", "412", "422", "429", "500",
    ] {
        builder = builder.response(status, with_replay_header(error_response(status), replayed));
    }
    if replayed {
        // Never recorded, so never replayed: no `Idempotency-Replayed`.
        builder = builder.response("503", keyed_capacity_response());
    }

    for name in path_parameter_names(path) {
        builder = builder.parameter(
            ParameterBuilder::new()
                .name(name)
                .parameter_in(ParameterIn::Path)
                .required(Required::True)
                .schema(Some(ObjectBuilder::new().schema_type(Type::String)))
                .build(),
        );
    }

    if let Some(query) = document.query {
        for parameter in query() {
            builder = builder.parameter(parameter);
        }
    }
    for header in document.headers {
        builder = builder.parameter(header);
    }

    if let Some((schema, required)) = document.request {
        let content = if document.request_binary {
            Content::new(Some(ObjectBuilder::new().schema_type(Type::String).format(
                Some(utoipa::openapi::SchemaFormat::KnownFormat(
                    utoipa::openapi::KnownFormat::Binary,
                )),
            )))
        } else {
            Content::new(Some(Ref::from_schema_name(schema())))
        };
        builder = builder.request_body(Some(
            RequestBodyBuilder::new()
                .required(Some(if required {
                    Required::True
                } else {
                    Required::False
                }))
                .content(document.request_media_type, content)
                .build(),
        ));
    }

    builder.build()
}

fn success_response(summary: &str, shape: ResponseShape) -> Response {
    let response = ResponseBuilder::new().description(format!("{summary} response"));
    match shape {
        ResponseShape::Enveloped(schema) => response
            .content(
                "application/json",
                Content::new(Some(
                    ObjectBuilder::new()
                        .property("data", Ref::from_schema_name(schema()))
                        .required("data")
                        .property("meta", Ref::from_schema_name("ResponseMeta"))
                        .required("meta"),
                )),
            )
            .build(),
        ResponseShape::Json(schema) => response
            .content(
                "application/json",
                Content::new(Some(Ref::from_schema_name(schema()))),
            )
            .build(),
        ResponseShape::Binary(content_types) => content_types
            .iter()
            .fold(response, |response, content_type| {
                response.content(
                    *content_type,
                    Content::new(Some(ObjectBuilder::new().schema_type(Type::String).format(
                        Some(utoipa::openapi::SchemaFormat::KnownFormat(
                            utoipa::openapi::KnownFormat::Binary,
                        )),
                    ))),
                )
            })
            .build(),
        ResponseShape::Empty => response.build(),
    }
}

/// Declare `Idempotency-Replayed` on a response an opted-in operation can
/// replay.
fn with_replay_header(mut response: Response, replayed: bool) -> Response {
    if replayed {
        response.headers.insert(
            hypercolor_types::api::envelope::IDEMPOTENCY_REPLAYED_HEADER.to_owned(),
            RefOr::T(
                utoipa::openapi::header::HeaderBuilder::new()
                    .schema(Some(ObjectBuilder::new().schema_type(Type::String)))
                    .description(Some(
                        "`true` when this answer was stored for an earlier request with the same \
                         Idempotency-Key and replayed instead of applied again; absent otherwise.",
                    ))
                    .build(),
            ),
        );
    }
    response
}

fn error_response(status: &str) -> Response {
    let description = match status {
        "400" => "Malformed request",
        "401" => "Authentication required",
        "403" => "Insufficient permission",
        "404" => "Resource not found",
        "409" => "State conflict",
        "412" => "Precondition failed",
        "422" => "Validation failed",
        "429" => "Request rate exceeded",
        "500" => "Internal daemon error",
        _ => "Request failed",
    };
    ResponseBuilder::new()
        .description(description)
        .content(
            "application/json",
            Content::new(Some(Ref::from_schema_name("ApiErrorBody"))),
        )
        .build()
}

/// The refusal a keyed write gets when the daemon takes nothing on for it:
/// too many keyed writes are in flight (`api::idempotency`), or the daemon
/// is still starting. Nothing is recorded for its key either way.
fn keyed_capacity_response() -> Response {
    ResponseBuilder::new()
        .description(
            "`service_unavailable`: the daemon took nothing on for this request, because too \
             many keyed writes are already in flight or it is still starting. Nothing was applied \
             or recorded for this Idempotency-Key, so send the same request again with the same \
             key.",
        )
        .content(
            "application/json",
            Content::new(Some(Ref::from_schema_name("ApiErrorBody"))),
        )
        .build()
}

fn path_parameter_names(path: &str) -> Vec<&str> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .collect()
}

pub(crate) fn base_document() -> OpenApiDocument {
    ApiDoc::openapi()
}

pub(crate) fn swagger(openapi: OpenApiDocument) -> SwaggerUi {
    SwaggerUi::new("/api/v1/docs").url("/api/v1/openapi.json", openapi)
}

pub fn document_json_pretty() -> serde_json::Result<String> {
    serde_json::to_string_pretty(&crate::api::openapi_document())
}
