# API Patterns Reference

Detailed patterns for the Hypercolor daemon REST API.

## Handler Pattern

All handlers receive `State<Arc<AppState>>` via Axum extractor and return `Response` (not `Result`). A handler is a thin adapter over one domain service reached through `state.domains`: convert wire input, call the service, wrap the outcome. It never locks a subsystem mutex, and it leaves event publication to the service, which knows when the commit landed. The config routes follow the same rule: `ConfigManager` publishes `ConfigChanged` once a save lands, so `api/config.rs` never publishes on its own.

```rust
// src/api/output.rs
use crate::api::envelope;
use crate::domain;

pub async fn get_output(State(state): State<Arc<AppState>>) -> Response {
    envelope::ok(domain::output::get_output(&state.domains.output))
}

pub async fn patch_output(
    State(state): State<Arc<AppState>>,
    Json(request): Json<OutputPatchRequest>,
) -> Response {
    match domain::output::patch_output(&state.domains.output, request).await {
        Ok(outcome) => envelope::ok(outcome.output),
        Err(error) => error.into_response(),
    }
}
```

`AppState::domains` is a `DomainContexts` (`src/domain/context.rs`), the composition root for eleven authorities: `runtime_session`, `devices`, `scene`, `layout`, `output`, `platform`, `display`, `diagnostics`, `effects`, `scene_tree`, `scene_library`. Each owns its own commit ordering, persistence, and event publication.

**Mutation provenance** rides beside the command, not inside it. Every trigger-bearing mutation takes a `MutationContext`: `MutationContext::api()` from REST, `MutationContext::mcp()` from MCP. Commands with no trigger-bearing canonical event take no context at all.

**Success envelope.** The wire types live in `hypercolor_types::api::envelope`; the daemon-side constructors are free functions in `crate::api::envelope`, not associated functions on `ApiResponse`.

```rust
// hypercolor_types::api::envelope
pub struct ApiResponse<T> {
    pub data: T,
    pub meta: ResponseMeta,
}

pub struct ResponseMeta {
    pub api_version: String,    // "1.0"
    pub request_id: String,     // "req_{uuid_v7}"
    pub timestamp: String,      // ISO 8601 UTC with ms precision
}
```

Constructors: `envelope::ok(data)` (200), `envelope::created(data)` (201), `envelope::accepted(data)` (202) -- all return `Response`.

## Shared Wire Contracts

`hypercolor_types::api` is the single definition of the REST request and response types across seventeen domain modules (assets, attachments, capture, config, controls, devices, diagnose, displays, drivers, effects, layouts, library, output, scene, scenes, simulators, system) plus `envelope`. The daemon serializes them; the web UI, the TUI, the CLI, and the generated Python client deserialize the same types. Adding or changing an endpoint in a shared domain means changing the type there, never mirroring it in the daemon.

Most list routes answer `ListResponse<T>`:

```rust
pub struct ListResponse<T> {
    pub items: Vec<T>,
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<PageInfo>,
}

pub struct PageInfo {
    pub offset: u64,
    pub limit: u64,
    pub has_more: bool,
}
```

`page: None` means the response is complete rather than faking a paging contract the route does not honor. Domain aliases name the concrete shape, so `EffectListResponse` is `ListResponse<EffectSummary>`.

Five list routes are outside that shape, and a client that assumes `items` on them fails to deserialize:

| Route                  | `data` shape                              | Declared as             |
| ---------------------- | ----------------------------------------- | ----------------------- |
| `/displays`            | bare JSON array                           | `OperationDoc::get_vec` |
| `/capture/monitors`    | bare JSON array                           | `OperationDoc::get_vec` |
| `/config/schema`       | bare JSON array                           | `OperationDoc::get_vec` |
| `/simulators/displays` | bare JSON array                           | `OperationDoc::get_vec` |
| `/control-surfaces`    | `ControlSurfaceListResponse { surfaces }` | `OperationDoc::get`     |

`get_list` is what produces the `{ items, total, page }` envelope; `get_vec` wraps a plain `Vec<T>`. Read the route's `OperationDoc` before writing a client type.

## Effect Application Flow

`POST /api/v1/effects/{id}/apply` triggers:

1. Parse the optional `If-Match` revision and the optional `ApplyEffectRequest` body.
2. Resolve the effect: `state.domains.effects.resolve_for_mutation(&id)` returns a `ResolvedEffect` bound to the catalog generation that produced it, or `None` for a 404. It accepts an exact UUID, or a case-insensitive name whose punctuation and spacing normalize away. It is not fuzzy and it does not match substrings.
3. Refuse a `Display`-category effect, which belongs on a display device rather than the LED pipeline.
4. Resolve any `preset_id` and seed controls from it when the request named none.
5. Call `domain::effect::apply_effect(&state.domains.effects, ApplyEffect { .. }, MutationContext::api())`. The service admits the controls, commits the scene tree, and wakes output. `EffectStarted` is queued on the scene mutation and published when the commit lands.
6. Wrap the outcome with `envelope::ok`.

