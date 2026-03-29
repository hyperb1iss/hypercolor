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

`ApiError` is a unit struct with static builder methods that return `Response` directly:

```rust
pub struct ApiError;

impl ApiError {
    pub fn not_found(message: impl Into<String>) -> Response { /* ... */ }
    pub fn bad_request(message: impl Into<String>) -> Response { /* ... */ }
    pub fn internal(message: impl Into<String>) -> Response { /* ... */ }
    pub fn conflict(message: impl Into<String>) -> Response { /* ... */ }
    pub fn validation(message: impl Into<String>) -> Response { /* ... */ }
    pub fn unauthorized(message: impl Into<String>) -> Response { /* ... */ }
    pub fn forbidden(message: impl Into<String>) -> Response { /* ... */ }
    pub fn rate_limited(message: impl Into<String>) -> Response { /* ... */ }
    // Also: forbidden_with_details(), rate_limited_with_details()
}
```

Error responses use a separate envelope:

```rust
pub struct ApiErrorResponse {
    pub error: ErrorBody,    // { code: ErrorCode, message: String, details: Option<Value> }
    pub meta: Meta,
}

pub enum ErrorCode {
    BadRequest, Unauthorized, Forbidden, NotFound, Conflict,
    ValidationError, RateLimited, InternalError,
}
```

Each `ErrorCode` variant maps to the corresponding HTTP status code.

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
