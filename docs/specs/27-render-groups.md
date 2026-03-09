# Spec 27 — Zones (Render Groups)

> Independent rendering pipelines within a Scene — each with its own effect, layout, and canvas.

**Status:** Draft
**Crate:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`
**Depends on:** Spatial Layout Engine (06), Effect System (07), Scenes & Automation (13)
**Evolves:** Scene struct (13 §1), ZoneAssignment (13 §2), render thread pipeline

---

## Naming Convention

| Context | Term | Notes |
|---------|------|-------|
| **User-facing** (UI, API, CLI, docs) | **Zone** | "Add a zone", "Desk zone", "drag devices into a zone" |
| **Internal** (Rust types, engine code) | **RenderGroup** | Avoids collision with existing `DeviceZone` (spatial layer) |

Users never see "RenderGroup." The API uses `/zones`. The UI says "Zone." The Rust
type is `RenderGroup` purely to disambiguate from `DeviceZone` (a single device's
spatial mapping on the canvas). If `DeviceZone` is ever renamed (e.g., to `DeviceSlot`),
the internal type could become `Zone` too.

---

## Table of Contents

1. [Motivation](#1-motivation)
2. [Concepts](#2-concepts)
3. [Type Definitions](#3-type-definitions)
4. [Scene Evolution](#4-scene-evolution)
5. [Render Pipeline Changes](#5-render-pipeline-changes)
6. [Device Exclusivity](#6-device-exclusivity)
7. [Default Behavior](#7-default-behavior)
8. [Canvas Management](#8-canvas-management)
9. [Transition Integration](#9-transition-integration)
10. [API Surface](#10-api-surface)
11. [UI Considerations](#11-ui-considerations)
12. [Migration](#12-migration)

---

## 1. Motivation

Today the render pipeline is strictly linear:

```
1 Effect → 1 Canvas (320×200) → N DeviceZones → N Devices
```

Every device sees the same effect. Users cannot run screen-mirror on their keyboard while
fans run ambient plasma and room strips pulse to music. The Scene system (Spec 13) already
models per-zone effect assignments via `ZoneAssignment`, but the render thread ignores them
— it runs one `EffectEngine` producing one `Canvas` for all devices.

**Zones** break this constraint. Each zone is an independent rendering pipeline: its own
effect, its own spatial layout, its own canvas. A Scene becomes a collection of zones,
enabling simultaneous multi-effect rendering with clean device isolation.

---

## 2. Concepts

### 2.1 Zone (RenderGroup)

A **Zone** is the atomic unit of the multi-effect pipeline. It binds three things:

| Component | What it is | Why it's per-zone |
|-----------|------------|-------------------|
| **Effect** | Which effect to render | Different zones run different effects |
| **Layout** | A `SpatialLayout` defining device positions on the canvas | Device positions are effect-relative — a keyboard "centered" on the screen-mirror canvas is positioned differently than "centered" on a plasma canvas |
| **Controls** | Effect parameter overrides | Same effect can run with different settings per zone |

Each zone produces its own full-resolution `Canvas` (320×200 by default). The spatial
sampler runs independently per zone, sampling that zone's canvas for that zone's devices.
Downstream device routing is unchanged.

### 2.2 Scene as Container

A **Scene** is a named, saveable, switchable collection of zones. It replaces the flat
`Vec<ZoneAssignment>` with structured `Vec<RenderGroup>`. The existing priority stack,
transition engine, and automation rules operate on Scenes — they don't need to know about
the internal zone structure.

### 2.3 Device Exclusivity

A device belongs to **exactly one** zone within a Scene. No priority math, no blending
between zones, no conflicts. If a device isn't assigned to any zone, it's off (or
inherits a fallback — see §7).

---

## 3. Type Definitions

### 3.1 RenderGroupId

```rust
/// Opaque render group identifier. UUID v7 for time-sortable ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RenderGroupId(pub Uuid);

