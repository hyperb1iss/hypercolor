# API Patterns Reference

Detailed patterns for the Hypercolor daemon REST API.

## Handler Pattern

All handlers receive `State<Arc<AppState>>` via Axum extractor and return `Response` (not `Result`):

```rust
async fn list_effects(State(state): State<Arc<AppState>>) -> Response {
    let registry = state.effect_registry.read().await;
    let effects: Vec<EffectSummary> = registry.iter().map(/* ... */).collect();
    ApiResponse::ok(effects)
}
```

**ApiResponse envelope** wraps success responses (returns `Response` directly, not `Json<...>`):

```rust
pub struct ApiResponse<T: Serialize> {
    pub data: T,
    pub meta: Meta,
}

pub struct Meta {
    pub api_version: String,    // "1.0"
    pub request_id: String,     // "req_{uuid_v7}"
    pub timestamp: String,      // ISO 8601 UTC with ms precision
}
```

Constructors: `ApiResponse::ok(data)` (200), `ApiResponse::created(data)` (201), `ApiResponse::accepted(data)` (202) -- all return `Response`.

## Effect Application Flow

`POST /api/v1/effects/{id}/apply` triggers:

1. Look up effect in `effect_registry` by ID (supports fuzzy/alias resolution via `resolve_effect_metadata()`)
2. Create renderer via `create_renderer_for_metadata()` (factory pattern)
3. Lock `EffectEngine` mutex
4. Call `engine.activate(renderer, metadata)`
5. Publish `EffectStarted` event to bus
6. WebSocket broadcasts to all subscribers
7. UI receives event and updates `active_effect_id` signal

## Control Update Flow

`PATCH /api/v1/effects/current/controls` with control key-value pairs:

1. Parse `ControlValue` from JSON
2. Lock `EffectEngine`
3. `engine.set_control_checked(name, value)` → validates against definition
4. Returns previous value on success (for undo)
5. Next `tick()` call uses new value

## Error Handling

`DomainError` in `src/domain/mod.rs` is the daemon's one error type, and its `IntoResponse` impl is the daemon's one error rendering. Services and helpers return `Result<T, DomainError>`; handlers return `Response`, so they `?` inside a helper or match and call `.into_response()`:

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
    // ... service call, then ApiResponse::ok(outcome)
}
```

Never hand a `Response` back through a `Result` and never hand-build an error body. Both grow a second error surface, and `tests/api_error_surface_tests.rs` scans every file under `src/api` for them.

### Variants

```rust
pub enum DomainError {
    NotFound { kind: ResourceKind, id: String },
    Validation { message: String, field: Option<String>, details: Option<Value> },
    Malformed { message: String },
    Conflict { message: String, details: Option<Value> },
    Unauthorized { message: String },
    Forbidden { message: String, details: Option<Value> },
    PayloadTooLarge { limit_bytes: u64 },
    UnsupportedMediaType { message: String },
    RateLimited { message: String, limit: u32, window_seconds: u64, retry_after_secs: u64 },
    PreconditionFailed { resource: ResourceKind, expected: u64, current: u64 },
    DeviceUnavailable { device_id: DeviceId, reason: String },
    Internal(anyhow::Error),
}
```

Constructors cover the common shapes: `not_found(kind, id)`, `validation(msg)`, `validation_field(field, msg)`, `validation_details(msg, json)`, `malformed(msg)`, `conflict(msg)`, `conflict_details(msg, json)`, `unauthorized(msg)`, `forbidden(msg)`, `forbidden_details(msg, json)`, `unsupported_media_type(msg)`. The remaining variants carry structured fields and are built directly.

`ResourceKind` names what an error is about: `Scene`, `Zone`, `Layer`, `Effect`, `Device`, `LogicalDevice`, `Display`, `DisplayPreview`, `SimulatedDisplay`, `Driver`, `Profile`, `Layout`, `Preset`, `Playlist`, `Favorite`, `Asset`, `AttachmentTemplate`, `AttachmentSlot`, `Control`, `ControlSurface`, `Sensor`, `Diagnostic`, `Config`, `ConfigKey`, `Session`. It renders lowercase, so not-found messages derive as `"scene not found: default"` instead of being written out by hand at each call site.

### Codes and statuses

| Variant                | Code                     | Status |
| ---------------------- | ------------------------ | ------ |
| `Malformed`            | `malformed_request`      | 400    |
| `Unauthorized`         | `unauthorized`           | 401    |
| `Forbidden`            | `forbidden`              | 403    |
| `NotFound`             | `not_found`              | 404    |
| `Conflict`             | `conflict`               | 409    |
| `PreconditionFailed`   | `precondition_failed`    | 412    |
| `PayloadTooLarge`      | `payload_too_large`      | 413    |
| `UnsupportedMediaType` | `unsupported_media_type` | 415    |
| `Validation`           | `validation_error`       | 422    |
| `RateLimited`          | `rate_limited`           | 429    |
| `Internal`             | `internal_error`         | 500    |
| `DeviceUnavailable`    | `device_unavailable`     | 503    |

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

All routes are defined as flat `.route()` calls on a single `Router`, then nested under `/api/v1` in `build_router()`:

```rust
pub fn build_router(state: Arc<AppState>, ui_dir: Option<&Path>) -> Router {
    let api = Router::new()
        .route("/effects", axum::routing::get(effects::list_effects))
        .route("/effects/{id}", axum::routing::get(effects::get_effect))
        .route("/effects/{id}/apply", axum::routing::post(effects::apply_effect))
        .route("/devices", axum::routing::get(devices::list_devices))
        // ... all other flat routes ...
        .route("/ws", axum::routing::get(ws::ws_handler));
    Router::new()
        .nest("/api/v1", api)
        .with_state(state)
}
```

Route modules (`effects`, `devices`, `library`, `layouts`, `profiles`, `scenes`, `config`, `settings`, `system`, `diagnose`, `preview`, `attachments`) export individual handler functions, not sub-routers. Path parameters use `{id}` Axum syntax.
