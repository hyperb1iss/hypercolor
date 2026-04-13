# 45 — Unified Scene Model

**Status:** Draft
**Owner:** TBD
**Target:** remove the dual EffectEngine/SceneManager render path; make scenes the universal orchestration layer for LED effects, display faces, and future per-zone/per-device composition.

---

## 1. Executive Summary

Hypercolor currently runs on two parallel render paths that don't know about each other:

- **EffectEngine path** — one global effect, one canvas, sampled onto every zone. Selected by `POST /effects/{id}/apply`.
- **Scene path** — zero or more `RenderGroup`s, each with its own effect instance, canvas, and spatial layout. Selected by `POST /scenes/{id}/activate`.

The composer picks one or the other per frame (`frame_composer.rs:110` — `if scene_runtime.has_active_render_groups() { scene path } else { effect path }`). They're mutually exclusive.

This split causes concrete problems:

1. **Display faces live inside scenes as `RenderGroup`s with `display_target: Some(..)`**, but the daemon's default runtime state has no active scene. Assigning a face therefore returns `409 Conflict` with "Activate a scene first" — there is no UI for scenes, so users literally cannot resolve it.
2. **There is no path from the running effect to a scene**. Applying an effect touches `EffectEngine` directly (`api/effects.rs:391`) and never populates a `RenderGroup`. That's why users with many devices still have zero render groups — the machinery only wakes up on scene activation.
3. **Overlays are per-device; faces are per-scene**. Asymmetric models for conceptually similar features (stuff drawn on an LCD).
4. **Future multi-zone / multi-device composition** (e.g., "sunset gradient on the ceiling strip, beat-react on the keyboard") has no clean home. The scene path already supports this, but the EffectEngine path undermines it.

**Target:** delete the EffectEngine path. Every frame is composed from render groups inside an active scene. Applying an effect upserts the "primary" render group of the active scene. Faces upsert a "display" render group. There is always an active scene — the daemon boots with an ephemeral `Default` scene ready for use.

External API shapes (`POST /effects/{id}/apply`, `PATCH /effects/current/controls`, `GET /effects/active`) stay stable — they're re-plumbed internally to operate on the primary render group.

---

## 2. Current State

### 2.1 Where the active effect lives today

`EffectEngine` (`crates/hypercolor-core/src/effect/engine.rs:41-75`) owns:

- `renderer: Option<Box<dyn EffectRenderer>>` — the live renderer
- `metadata: Option<EffectMetadata>` — the active effect's definition
- `controls: HashMap<String, ControlValue>` — live control values
- `active_preset_id: Option<String>` — applied preset
- `state: EffectState` — Loading | Initializing | Running | Paused | Destroying

It is called once per frame (`frame_sources.rs:74`) to render a single canvas. The canvas is published to the global `canvas` watch channel (`bus/mod.rs`) and sampled by a global `SpatialEngine` into zone colors for every device.

### 2.2 Where render groups live today

A `Scene` holds `groups: Vec<RenderGroup>`. When any scene is active, `frame_composer.rs:110` routes frames through `render_groups.rs:96-191`. Each group renders via `EffectPool::render_group_into()` using its own `Canvas`, its own `SpatialEngine`, and the same shared audio/sensor/screen snapshots.

The primary render path and the group path are mutually exclusive per tick. When a scene activates, `EffectEngine` keeps running but its canvas is discarded (composer never calls `render_effect_frame` while `has_active_render_groups()` is true).

### 2.3 Persistence today

- `runtime-state.json` captures effect_id, controls, active_preset_id, brightness, layout_id. Startup with `daemon.start_profile = "last"` restores via `EffectEngine::activate()` (`startup/lifecycle.rs:286-343`).
- `SceneManager` is in-memory only. Scenes don't survive restart.
- Profiles (`profiles.json`) snapshot effect-engine state + brightness + layout. They do not capture scenes.

### 2.4 Why the 409 exists

`api/displays.rs:277-281`:

```rust
let Some(active_scene_id) = scene_manager.active_scene_id().copied() else {
    return ApiError::conflict(
        "No active scene to attach a display face to. Activate a scene first."
    );
};
```

Faces are `RenderGroup`s. Render groups live inside scenes. No scene → nowhere to put the group. The user has no UI to create/activate a scene, so the feature is inaccessible without CLI or direct API access.

---

## 3. Target Model

### 3.1 Invariants

1. **There is always exactly one active scene.** On boot, `SceneManager` ensures an ephemeral `Default` scene exists and is active. Activating another scene pushes it onto the priority stack; deactivating pops back, and the stack's floor is always Default.
2. **Every rendered frame is composed from render groups.** The `EffectEngine` type is removed. The composer always takes the render-groups path.
3. **Each scene has at most one `Primary` render group.** The primary group's layout has `SceneScope::Full` semantics (covers every zone). It's the successor to "the global active effect."
4. **Each scene has at most one `Display` render group per device.** Faces are render groups with `display_target: Some(DisplayFaceTarget { device_id })`. Enforced by a uniqueness check on `(scene_id, device_id)`.
5. **Applying an effect mutates the active scene's primary group.** The active scene (Default or named) is the target — not a fixed "Default" scene.

### 3.2 Scene taxonomy

Scenes carry a new `kind` field distinguishing behaviour:

- `SceneKind::Ephemeral` — not persisted, not listed in `GET /scenes`, cannot be deleted. The `Default` scene is the canonical example. Future room/context scenes may also be ephemeral (e.g., auto-created for an automation context).
- `SceneKind::Named` — persisted to disk, listed in the scenes API, user-visible. Activated explicitly.