impl RenderGroupId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}
```

### 3.2 RenderGroup

```rust
/// An independent rendering pipeline within a Scene.
///
/// Each group runs its own effect, maintains its own spatial layout,
/// and produces its own canvas. Device zones are exclusive — a zone
/// appears in at most one group per Scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderGroup {
    /// Unique identifier.
    pub id: RenderGroupId,

    /// Human-readable name (e.g., "Desk", "Case Fans", "Room").
    pub name: String,

    /// Optional description for UI display.
    pub description: Option<String>,

    /// The effect to render. `None` means this group is paused/empty.
    pub effect_id: Option<EffectId>,

    /// Effect control overrides. Keys must match the effect's control
    /// definitions. Missing keys use the effect's defaults.
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,

    /// Active preset for this group's effect (if any).
    pub preset_id: Option<PresetId>,

    /// Spatial layout defining where device zones sit on this group's canvas.
    /// Each group gets its own layout because device positions are
    /// relative to the effect's canvas — not a global coordinate space.
    pub layout: SpatialLayout,

    /// Per-group brightness multiplier. Applied after effect rendering,
    /// before device write. Range: 0.0–1.0.
    #[serde(default = "default_brightness")]
    pub brightness: f32,

    /// Whether this group is actively rendering.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Display color for UI grouping (hex string, e.g., "#e135ff").
    pub color: Option<String>,
}

fn default_brightness() -> f32 { 1.0 }
fn default_true() -> bool { true }
```

### 3.3 Why Layout Lives Inside RenderGroup

The layout is per-group, not shared, because spatial positioning is relative to the effect
canvas. A keyboard "centered" on a screen-capture canvas occupies a different logical
position than "centered" on a plasma canvas. Groups are independent viewports.

This also means users can size and position zones optimally per-effect. A strip running
a 1D gradient only needs a horizontal slice; fans running a radial effect want a centered
square region. The layout adapts to the effect's visual characteristics.

---

## 4. Scene Evolution

### 4.1 Before (Spec 13)

```rust
pub struct Scene {
    pub id: SceneId,
    pub name: String,
    pub scope: SceneScope,
    pub zone_assignments: Vec<ZoneAssignment>,  // flat per-zone mapping
    pub transition: TransitionSpec,
    pub priority: ScenePriority,
    pub enabled: bool,
    pub metadata: HashMap<String, String>,
}
```

### 4.2 After

```rust
pub struct Scene {
    pub id: SceneId,
    pub name: String,
    pub description: Option<String>,

    /// The render groups that make up this scene.
    /// Each group is an independent effect + layout + controls.
    pub groups: Vec<RenderGroup>,

    /// Default transition when activating this scene.
    pub transition: TransitionSpec,

    /// Priority for inter-scene conflict resolution.
    pub priority: ScenePriority,

    /// Whether this scene is enabled.
    pub enabled: bool,

    /// Freeform metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}
```

**Removed fields:**
- `scope: SceneScope` — replaced by the union of all group layouts. Scope is now implicit:
  the scene covers whichever devices appear in its groups.
- `zone_assignments: Vec<ZoneAssignment>` — replaced by `groups: Vec<RenderGroup>`.

**`SceneScope` is not deleted** — it remains useful for automation rule targeting and
the priority stack's conflict detection. It becomes a derived property:

```rust
impl Scene {
    /// Derive the effective scope from the union of all group layouts.
    pub fn effective_scope(&self) -> SceneScope {
        let all_zone_ids: Vec<String> = self.groups.iter()
            .flat_map(|g| g.layout.zones.iter().map(|z| z.id.clone()))
            .collect();

        if all_zone_ids.is_empty() {
            SceneScope::Full
        } else {
            SceneScope::Zones(all_zone_ids)
        }
    }
}
```

### 4.3 ZoneAssignment

`ZoneAssignment` is **not deleted**. It becomes a flattened view derived from render groups,
used by the transition engine for cross-fade blending between scenes:

```rust
impl Scene {
    /// Flatten render groups into per-zone assignments for transition blending.
    pub fn zone_assignments(&self) -> Vec<ZoneAssignment> {
        self.groups.iter()
            .filter(|g| g.enabled)
            .flat_map(|group| {
                let effect_name = group.effect_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "static".to_string());

                group.layout.zones.iter().map(move |zone| {
                    ZoneAssignment {
                        zone_name: zone.id.clone(),
                        effect_name: effect_name.clone(),
                        parameters: group.controls.iter()
                            .map(|(k, v)| (k.clone(), v.to_string()))
                            .collect(),
                        brightness: Some(group.brightness),
                    }
                })
            })
            .collect()
    }
}
```

---

## 5. Render Pipeline Changes

### 5.1 Current Pipeline (Single Effect)

```
RenderThread::run_frame():
    canvas = effect_engine.tick(delta, audio, interaction)
    zone_colors = spatial_engine.sample(canvas, layout)
    backend_manager.write_frame(zone_colors)