The renderer is not constructed here. `create_renderer_for_metadata()` (`core/src/effect/factory.rs`) is called by the render thread's effect pool (`core/src/effect/pool.rs`) when the committed scene tree next needs a slot, and frames come from `EffectRenderer::render_into(&mut self, &FrameInput<'_>, &mut Canvas)`.

## Control Update Flow

`PATCH /api/v1/scene/zones/{zone}/layers/{layer}/controls` with a
`PatchControlsRequest`:

1. Parse both path segments as strict UUIDs. A non-UUID is a 404 (`zone_not_found` / `layer_not_found`), not a name lookup: name and substring resolution is an MCP adapter concern, never REST.
2. Collect `values` into a `HashMap<String, ControlValue>`.
3. Call `scene_tree::patch_layer_controls(&state.domains.scene_tree, PatchLayerControls { .. }, MutationContext::api())`. The service validates the values, applies the binding clears, and commits atomically.
4. One `EffectControlChanged` is queued per value that actually changed, each carrying zone and layer identity, and published when the commit lands.
5. Wrap the written zone with `envelope::ok` and attach the new revision.

A patch naming a control an input binding already drives is refused 409 `control_bound` unless the same request clears that binding.

## Error Handling

`DomainError` in `src/domain/mod.rs` is the daemon's one error type, and the `IntoResponse` impl in `src/api/error.rs` is its one REST rendering. The split is deliberate: the error type stays transport free so each transport can own exactly one projection. Services and helpers return `Result<T, DomainError>`; handlers return `Response`, so they `?` inside a helper or match and call `.into_response()`:

```rust
fn parse_simulator_id(raw: &str) -> Result<DeviceId, DomainError> {
    raw.parse::<DeviceId>()
        .map_err(|_| DomainError::validation(format!("Invalid simulator id: {raw}")))
}