Applying an effect mutates whichever scene is currently active. Named scenes are not "locked" in this spec — that's a separate feature (§12.3).

### 3.3 Render group roles

A new enum marks render group roles to make upsert targets unambiguous:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderGroupRole {
    #[default]
    Custom,   // user-defined, typically targeting a zone subset
    Primary,  // the "apply effect" target — covers full scope
    Display,  // has display_target; renders to an LCD
}
```

Invariants:

- At most one `Primary` group per scene.
- `Display` implies `display_target.is_some()`; `display_target.is_some()` implies `Display`. Enforced by constructor.
- `Custom` groups can freely coexist.

Alternative considered: detect the primary group by "no `display_target` and full-scope layout." Rejected because it's fragile — a user could create a Custom full-scope group and overlaps would be ambiguous. The explicit role marker keeps the upsert path deterministic.

### 3.4 Mental model summary

```
Daemon
└── SceneManager
    ├── active_scene (always)
    │   └── groups: [
    │         Primary (role=Primary, layout=Full, effect=<active>),
    │         Display[dev A] (display_target=Some(A), effect=<face A>),
    │         Display[dev B] (display_target=Some(B), effect=<face B>),
    │         Custom[...] (future: per-zone / per-device compositions)
    │       ]
    └── Default scene (ephemeral, always present)
```

Every frame: composer iterates active scene's enabled groups, EffectPool renders each, SpatialEngines sample zones, display workers receive canvases for display groups.

---

## 4. Architectural Decisions

### 4.1 Remove `EffectEngine`, don't reuse it

**Decision:** delete `hypercolor-core/src/effect/engine.rs` outright. Its responsibilities (renderer ownership, control state, preview canvas) are already handled by `EffectPool` in a per-group way. Keeping it around as a shim for one specific group adds complexity for no gain.

**Consequence:** every consumer that currently holds `effect_engine: Arc<Mutex<EffectEngine>>` is replaced with access through `SceneManager` + `EffectPool`. Several call sites shrink.

### 4.2 Default scene is ephemeral, not persisted

**Decision:** the `Default` scene is recreated on every daemon boot. Its contents (primary group's effect, display groups, etc.) ARE persisted via `runtime-state.json`, but the scene shell itself is synthesized at startup. Users never see it in the scenes list.

**Rationale:** avoids a special "don't serialize this scene" code path. `runtime-state.json` already persists "what the daemon was doing when it shut down"; extending it to cover the Default scene's group contents is the minimum viable change.

### 4.3 Named scenes persist; activation order persists

**Decision:** add `SceneStore` (JSON at `data_dir()/scenes.json`) following the `ProfileStore` pattern. On startup, load named scenes, then restore either (a) the last-active named scene, or (b) the Default scene, based on `runtime-state.json::active_scene_id`.

**Rationale:** scene persistence is overdue. This spec is the natural place to land it because unified-model tests all depend on deterministic scene state across restarts.

### 4.4 External API shapes unchanged (except face 409)

**Decision:** `POST /effects/{id}/apply`, `GET /effects/active`, `PATCH /effects/current/controls`, `POST /effects/current/reset` keep their current request/response shapes. Internals route through scene + render group. The face 409 goes away.

**Additive:** `GET /effects/active` gains `render_group_id: RenderGroupId` in its response body. Clients that don't know about render groups continue to work.

**Rationale:** the UI and tooling have dozens of call sites (§8 below). Changing the API shape now is a massive scope explosion. The refactor is internal plumbing; external shapes change only where the scene model leaks out intentionally.

### 4.5 Primary group's canvas IS the global preview canvas

**Decision:** the render loop publishes the primary render group's canvas to the global `canvas` watch channel (in addition to `group_canvases[primary_id]`). The UI's existing canvas-preview subscriber keeps working unchanged.

**Future:** once per-group previews are in the UI, the global `canvas` channel can be deprecated. Not in this spec.

### 4.6 Display groups are always per-scene, not per-device

**Decision:** faces stay in scenes as `Display` render groups. The Default scene holds face assignments for "standard use." Scenes with different face compositions are created only when a user intentionally creates named scenes that override.

**Alternative considered:** move faces to per-device settings (parallel to overlays). Rejected because:

- Faces need the same rendering machinery (EffectPool, per-group canvas, audio/sensor inputs) as LED effects. Duplicating that outside scenes is more work than leaving them in.
- The scene model is already the cleanest home for "LCD shows a face while LEDs run an effect" composition.
- Users wanting "face always X regardless of scene" can be served later with a per-device default that named scenes inherit unless they explicitly override.

### 4.7 Ephemeral Default scene survives across `scene activate/deactivate`

**Decision:** the Default scene has `ScenePriority::AMBIENT` (lowest) and is always on the priority stack floor. Activating a named scene pushes it on top; deactivating returns to Default. There is no user-visible "deactivate default" action.

**Rationale:** keeps the "always an active scene" invariant without special-case code in the composer. Priority stack already handles fallback semantics.

### 4.8 Scene transitions stay as-is for named-scene switches; cut for effect swaps

**Decision:**

- `POST /scenes/{id}/activate` — runs the scene's `TransitionSpec` (blend zone assignments from→to).
- `POST /effects/{id}/apply` — mutates the active scene's primary group in place, effect swap is a cut (destroy old renderer, init new one via EffectPool). No zone-assignment blend.

**Rationale:** users expect "apply effect" to be immediate. Named-scene switches are the canonical place to pay for fade transitions.

### 4.9 Audio reactivity driven by *any* active group's metadata

**Decision:** `frame_state.rs:130-134` changes from "is EffectEngine's metadata audio_reactive?" to "does any enabled active render group have audio_reactive metadata?"

**Consequence:** audio capture runs if the primary group OR any display face OR any custom group requires it.

---

## 5. Detailed Design

### 5.1 Type changes

#### 5.1.1 `RenderGroup` (`hypercolor-types/src/scene.rs`)

Add `role` field:

```rust
pub struct RenderGroup {
    pub id: RenderGroupId,
    pub name: String,
    pub description: Option<String>,
    pub effect_id: Option<EffectId>,
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
    pub preset_id: Option<PresetId>,
    pub layout: SpatialLayout,
    #[serde(default = "default_group_brightness")]
    pub brightness: f32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_target: Option<DisplayFaceTarget>,
    #[serde(default)]
    pub role: RenderGroupRole,  // NEW
}
```

New enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderGroupRole {
    #[default]
    Custom,
    Primary,
    Display,
}
```