```

### 5.2 New Pipeline (Multi-Group)

```
RenderThread::run_frame():
    scene = scene_manager.active_scene()

    for group in scene.groups where group.enabled:
        canvas = effect_pool.tick(group.id, group.effect_id, delta, audio, interaction)
        zone_colors += spatial_engine.sample(canvas, &group.layout)

    backend_manager.write_frame(zone_colors)
```

### 5.3 EffectPool

The `EffectEngine` currently owns a single renderer. A new `EffectPool` manages multiple
concurrent renderers, one per active render group:

```rust
/// Manages multiple concurrent effect renderers, one per active RenderGroup.
pub struct EffectPool {
    /// Active renderer slots, keyed by RenderGroupId.
    slots: HashMap<RenderGroupId, EffectSlot>,

    /// Shared effect registry for metadata lookups.
    registry: Arc<EffectRegistry>,
}

struct EffectSlot {
    /// The effect currently loaded in this slot.
    effect_id: EffectId,

    /// The renderer (WGPU or Servo) for this slot.
    renderer: Box<dyn EffectRenderer>,

    /// This slot's output canvas.
    canvas: Canvas,

    /// Current control values.
    controls: HashMap<String, ControlValue>,
}

impl EffectPool {
    /// Tick all active slots, producing one canvas per group.
    pub fn tick_all(
        &mut self,
        groups: &[RenderGroup],
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
    ) -> HashMap<RenderGroupId, &Canvas> {
        // Reconcile slots with groups:
        // - Spawn renderers for new groups
        // - Destroy renderers for removed groups
        // - Update controls for changed groups
        // - Swap effects if group.effect_id changed
        self.reconcile(groups);

        // Tick each slot
        let mut canvases = HashMap::new();
        for (group_id, slot) in &mut self.slots {
            slot.renderer.tick(delta_secs, audio, interaction);
            canvases.insert(*group_id, &slot.canvas);
        }
        canvases
    }
}
```

### 5.4 Reconciliation

The `EffectPool` reconciles its slot map against the active scene's groups each frame.
This is a diff operation, not a rebuild:

| Situation | Action |
|-----------|--------|
| New group appears | Spawn renderer, load effect |
| Group removed | Destroy renderer, free canvas |
| Group effect changed | Hot-swap effect in existing slot |
| Group controls changed | Update control values (no respawn) |
| Group disabled | Pause renderer (keep state, skip tick) |
| Group re-enabled | Resume renderer |

Renderer creation is async (especially Servo). During spawn, the group's canvas shows the
last frame from the previous effect (or black if new).

### 5.5 Resource Budget

Each canvas is 320×200×4 = 250 KB. Even 10 simultaneous groups = 2.5 MB. Memory is not
a concern.

Servo renderers are heavyweight (~50 MB each). The pool should enforce a configurable cap
on concurrent Servo instances (default: 2). WGPU renderers share a single GPU device and
are lightweight — no practical cap needed.

```rust
pub struct EffectPoolConfig {
    /// Maximum concurrent Servo (HTML) renderers. Default: 2.
    pub max_servo_instances: usize,