pub async fn delete_simulated_display(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match parse_simulator_id(&id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    // ... service call, then envelope::ok(outcome)
}
```

Never hand a `Response` back through a `Result` and never hand-build an error body. Both grow a second error surface, and `tests/api_error_surface_tests.rs` scans every file under `src/` for them.

### Variants

```rust
pub enum DomainError {
    NotFound { kind: ResourceKind, id: String },
    Validation { message: String, field: Option<String>, details: Option<DomainErrorDetails> },
    Malformed { message: String },
    Conflict { message: String, details: Option<DomainErrorDetails> },
    ControlBound { keys: Vec<String> },
    Unauthorized { message: String },
    Forbidden { message: String, details: Option<DomainErrorDetails> },
    PayloadTooLarge { limit_bytes: u64 },
    UnsupportedMediaType { message: String },
    RateLimited { message: String, limit: u32, window_seconds: u64, retry_after_secs: u64 },
    PreconditionFailed { resource: ResourceKind, expected: u64, current: u64 },
    DeviceUnavailable { device_id: DeviceId, reason: String },
    ServiceUnavailable { message: String, details: Option<DomainErrorDetails> },
    Internal(anyhow::Error),
}
```

Constructors cover the common shapes: `not_found(kind, id)`, `validation(msg)`, `validation_field(field, msg)`, `validation_details(msg, details)`, `malformed(msg)`, `conflict(msg)`, `conflict_details(msg, details)`, `unauthorized(msg)`, `forbidden(msg)`, `forbidden_details(msg, details)`, `unsupported_media_type(msg)`, `service_unavailable_details(msg, details)`. The remaining variants carry structured fields and are built directly.

`ControlBound` is the Spec 78 §1.6 refusal an agent meets whenever it writes a control an input binding already drives. It renders 409 `control_bound` with `details: { "bound": [...] }`, and it is recoverable in the same shape: drop the key, or clear its binding in the same request.

### Structured details

The `details` payload is `Option<DomainErrorDetails>`, a closed enum, not free-form `serde_json::Value`. Thirteen variants: `SceneCommitSuperseded`, `EffectIdMigrationSuperseded`, `EffectResolutionSuperseded`, `MediaAdmission`, `Zone`, `Layer`, `Member`, `UnknownMember`, `MemberCount`, `Errors`, `Segments`, `RejectedControls`, and `Adapter`. A service raising one and a service branching on one agree at compile time; the transport projection in `src/api/error.rs` turns them into wire JSON. `Adapter(serde_json::Value)` is the escape hatch for transport-shaped context, and domain modules cannot reach it because `internal_api_surface_tests` fences `serde_json` out of their sources. `impl From<serde_json::Value> for DomainErrorDetails` exists, so a `json!` literal at an adapter call site still compiles into `Adapter`.

`ResourceKind` names what an error is about, and it has exactly 25 variants: `Scene`, `Zone`, `Layer`, `Effect`, `Device`, `Display`, `DisplayFrame`, `SimulatedDisplay`, `Driver`, `AttachmentProfile`, `Layout`, `Preset`, `Playlist`, `Favorite`, `Asset`, `AttachmentTemplate`, `AttachmentSlot`, `Control`, `ControlSurface`, `Diagnostic`, `Sensor`, `Config`, `ConfigKey`, `Session`, `Route`. It renders lowercase, so not-found messages derive as `"scene not found: default"` instead of being written out by hand at each call site.

### Codes and statuses

| Variant                | Code                     | Status |
| ---------------------- | ------------------------ | ------ |
| `Malformed`            | `malformed_request`      | 400    |
| `Unauthorized`         | `unauthorized`           | 401    |
| `Forbidden`            | `forbidden`              | 403    |
| `NotFound`             | `{kind}_not_found`       | 404    |
| `Conflict`             | `conflict`               | 409    |
| `ControlBound`         | `control_bound`          | 409    |
| `PreconditionFailed`   | `precondition_failed`    | 412    |
| `PayloadTooLarge`      | `payload_too_large`      | 413    |
| `UnsupportedMediaType` | `unsupported_media_type` | 415    |
| `Validation`           | `validation_error`       | 422    |
| `RateLimited`          | `rate_limited`           | 429    |
| `Internal`             | `internal_error`         | 500    |
| `DeviceUnavailable`    | `device_unavailable`     | 503    |
| `ServiceUnavailable`   | `service_unavailable`    | 503    |

`NotFound` never emits the bare string `not_found`. Its code comes from `ResourceKind::not_found_code()`, one per kind: `scene_not_found`, `zone_not_found`, `effect_not_found`, `device_not_found`, and so on through all 25. A client matching on `"not_found"` never fires.

### Wire shape

```json
{
  "error": {
    "code": "validation_error",
    "message": "zone name must not be empty",
    "details": { "field": "name" }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_0192f3c1-...",
    "timestamp": "2026-03-29T12:00:00.000Z"
  }
}
```

The body types are `ApiErrorBody` and `ApiErrorDetail` in `hypercolor_types::api::envelope`. `details` carries `skip_serializing_if = "Option::is_none"`, so the key is absent rather than `null` when a variant has no structured context to hand back.

`Internal` always renders `"internal error"` on the wire; the full chain goes to `tracing::error!`. `PreconditionFailed` renders `details: { "expected": N, "current": M }` and also attaches an `ETag` header carrying `current`, so a client rebases off `error.details.current` without a second read.

## Route Registration

Handlers live in `src/api/<domain>.rs`. Registration lives separately in `src/api/routes/<domain>.rs`, one `utoipa_axum` `OpenApiRouter` sub-router per domain, each exporting `pub(super) fn router()`. Every entry goes through `openapi::documented_route`, which pairs the method router with the `OperationDoc` values that describe it:

```rust
// src/api/routes/effects.rs
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/effects",
            axum::routing::get(effects::list_effects),
            [
                OperationDoc::get_list::<hypercolor_types::api::effects::EffectSummary>(
                    "list_effects",
                    "effects",
                    "List effects",
                )
                .query::<effects::EffectListQuery>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/effects/{id}/apply",
            axum::routing::post(effects::apply_effect),
            [
                OperationDoc::post::<hypercolor_types::api::scene::ApplyEffectResponse>(
                    "apply_effect",
                    "effects",
                    "Apply effect",
                )
                .optional_body::<hypercolor_types::api::scene::ApplyEffectRequest>(),
            ],
        ))
    // ... the rest of the effects surface
}
```

`routes::versioned()` merges the eighteen sub-routers (`assets`, `attachments`, `capture`, `config`, `controls`, `devices`, `diagnose`, `displays`, `drivers`, `effects`, `layouts`, `library`, `media`, `output`, `scene`, `scenes`, `simulators`, `system`) and adds `/ws`. `build_router()` then splits the router from its OpenAPI document, nests both under `/api/v1`, attaches an API-scoped fallback so a deleted route answers as one instead of falling through to the SPA, and merges the MCP router and Swagger UI. `/health` is registered separately at the root by `documented_root_routes()`. Path parameters use `{id}` Axum syntax.

**Never register with a bare `.route()`.** It compiles and serves traffic, but it carries no `OperationDoc`, so the operation never reaches the OpenAPI document. `crates/hypercolor-daemon/tests/openapi_tests.rs` locks the runtime document against `tests/fixtures/rest_v1/spec78-target-manifest.json` at exactly 118 operations across 83 paths, and any divergence fails `runtime_document_exactly_matches_the_spec_78_target_manifest`.

`just api-doc-route-check` is the companion documentation gate. It compares the same target manifest against `docs/content/api/rest.md` in both directions, and it scans the Markdown under `AGENTS.md`, `.agents/`, `crates/`, and `docs/` for retired route paths. This file is inside that scan, so a retired route quoted here fails the gate.