Because this model has not shipped yet, persisted scenes must include explicit `role` and `kind` fields. Missing-role or missing-kind payloads are rejected instead of being auto-migrated.

#### 5.1.2 `Scene` (`hypercolor-types/src/scene.rs`)

Add `kind`:

```rust
pub struct Scene {
    // ... existing fields
    #[serde(default)]
    pub kind: SceneKind,  // NEW
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneKind {
    #[default]
    Named,
    Ephemeral,
}
```

Invariant: `Scene::validate()` rejects scenes with more than one `Primary` group or duplicate `Display` device IDs.

#### 5.1.3 Well-known Default scene ID

```rust
impl SceneId {
    pub const DEFAULT: Self = Self(Uuid::from_u128(0));
    pub fn is_default(&self) -> bool { *self == Self::DEFAULT }
}
```

Or a named constant declared in `scene/manager.rs`. The all-zeros UUID makes serialization round-trip cleanly and is trivially recognizable in logs.

### 5.2 `SceneManager` changes (`hypercolor-core/src/scene/mod.rs`)

#### 5.2.1 Initialization

```rust
impl SceneManager {
    pub fn with_default() -> Self {
        let mut manager = Self::new();
        manager.install_default_scene();
        manager
    }

    fn install_default_scene(&mut self) {
        let default = Scene {
            id: SceneId::DEFAULT,
            name: "Default".into(),
            description: Some("Auto-managed default scene.".into()),
            scope: SceneScope::Full,
            zone_assignments: Vec::new(),
            groups: Vec::new(),
            transition: TransitionSpec::default(),
            priority: ScenePriority::AMBIENT,
            enabled: true,
            metadata: HashMap::new(),
            unassigned_behavior: UnassignedBehavior::default(),
            kind: SceneKind::Ephemeral,
        };
        self.scenes.insert(default.id, default);
        self.priority_stack.push(SceneId::DEFAULT, ScenePriority::AMBIENT);
        self.refresh_active_render_groups();
    }
}
```

Called from `DaemonState::initialize()` (in place of `SceneManager::new()`).

#### 5.2.2 Protected operations

`delete(id)` — reject if `id == SceneId::DEFAULT` with `anyhow::bail!("cannot delete default scene")`.
`update(scene)` — reject if caller attempts to change `scene.kind` from `Ephemeral` to `Named` or vice versa, or to rename Default.

`deactivate_current()` — when stack underflows below the Default floor, no-op (Default stays active). Currently `deactivate_current()` already pops from the stack; the bottom entry is now always Default, so regular users get Default on final pop — no change in logic, just in initial state.

#### 5.2.3 Primary group helpers

```rust
impl Scene {
    pub fn primary_group(&self) -> Option<&RenderGroup> {
        self.groups.iter().find(|g| g.role == RenderGroupRole::Primary)
    }

    pub fn primary_group_mut(&mut self) -> Option<&mut RenderGroup> {
        self.groups.iter_mut().find(|g| g.role == RenderGroupRole::Primary)
    }

    pub fn display_group_for(&self, device_id: DeviceId) -> Option<&RenderGroup> {
        self.groups.iter().find(|g| {
            g.role == RenderGroupRole::Display
                && g.display_target.as_ref().is_some_and(|t| t.device_id == device_id)
        })
    }
}
```

#### 5.2.4 Upsert helpers on SceneManager

```rust
impl SceneManager {
    /// Upsert the primary render group of the active scene. Creates it if missing.
    /// Returns the updated group.
    pub fn upsert_primary_group(
        &mut self,
        effect: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        active_preset_id: Option<PresetId>,
        full_scope_layout: SpatialLayout,
    ) -> Result<&RenderGroup> { /* ... */ }

    /// Upsert a display render group in the active scene.
    pub fn upsert_display_group(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        surface: DisplaySurfaceInfo,
        effect: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
    ) -> Result<&RenderGroup> { /* ... */ }

    /// Remove a display group by device id. Returns whether removal occurred.
    pub fn remove_display_group(&mut self, device_id: DeviceId) -> bool { /* ... */ }

    /// Patch controls on a group by id (primary or display). Returns None if missing.
    pub fn patch_group_controls(
        &mut self,
        group_id: RenderGroupId,
        updates: HashMap<String, ControlValue>,
    ) -> Option<&RenderGroup> { /* ... */ }
}
```

All of these operate on `self.active_scene_id()`'s scene. They emit `HypercolorEvent::RenderGroupChanged` (new variant) on the bus after mutation and refresh `active_render_groups`.