    /// Maximum total renderers across all types. Default: 8.
    pub max_total_renderers: usize,
}
```

If a group exceeds the budget, it enters a `Queued` state and renders when a slot opens.

---

## 6. Device Exclusivity

### 6.1 Rule

A device zone appears in **exactly one** Render Group within a Scene. This is enforced
at Scene validation time, not at render time.

### 6.2 Validation

```rust
impl Scene {
    /// Validate that no device zone appears in multiple groups.
    pub fn validate_exclusivity(&self) -> Result<(), Vec<String>> {
        let mut seen: HashMap<&str, &str> = HashMap::new(); // zone_id → group_name
        let mut conflicts = Vec::new();

        for group in &self.groups {
            for zone in &group.layout.zones {
                if let Some(existing_group) = seen.insert(&zone.id, &group.name) {
                    conflicts.push(format!(
                        "Zone '{}' claimed by both '{}' and '{}'",
                        zone.id, existing_group, group.name
                    ));
                }
            }
        }

        if conflicts.is_empty() { Ok(()) } else { Err(conflicts) }
    }
}
```

### 6.3 Unassigned Devices

Devices not assigned to any group in the active scene can either:

- **Off** — LEDs are black (safe default)
- **Hold** — retain the last color they were set to (useful during scene editing)
- **Fallback group** — a special group marked as the catch-all

This is a Scene-level setting:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnassignedBehavior {
    /// Turn off unassigned device LEDs.
    Off,
    /// Hold the last rendered color.
    Hold,
    /// Route to a named group that acts as the catch-all.
    Fallback(RenderGroupId),
}
```

---

## 7. Default Behavior

