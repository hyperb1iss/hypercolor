#![allow(clippy::needless_for_each)]

use axum::routing::MethodRouter;
use utoipa::openapi::path::{OperationBuilder, ParameterBuilder, ParameterIn, Paths};
use utoipa::openapi::request_body::RequestBodyBuilder;
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{
    Content, HttpMethod, OpenApi as OpenApiDocument, Ref, Required, Response, ResponseBuilder,
};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::UtoipaMethodRouter;
use utoipa_swagger_ui::SwaggerUi;

use crate::api::envelope;

#[derive(OpenApi)]
#[openapi(
    components(
        schemas(
            envelope::Meta,
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
    Binary(&'static str),
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
    success: ResponseShape,
    request: Option<(SchemaName, bool)>,
    response_schema: SchemaRegistrar,
    request_schema: Option<SchemaRegistrar>,
    additional_schemas: Vec<SchemaRegistrar>,
    query: Option<ParameterProvider>,
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
            success: ResponseShape::Enveloped(schema_name::<T>),
            request: None,
            response_schema: register_schema::<T>,
            request_schema: None,
            additional_schemas: Vec::new(),
            query: None,
        }
    }

    pub(crate) fn raw(mut self) -> Self {
        self.success = ResponseShape::Json(match self.success {
            ResponseShape::Enveloped(schema) | ResponseShape::Json(schema) => schema,
            ResponseShape::Binary(_) | ResponseShape::Empty => schema_name::<serde_json::Value>,
        });
        self
    }

    pub(crate) fn binary(mut self, content_type: &'static str) -> Self {
        self.success = ResponseShape::Binary(content_type);
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

    pub(crate) fn body<T: utoipa::ToSchema>(mut self) -> Self {
        self.request = Some((schema_name::<T>, true));
        self.request_schema = Some(register_schema::<T>);
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

    fn component<T: utoipa::ToSchema>(mut self) -> Self {
        self.additional_schemas.push(register_schema::<T>);
        self
    }
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
    let mut builder = OperationBuilder::new()
        .tag(document.tag)
        .summary(Some(document.summary))
        .operation_id(Some(document.operation_id))
        .response(
            document.success_status,
            success_response(document.summary, document.success),
        );
    if let Some(status) = document.alternate_success_status {
        builder = builder.response(status, success_response(document.summary, document.success));
    }

    for status in [
        "400", "401", "403", "404", "409", "412", "422", "429", "500",
    ] {
        builder = builder.response(status, error_response(status));
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

    if let Some((schema, required)) = document.request {
        builder = builder.request_body(Some(
            RequestBodyBuilder::new()
                .required(Some(if required {
                    Required::True
                } else {
                    Required::False
                }))
                .content(
                    "application/json",
                    Content::new(Some(Ref::from_schema_name(schema()))),
                )
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
        ResponseShape::Binary(content_type) => response
            .content(
                content_type,
                Content::new(Some(ObjectBuilder::new().schema_type(Type::String).format(
                    Some(utoipa::openapi::SchemaFormat::KnownFormat(
                        utoipa::openapi::KnownFormat::Binary,
                    )),
                ))),
            )
            .build(),
        ResponseShape::Empty => response.build(),
    }
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