### 5.3 Render pipeline changes

#### 5.3.1 Remove the effect-engine branch

`frame_composer.rs:108-177` — delete the `else` arm. Always call `compose_render_group_frame_set(stage_start)`.

`frame_sources.rs:render_effect_frame()` and `frame_sources.rs:74` — delete.

`pipeline_driver.rs` — delete any `effect_engine.lock()` call sites.

#### 5.3.2 EffectPool covers all groups uniformly

No behavioral change — `EffectPool` (`effect/pool.rs`) already handles per-group effect lifecycle. The primary group just becomes another entry keyed by its `RenderGroupId`. Reconciliation (`pool.rs:33-66`) handles effect_id changes cleanly (destroy old, create new).

#### 5.3.3 Primary group's canvas to global channel

In `frame_io.rs` (where `group_canvases` watch channels are published), also publish the primary group's canvas to the global `canvas` channel:

```rust
if let Some(primary) = active_scene.primary_group() {
    if let Some(frame) = target_canvases.get(&primary.id) {
        event_bus.canvas_sender().send_replace(frame.clone());
    }
}
```

This keeps the UI's canvas preview working unchanged.

#### 5.3.4 Single-group fast path

`render_groups.rs:250-305` has a single-group rendering fast path. It works today; verify it still applies when the single group is a Primary one (it should — the decision is based on group count and layout fit, not role).

### 5.4 API layer changes

#### 5.4.1 `POST /api/v1/effects/{id}/apply`

Reroute `apply_effect` in `api/effects.rs:333-445`:

**Before** (simplified):
```rust
let renderer = create_renderer_for_metadata_with_mode(...)?;
engine.activate(renderer, metadata.clone());
for (k, v) in controls { engine.set_control_checked(k, v); }
```

**After**:
```rust
let layout = resolve_full_scope_layout(&state).await;  // full-canvas layout
let group = {
    let mut scene_manager = state.scene_manager.write().await;
    scene_manager.upsert_primary_group(
        &metadata,
        controls,
        /*active_preset_id:*/ None,
        layout,
    )?
    .clone()
};
```

The renderer is no longer built in the API handler — `EffectPool::reconcile()` creates it on the next tick when it sees the new `effect_id` on the primary group.

Keep `HypercolorEvent::EffectStarted` publication for existing clients, and also publish the new `RenderGroupChanged` event. Keep `persist_runtime_session`.

Display category effects keep their validation rejection (line 353-358) — display faces go through `/displays/{id}/face`, not `/effects/apply`.

#### 5.4.2 `GET /api/v1/effects/active`

Read from scene_manager instead of effect_engine:

```rust
let scene_manager = state.scene_manager.read().await;
let active_scene = scene_manager.active_scene()?;
let primary = active_scene.primary_group()?;
let Some(effect_id) = primary.effect_id else {
    return ApiResponse::ok(ActiveEffectResponse::idle());
};
let effect = effect_registry.get(&effect_id)?;
ApiResponse::ok(ActiveEffectResponse {
    id: effect_id.to_string(),
    name: effect.name.clone(),
    state: "running".into(),  // derived from EffectPool slot state
    controls: effect.controls.clone(),
    control_values: primary.controls.clone(),
    active_preset_id: primary.preset_id.map(|p| p.to_string()),
    render_group_id: Some(primary.id),  // NEW field
})
```

Add `render_group_id: Option<RenderGroupId>` to the response DTO.

#### 5.4.3 `PATCH /api/v1/effects/current/controls`

Reroute `update_current_controls` to `scene_manager.patch_group_controls(primary_group_id, updates)`. The changes land on the `RenderGroup.controls` field; `EffectPool::reconcile()` observes the revision bump and syncs into the live renderer.

Rejection semantics preserved (invalid control shapes return warnings).

#### 5.4.4 `POST /api/v1/effects/current/reset`

Reset primary group's controls to effect metadata defaults.

#### 5.4.5 `POST /api/v1/effects/current/stop`

Semantics: set the active scene's primary group `effect_id = None`. EffectPool drops the slot. If the user wanted "black output everywhere," this is how.

#### 5.4.6 `PUT /api/v1/displays/{id}/face`

`api/displays.rs:240-309`:

**Before**: 409 if no active scene.
**After**: always succeeds. `scene_manager.upsert_display_group(device_id, device_name, surface, &effect, controls)` — the active scene is always present now.

`DELETE /displays/{id}/face` (line 312+): uses `remove_display_group(device_id)`. No 409. Idempotent (returns the cleared state even if no group was present).

`PATCH /displays/{id}/face/controls` (line 375+): uses `patch_group_controls(display_group_id, updates)`.

`GET /displays/{id}/face` (line 791+): reads from active scene's display group. No "active scene not found" branch.

#### 5.4.7 Scene API endpoints

Unchanged: `GET /scenes`, `POST /scenes`, `GET /scenes/:id`, `PUT /scenes/:id`, `DELETE /scenes/:id`, `POST /scenes/:id/activate`.

`GET /scenes` — exclude scenes where `kind == Ephemeral` from the list (Default never surfaces).
`DELETE /scenes/:id` — already rejects `SceneId::DEFAULT` via SceneManager.

New: `POST /scenes/deactivate` — pops the current non-Default scene, returns to Default. (Currently no endpoint exists; useful for UI.)

### 5.5 Event bus changes

New events in `HypercolorEvent`:

```rust
RenderGroupChanged {
    scene_id: SceneId,
    group_id: RenderGroupId,
    role: RenderGroupRole,
    kind: RenderGroupChangeKind,  // Created | Updated | Removed | ControlsPatched
},
ActiveSceneChanged {
    previous: Option<SceneId>,
    current: SceneId,
    reason: SceneChangeReason,  // UserActivate | UserDeactivate | EffectApplied | DaemonStart
},
```

Retain existing `EffectStarted` / `EffectStopped` / `EffectControlChanged` — emit whenever the *primary* group changes effect/controls. This preserves WS subscriber semantics for UI clients that listen to these events (e.g. `messages.rs:452-457`).

### 5.6 Persistence changes

#### 5.6.1 `SceneStore`

New: `crates/hypercolor-daemon/src/scene_store.rs`, modeled on `profile_store.rs`.

```rust
pub struct SceneStore {
    path: PathBuf,
    scenes: HashMap<SceneId, Scene>,
}

impl SceneStore {
    pub fn load(path: &Path) -> Result<Self> { /* reads scenes.json */ }
    pub fn save(&self) -> Result<()> { /* atomic write */ }
    pub fn put(&mut self, scene: Scene) -> Result<()> { /* upsert named scene */ }
    pub fn remove(&mut self, id: &SceneId) -> Option<Scene> { /* not Default */ }
    pub fn list(&self) -> impl Iterator<Item = &Scene> { /* named only */ }
}
```

The store only holds `kind == Named` scenes. Default is synthesized at runtime.

#### 5.6.2 `runtime-state.json` extensions

Current fields:
```json
{
  "effect": { "id": "...", "controls": {}, "preset_id": null },
  "brightness": 1.0,
  "layout": { "id": "..." }
}
```

New fields:
```json
{
  "active_scene_id": "uuid-or-default-zero",
  "default_scene_groups": [ /* full RenderGroup array of the Default scene */ ],
  "brightness": 1.0,
  "layout": { "id": "..." }
}
```

Rationale: Default scene isn't in `scenes.json`, so its contents go into `runtime-state.json`. Named scenes restore from `scenes.json` by ID. The file is scene-backed only; older effect-only payloads are rejected.

#### 5.6.3 Startup sequence

`services.rs::DaemonState::initialize()` (currently L73-438):

1. ConfigManager, event bus (unchanged)
2. **SceneManager::with_default()** (replaces `SceneManager::new()`)
3. Load `scenes.json` via `SceneStore::load()`, insert each named scene into SceneManager
4. EffectRegistry, render loop (unchanged)
5. `startup/lifecycle.rs::restore_runtime_session_if_configured()`:
   - If `active_scene_id` is a Named scene ID → `scene_manager.activate(id)`
   - If Default, no-op (already active)
   - Apply `default_scene_groups` to Default scene's groups (via direct mutation)
   - Restore brightness, layout

#### 5.6.4 Named scene persistence hooks