When no explicit scene is configured (fresh install, or user hasn't set up groups), the
system creates an **implicit default scene** with a single render group containing all
discovered devices and the currently selected effect.

```rust
impl Scene {
    /// Create the implicit default scene wrapping a single effect + layout.
    pub fn default_single(effect_id: EffectId, layout: SpatialLayout) -> Self {
        Self {
            id: SceneId::new(),
            name: "Default".to_string(),
            description: None,
            groups: vec![RenderGroup {
                id: RenderGroupId::new(),
                name: "All Devices".to_string(),
                description: None,
                effect_id: Some(effect_id),
                controls: HashMap::new(),
                preset_id: None,
                layout,
                brightness: 1.0,
                enabled: true,
                color: None,
            }],
            transition: TransitionSpec::default(),
            priority: ScenePriority::USER,
            enabled: true,
            metadata: HashMap::new(),
        }
    }
}
```

This preserves the current UX: pick an effect, it applies to everything. Users opt into
multi-group rendering by creating additional groups within the scene.

---

## 8. Canvas Management

### 8.1 Per-Group Canvas

Each RenderGroup has its own canvas. The canvas dimensions come from the group's layout:

```rust
let canvas = Canvas::new(group.layout.canvas_width, group.layout.canvas_height);
```

Groups MAY have different canvas sizes, though 320×200 is the standard default. A group
running a 1D strip effect might use 320×1 for efficiency.

### 8.2 Preview Compositing

The UI needs a unified preview of all groups. This is a display-only composite — it does
not affect the render pipeline:

```rust
/// Composite all group canvases into a single preview image for the UI.
pub fn composite_preview(
    groups: &[(RenderGroup, &Canvas)],
    output_width: u32,
    output_height: u32,
) -> Canvas {
    // Tile or arrange group canvases based on group count.
    // Each group gets a labeled thumbnail.
    // This is purely for UI display — never fed back into the pipeline.
}
```

---

## 9. Transition Integration

Scene transitions (crossfade between scenes) work at the **zone level**, not the group
level. When transitioning from Scene A to Scene B:

1. Flatten Scene A's groups → `Vec<ZoneAssignment>` (via `scene.zone_assignments()`)
2. Flatten Scene B's groups → `Vec<ZoneAssignment>`
3. Feed both into the existing `TransitionState` (Spec 13 §4)
4. The transition engine blends per-zone brightness and swaps effects at the midpoint

This means the existing transition infrastructure works unchanged. Groups are an internal
optimization — transitions see a flat zone-to-effect mapping.

During a transition, the `EffectPool` may temporarily run renderers for **both** scenes
(the outgoing and incoming effects). Once the transition completes, outgoing renderers
are destroyed.

---

## 10. API Surface

### 10.1 Scene Endpoints (Evolved)

Existing scene CRUD endpoints gain `zones` in request/response bodies:

```
GET    /api/v1/scenes/:id          → Scene with zones
POST   /api/v1/scenes              → Create scene with zones
PUT    /api/v1/scenes/:id          → Update scene (replaces zones)
```

### 10.2 Zone Endpoints (New)

Fine-grained zone management within a scene:

```
GET    /api/v1/scenes/:id/zones                    → List zones in scene
POST   /api/v1/scenes/:id/zones                    → Add zone to scene
GET    /api/v1/scenes/:id/zones/:zid               → Get zone detail
PUT    /api/v1/scenes/:id/zones/:zid               → Update zone
DELETE /api/v1/scenes/:id/zones/:zid               → Remove zone
POST   /api/v1/scenes/:id/zones/:zid/effect        → Change zone's effect
PUT    /api/v1/scenes/:id/zones/:zid/controls      → Update zone controls
POST   /api/v1/scenes/:id/zones/:zid/devices       → Assign devices to zone
DELETE /api/v1/scenes/:id/zones/:zid/devices/:did   → Remove device from zone
```

### 10.3 Quick Apply (Convenience)

For the common case of "just apply this effect to everything":

```
POST /api/v1/effects/:id/apply    → (existing) Creates/updates default scene
```

This endpoint continues to work by updating the default scene's single render group.

---

## 11. UI Considerations

### 11.1 Zone Management

The UI presents zones as visual buckets. Users:

1. Create a zone ("Desk", "Case", "Room")
2. Drag devices into zones
3. Pick an effect per zone
4. Adjust controls per zone

### 11.2 Layout Editor Per Zone

Each zone has its own spatial layout editor — the existing canvas editor, but scoped
to that zone's devices. Selecting a zone in the sidebar switches the layout editor
to show only that zone's devices on its canvas.

### 11.3 Live Preview

The dashboard shows a tiled or stacked preview of all active zones:

```
┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐
│   ⌨️  Desk       │ │   🌀  Case       │ │   🎵  Room       │
│  screen-mirror  │ │  ambient-plasma  │ │  audio-pulse    │
│  [canvas prev]  │ │  [canvas prev]  │ │  [canvas prev]  │
└─────────────────┘ └─────────────────┘ └─────────────────┘
```

---

## 12. Migration

### 12.1 Config Migration

Existing scene configs using `zone_assignments` are migrated to a single-group scene:

```rust
fn migrate_v1_scene(old: SceneV1) -> Scene {
    // All zone assignments collapse into one group with the default layout
    let effect_id = old.zone_assignments.first()
        .map(|za| resolve_effect_id(&za.effect_name));

    Scene {
        id: old.id,
        name: old.name,
        groups: vec![RenderGroup {
            id: RenderGroupId::new(),
            name: "All Devices".to_string(),
            effect_id,
            layout: current_layout.clone(),
            controls: HashMap::new(),
            // ... defaults
        }],
        // ... rest unchanged
    }
}
```

### 12.2 Versioning

The scene config gains a `version` field:

- **v1**: Original `zone_assignments` format (Spec 13)
- **v2**: Render groups format (this spec)

The loader detects version and migrates transparently. V1 scenes are auto-upgraded on
first load; the original file is not modified until the user saves.