`POST /scenes`, `PUT /scenes`, `DELETE /scenes` — call `scene_store.save()` after mutation. Failures surface as 500 (don't silently drop).

`runtime-state.json` writes on every apply_effect, patch_controls, face assignment (same as today).

### 5.7 Display face changes

Most of this is API-level and covered in §5.4.6. The core mutation helpers (`upsert_display_face_group`, `display_face_layout`) in `api/displays.rs:844-906` move into `SceneManager::upsert_display_group` so they're uniformly available for MCP, CLI, and future UI callers.

### 5.8 EffectEngine removal

Files to delete:
- `crates/hypercolor-core/src/effect/engine.rs`
- References in `crates/hypercolor-core/src/effect/mod.rs`

Fields to remove from `AppState`:
- `effect_engine: Arc<Mutex<EffectEngine>>`

Every `effect_engine.lock().await` call site (found in §6 below) becomes `scene_manager.read/write().await` + primary-group access.

### 5.9 Profile persistence changes

`Profile` struct (`profile_store.rs`) currently captures:
```rust
struct Profile {
    id, name, effect_id, controls, active_preset_id, brightness, layout_id, ...
}
```

To support the new model, profiles also capture face assignments. New shape:

```rust
struct Profile {
    id: String,
    name: String,
    // Inclusion = "this profile sets the X". Missing = "don't touch X on apply."
    primary: Option<ProfilePrimary>,
    displays: Vec<ProfileDisplay>,     // faces per device_id
    brightness: Option<f32>,
    layout_id: Option<String>,
    // ...
}

struct ProfilePrimary {
    effect_id: EffectId,
    controls: HashMap<String, ControlValue>,
    active_preset_id: Option<PresetId>,
}

struct ProfileDisplay {
    device_id: DeviceId,
    effect_id: EffectId,
    controls: HashMap<String, ControlValue>,
}
```

Profiles persist only the final scene-backed shape. Pre-final top-level `effect_id`/`controls` fields are rejected on load.

Apply logic (`api/profiles.rs:209-252`) becomes:
- If `primary` → `upsert_primary_group`
- For each `displays` entry → `upsert_display_group`
- Set brightness, layout as before

---

## 6. File-by-file Touchpoints

### 6.1 Types (`crates/hypercolor-types/`)

- `src/scene.rs` — add `RenderGroupRole`, `SceneKind`, `SceneId::DEFAULT`, invariants on `Scene::validate` / new constructor helpers.
- `src/event.rs` — add `RenderGroupChanged`, `ActiveSceneChanged`, `RenderGroupChangeKind`, `SceneChangeReason`.

### 6.2 Core (`crates/hypercolor-core/`)

- `src/scene/mod.rs` — `install_default_scene`, `with_default`, upsert helpers, validation, events on mutation, protect Default from delete.
- `src/effect/mod.rs` — remove `engine` submodule declaration.
- `src/effect/engine.rs` — **DELETE**.
- `src/effect/pool.rs` — no behavioral change; confirm it handles Primary groups fine.
- `src/config/paths.rs` — add `scenes_json_path()` helper.

### 6.3 Daemon (`crates/hypercolor-daemon/`)

- `src/api/mod.rs`:
  - Remove `effect_engine: Arc<Mutex<EffectEngine>>` from `AppState`.
  - Pass `scene_store: Arc<RwLock<SceneStore>>` into `AppState`.
- `src/services.rs` (or wherever DaemonState::initialize lives):
  - Use `SceneManager::with_default()`.
  - Load `SceneStore::load()` before `restore_runtime_session_if_configured`.
  - Remove EffectEngine construction.
- `src/scene_store.rs` — **NEW**.
- `src/api/effects.rs`:
  - `apply_effect`, `update_current_controls`, `reset_current_controls`, `stop_current`, `get_active_effect` — reroute to scene_manager.
  - Remove renderer creation in the API handler (EffectPool handles it).
  - Keep Display category rejection.
- `src/api/displays.rs`:
  - `set_display_face` — remove 409 branch, call `upsert_display_group`.
  - `delete_display_face` — idempotent, no 409.
  - `patch_display_face_controls` — use `patch_group_controls`.
  - `current_display_face_assignment` — read primary/display groups from active scene.
  - Move `upsert_display_face_group`, `display_face_layout` into SceneManager.
- `src/api/scenes.rs` — filter out `SceneKind::Ephemeral` from `list`. Add `POST /scenes/deactivate`.
- `src/api/profiles.rs` — use the new profile shape and apply via scene_manager.
- `src/api/ws/`:
  - `protocol.rs` — add new server events if desired.
  - `relays.rs` / `mod.rs` — wire `RenderGroupChanged` / `ActiveSceneChanged` through as WS events.
- `src/render_thread/frame_composer.rs` — delete the effect-only branch.
- `src/render_thread/frame_sources.rs` — delete `render_effect_frame` path.
- `src/render_thread/frame_io.rs` — publish primary group canvas to global `canvas` channel.
- `src/render_thread/frame_state.rs` — audio-reactive detection iterates all active groups.
- `src/render_thread/pipeline_driver.rs` — drop `effect_engine.lock()` uses.
- `src/runtime_state.rs` — store only scene-backed runtime fields.
- `src/startup/lifecycle.rs` — update restore logic.
- `src/mcp/tools/effects.rs` — `set_effect`, `stop_effect` handlers route through scene_manager.
- `src/mcp/tools/scenes.rs` — surface the new `ActiveSceneChanged` event; Default doesn't appear in `list_scenes`.
- `src/profile_store.rs` — new profile shape with strict deserialization.

### 6.4 UI (`crates/hypercolor-ui/`)

Minimal — the API shapes are preserved.

- `src/api/effects.rs` — add `render_group_id` to `ActiveEffectResponse` (optional, ignore-able for callers that don't care).
- `src/app.rs`::`EffectsContext` — add optional `active_render_group_id: ReadSignal<Option<RenderGroupId>>` (used by future scene UI; not needed for current pages).
- `src/ws/messages.rs` — handle `render_group_changed` / `active_scene_changed` events (minimally, to avoid "unknown event" warnings in console).
- `src/pages/displays.rs` — drop any 409-specific error-path UX.

### 6.5 TUI (`crates/hypercolor-tui/`)

- `src/client/rest.rs` — `ActiveEffectResponse` gains optional `render_group_id` field. No shape break.
- `src/app.rs::DaemonState` — no change needed; already tracks effect_name + effect_id.

### 6.6 CLI (`crates/hypercolor-cli/`)

- `src/commands/effects.rs` — no source-level change; command shapes unchanged.
- Add `hyper scene activate/deactivate/list/create` commands? — **out of scope for this spec**, leave for follow-up. CLI can work through the existing endpoints.

### 6.7 Tray (`crates/hypercolor-tray/`)

- No changes to `current_effect`/`EffectInfo`.

### 6.8 SDK (`sdk/`)

- No changes.

---

## 7. API Contracts

### 7.1 Stable shapes (no break)

- `POST /api/v1/effects/{id}/apply` — body + response unchanged. `warnings` array gains potential `"primary_group_created"` strings.
- `POST /api/v1/effects/current/reset` — unchanged.
- `POST /api/v1/effects/current/stop` — unchanged.
- `GET /api/v1/effects/active` — response DTO gains `render_group_id: Option<String>`.
- `PATCH /api/v1/effects/current/controls` — unchanged.
- `GET /api/v1/scenes` — same shape; Ephemeral scenes filtered out.
- `GET /api/v1/scenes/:id` — unchanged.
- `POST /api/v1/scenes` — unchanged.
- `PUT /api/v1/scenes/:id` — unchanged.
- `DELETE /api/v1/scenes/:id` — unchanged (already errors on Default).
- `POST /api/v1/scenes/:id/activate` — unchanged.
- `PUT /api/v1/displays/{id}/face` — no more 409.
- `DELETE /api/v1/displays/{id}/face` — no more 409, idempotent.
- `PATCH /api/v1/displays/{id}/face/controls` — no more 409.
- `GET /api/v1/displays/{id}/face` — returns `null` face when primary-only scene.

### 7.2 New endpoints

- `POST /api/v1/scenes/deactivate` — pop non-Default scene, return to Default.
- `GET /api/v1/scenes/active` — returns the active scene including Default. Response:
  ```json
  {
    "id": "00000000-0000-0000-0000-000000000000",
    "name": "Default",
    "kind": "ephemeral",
    "groups": [...]
  }
  ```

### 7.3 WS events

- Preserve `effect_started`, `effect_stopped`, `effect_control_changed`.
- Add `render_group_changed`, `active_scene_changed`.

---

## 8. Test Plan

### 8.1 Unit tests (move to `tests/` directories; never `#[cfg(test)]` inline)

- `crates/hypercolor-core/tests/scene_manager_default_tests.rs`:
  - `with_default_installs_default_scene_as_ephemeral`
  - `default_scene_cannot_be_deleted`
  - `deactivate_below_default_is_noop`
  - `upsert_primary_group_creates_when_absent`
  - `upsert_primary_group_updates_effect_id_when_present`
  - `upsert_display_group_uniqueness_per_device`
  - `remove_display_group_is_idempotent`
  - `patch_group_controls_missing_group_returns_none`
- `crates/hypercolor-core/tests/render_group_role_tests.rs`:
  - `scene_validate_rejects_two_primary_groups`
  - `scene_validate_rejects_display_without_target`
  - `scene_validate_rejects_duplicate_display_device_ids`
- `crates/hypercolor-core/tests/scene_migrate_role_tests.rs`:
  - Legacy scenes with no `role` field deserialize with `Custom` default.
  - Migration promotes full-scope groups to `Primary` (if applicable).
  - Migration promotes `display_target.is_some` groups to `Display`.

### 8.2 Daemon integration tests (`crates/hypercolor-daemon/tests/`)

- `api_effects_tests.rs`:
  - `apply_effect_upserts_primary_group`
  - `apply_effect_mutates_active_scene_not_default_if_named_active`
  - `apply_effect_swap_replaces_primary_effect_id`
  - `get_active_effect_returns_primary_group_info`
  - `patch_controls_updates_primary_group_controls`
  - `stop_current_clears_primary_effect_id_but_keeps_scene`
- `api_displays_tests.rs`:
  - `put_face_from_cold_start_succeeds_no_409` (regression)
  - `delete_face_idempotent_when_no_group_present`
  - `get_face_returns_null_when_no_display_group`
  - `patch_face_controls_updates_display_group`
  - `face_survives_effect_swap` (face assignment is orthogonal to primary effect swap)
- `api_scenes_tests.rs`:
  - `list_scenes_excludes_default`
  - `delete_default_returns_409_or_422`
  - `deactivate_returns_to_default`
  - `activating_named_scene_then_applying_effect_mutates_named_scene`
- `persistence_tests.rs`:
  - `runtime_state_captures_default_scene_groups`
  - `named_scenes_persist_across_restart`
  - `default_scene_contents_restore_on_restart`
  - `pre_final_profile_shape_is_rejected_on_load`
  - `removed_runtime_effect_fields_are_rejected_on_startup`

### 8.3 Render pipeline tests

- `crates/hypercolor-daemon/tests/render_pipeline_tests.rs` (may already exist; extend):
  - `primary_group_canvas_published_to_global_channel`
  - `display_group_canvas_routes_to_device_worker`
  - `audio_capture_enabled_when_any_active_group_is_reactive`
  - `effect_engine_removal_does_not_break_single_group_fast_path`

### 8.4 Manual verification checklist

- Cold start daemon, assign a face → succeeds, no 409, logs show "default scene primary group created".
- Apply effect via TUI → zones light up, preview canvas visible in UI, `render_group_id` in `GET /effects/active` response is non-null.
- Restart daemon → last effect restored, last face restored, zones + LCD both resume.
- Create named scene A via `POST /scenes`, activate it, apply a new effect → new effect modifies scene A (not Default). Deactivate → Default's last effect resumes.
- Profiles: save current state → apply to a freshly-started daemon → verify effect + face both restored.

---

## 9. Implementation Phases

Recommended rollout order. Each phase should leave the workspace green (`just verify` passes) before moving on.

### Phase 1 — Types & invariants (no runtime behavior change)
- Add `RenderGroupRole`, `SceneKind`, `SceneId::DEFAULT`.
- Add `Scene::validate()` for new invariants.
- Require explicit `role`/`kind` in persisted scene payloads.
- Unit tests for types.

### Phase 2 — SceneManager default + helpers
- `SceneManager::with_default()` + ephemeral Default scene.
- `upsert_primary_group`, `upsert_display_group`, `remove_display_group`, `patch_group_controls`.
- New events on bus.
- Unit tests for SceneManager.

### Phase 3 — SceneStore + persistence
- `SceneStore::load/save`.
- `runtime-state.json` schema cutover to scene-backed data only.
- Startup loads + restores.
- Persistence tests.

### Phase 4 — Render pipeline unification
- Wire `SceneManager::with_default()` into `DaemonState::initialize`.
- Publish primary group canvas to global `canvas` channel in frame_io.
- Audio reactivity iterates active groups in frame_state.
- Delete `EffectEngine`, `engine.rs`, `render_effect_frame`, effect-branch of composer.
- Pipeline integration tests.
- **At this point: daemon runs, effects still work via existing `POST /effects/{id}/apply` IF we keep that endpoint writing to EffectEngine → this doesn't work because EffectEngine is gone. So Phase 5 lands with Phase 4.**

### Phase 5 — API rewiring
- `apply_effect`, `update_current_controls`, `reset_current_controls`, `stop_current`, `get_active_effect` — reroute to scene_manager.
- `set_display_face`, `delete_display_face`, `patch_display_face_controls`, `current_display_face_assignment` — drop 409, route through scene_manager helpers.
- MCP `set_effect`, `stop_effect` — reroute.
- Profile save/apply — new shape only.
- API integration tests.

### Phase 6 — Events & WS
- Emit `RenderGroupChanged`, `ActiveSceneChanged`.
- WS event serialization + UI message handlers (stubs, no UI change required).

### Phase 7 — Cleanup
- Remove `effect_engine` from `AppState`.
- Remove effect-engine-specific code paths in profile store, startup, etc.
- Grep for dangling references; confirm workspace is clean.

Phases 1–3 are safe to merge independently. Phases 4–5 must land together (deleting EffectEngine breaks the effect API path unless the rewire happens in the same PR).

---

## 10. Risk & Mitigation

### 10.1 Persistence contract drift

**Risk:** local dev data written by pre-final branches may no longer parse once the strict scene-backed schema lands.

**Mitigation:** fail fast on load, log the parse error, and continue with a clean synthesized Default scene. Because this has not shipped, we prefer an explicit contract cutover over carrying compatibility code.

### 10.2 Servo effect re-init cost

**Risk:** EffectEngine today reuses the Servo renderer across tick loops. `EffectPool` reconciliation creates a new renderer when `effect_id` changes. If `apply_effect` is implemented as "set primary.effect_id", the EffectPool will tear down and rebuild the renderer on next tick — same semantics as today's `engine.activate()`. But any case where `EffectPool` re-reconciles unnecessarily would cause a visible glitch.

**Mitigation:** `EffectPool::reconcile` already keys on `slot.effect_id != group.effect_id`. Test path: apply effect A, patch controls (revision bumps but effect_id same), verify the renderer is NOT rebuilt (slot Initializing → Running → Running — no Destroying).

### 10.3 `GET /effects/active` idle-state surprise

**Risk:** Primary group with `effect_id: None` (e.g., fresh Default scene, or user called `/effects/current/stop`) returns an "idle" response. Existing UI may assume this endpoint always has an active effect.

**Mitigation:** define `ActiveEffectResponse::idle()` as `{ state: "idle", id: null, ... }`. The UI already handles `None` in `fetch_active_effect`. TUI's `DaemonState::effect_name: Option<String>` is already optional.

### 10.4 Display-target device disappearing

**Risk:** user assigns face to device X, removes device X. Scene now has a display group pointing at a non-existent device.

**Mitigation:** Not new — this is a pre-existing bug. Out of scope for this spec. File a follow-up.

### 10.5 Named-scene mutation during apply_effect

**Risk:** user activates a carefully curated "Focus" scene, then clicks a different effect in the sidebar. Apply_effect mutates Focus, not Default. Next time user activates Focus, it has the new effect, not the original.

**Mitigation:** documented behavior per §3.2. Follow-up spec can add "locked" named scenes. For now: the UI warns the user when applying an effect while a named scene is active, with an optional "Return to Default first" affordance. **Out of scope for this spec; file a follow-up.**

### 10.6 Scene persistence vs. concurrent mutation

**Risk:** two writes to `scenes.json` race (e.g., API call + profile apply).

**Mitigation:** SceneStore wraps state in `RwLock`, `save()` atomic-renames (same pattern as ProfileStore). Serialize via `state.scene_store.write().await` in API handlers.

---

## 11. Observability

- Log at info level: `scene_activated`, `scene_deactivated`, `default_scene_primary_updated`, `display_group_upserted`, `display_group_removed`.
- Log at warn level: persistence parse failures and any scene validation failures.
- Metrics: `render_group_count` already in `MetricsTimeline`. Add `render_group_primary_effect_id`, `render_group_display_count` for visibility.

---

## 12. Out of Scope

### 12.1 Scene UI
Built later; this spec only makes the backend ready. The `GET /scenes`, `POST /scenes`, etc. work today — UI just hasn't picked them up.

### 12.2 Per-zone / per-device render groups
The render-groups path supports it. Surfacing it as a user-facing API is a separate spec.

### 12.3 Named-scene lockdown
"This scene is a snapshot, don't auto-mutate it when I apply an effect." Future feature per §10.5.

### 12.4 Device-lifecycle cleanup for stale display targets
Separate bug. Faces pointing at gone devices currently produce warnings; fixing that is its own spec.

### 12.5 Deprecating the global `canvas` channel
Keep it for now, revisit when UI moves to per-group previews.

### 12.6 CLI/TUI scene commands
Possible follow-up. Not blocking.

---

## 13. Definition of Done

1. `just verify` green on every commit.
2. `EffectEngine` type deleted; `rg EffectEngine` returns zero hits in `src/`.
3. `POST /displays/{id}/face` succeeds from a clean-slate daemon (no setup required).
4. Restarting the daemon restores the last effect + all face assignments.
5. `GET /scenes` returns no Default.
6. Every test listed in §8 exists and passes.
7. `runtime-state.json` and `profiles.json` use only the final scene-backed schema; pre-final shapes are rejected.
8. Manual verification checklist (§8.4) complete.

---

## 14. Follow-ups

- **Spec 46** — Scene UI (Leptos page, named scene CRUD, activate/deactivate, transition picker).
- **Spec 47** — Per-zone render group composition API + UI.
- **Spec 48** — Named-scene lockdown / snapshot mode.
- **Spec 49** — Device-disappearance cleanup for display_target orphans.
- **Spec 50** — Deprecate global `canvas` channel in favor of per-group previews.
