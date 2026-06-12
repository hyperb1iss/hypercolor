# Spec 70 — Agent Rig Setup & MCP Full Control

> Let any MCP-connected agent configure a Hypercolor rig end-to-end. A user
> says "I've got a Lian Li O11D EVO RGB with nine Uni Fans, a Strimer, and a
> Razer keyboard — set it up," and the agent discovers hardware, pairs network
> devices, researches the case geometry, places every device on the spatial
> canvas, builds zones and layers, verifies placement with identify flashes,
> and saves the result. The REST API and CLI can already author all of this;
> the MCP surface cannot author any of it. This spec closes that gap (full
> control), adds the missing geometry knowledge layer (rig templates), and
> encodes the setup workflow (calibration loop, MCP prompt, agent skill).

**Status:** Draft
**Author:** Nova
**Date:** 2026-06-12
**Crates:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`
**Depends on:** Spatial Layout Engine (06), Scenes & Automation (13),
Render Groups (27), Dynamic Driver Control Surfaces (52), Multi-Zone
Scenes (64), Component Designer (66)
**Related:** Studio Composition UI (65), RFC 53 productization (cloud
template registry, deferred), brainstorm `1359138d-f0b7-49a3-95d2-7e96c8f7c820`

---

## Naming Convention

This spec spans four "thing with a position" concepts. The collisions are
real, so the vocabulary is fixed up front:

| Term                  | Type / Surface                              | Meaning                                                                |
| --------------------- | ------------------------------------------- | ---------------------------------------------------------------------- |
| **Output**            | `spatial::Output`                           | One device zone placed on the canvas (position, size, rotation, topology) |
| **Zone**              | REST `/scenes/{id}/zones`, `RenderGroup`    | A render partition of a scene: a set of Outputs with its own layer stack |
| **Component**         | `attachment::ComponentTemplate`             | A pluggable accessory (fan, strip, strimer) bound to a controller slot  |
| **Mount**             | `rig::RigMount` (new)                       | A physical location a rig template offers (fan mount, strip channel)    |
| **Rig template**      | `rig::RigTemplate` (new)                    | The geometry of a case, desk, or room: a named set of mounts            |

User-facing language follows Spec 64: users see "zones," never
"render groups." Agents see all five terms in tool schemas, with
descriptions that disambiguate.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Architecture Overview](#4-architecture-overview)
5. [Layer 1 — MCP Full Control Surface](#5-layer-1--mcp-full-control-surface)
6. [Layer 2 — Rig Templates (Geometry Knowledge)](#6-layer-2--rig-templates-geometry-knowledge)
7. [Layer 3 — Setup Experience](#7-layer-3--setup-experience)
8. [REST and CLI Additions](#8-rest-and-cli-additions)
9. [Validation and Error Design](#9-validation-and-error-design)
10. [Security Posture](#10-security-posture)
11. [Implementation Plan](#11-implementation-plan)
12. [Testing Strategy](#12-testing-strategy)
13. [Open Questions](#13-open-questions)

---

## 1. Overview

Hypercolor already has every mechanical primitive an agent needs to
configure a rig:

- **Layouts** — full CRUD over `SpatialLayout` documents
  (`api/layouts.rs:143`, `api/layouts.rs:200`), apply, and preview.
- **Scene zones** — zone lifecycle, device assignment (including inline
  `Output` creation via `OutputAssignment::New`), per-zone spatial layout
  (`api/scenes_zones.rs:348` accepts a whole `SpatialLayout`), layer
  stacks, layer controls, unassigned-output behavior.
- **Device lifecycle** — discovery sweeps (`POST /devices/discover`),
  pairing with typed flows (`pairing::PairDeviceRequest`,
  `PairingFlowKind`), identify flashes at device, device-zone, and
  attachment-slot granularity, attachment slot/binding configuration
  (`GET/PUT /devices/{id}/attachments`).
- **Components** — `ComponentTemplate` already models accessory geometry:
  topology, LED remapping, `physical_size_mm`, slot compatibility, and
  `ComponentSuggestedZone` for layout import (Spec 66).

What's missing is threefold, and none of it is engine work:

1. **The MCP server cannot author.** All 16 tools are control-plane.
   A claude.ai or Claude Desktop user connected to the daemon's MCP
   endpoint can flip effects on and off but cannot discover a device,
   pair a bridge, place an Output, or create a zone.
2. **Nothing knows physical geometry at rig scale.** The vendor database
   (`data/drivers/vendors/*.toml`) is an identification database — VID,
   PID, driver, transport — not a geometry database. `ComponentTemplate`
   covers accessory-scale geometry, but no type anywhere says "an O11D
   EVO has three side intake mounts here, three bottom mounts here."
3. **The workflow is not encoded.** The order of operations (discover →
   pair → attachments → identify → place → verify), the calibration
   loop, and the mapping from "front intake fans" to canvas coordinates
   live in nobody's head but ours.

This spec adds the three layers in order of leverage: the **surface**
(Section 5), the **knowledge** (Section 6), and the **experience**
(Section 7).

## 2. Problem Statement

### 2.1 MCP Is Control-Only

The dispatch table at `daemon/src/mcp/tools/mod.rs:67` routes exactly
these tools:

| Tool               | Kind      | Notes                                              |
| ------------------ | --------- | -------------------------------------------------- |
| `get_status`       | read      |                                                    |
| `get_devices`      | read      | No pairing descriptors, no attachment state        |
| `get_layout`       | read      | Active layout only, read-only                      |
| `get_audio_state`  | read      |                                                    |
| `get_sensor_data`  | read      |                                                    |
| `list_effects`     | read      |                                                    |
| `list_scenes`      | read      |                                                    |
| `diagnose`         | read      |                                                    |
| `set_effect`       | control   |                                                    |
| `stop_effect`      | control   |                                                    |
| `set_color`        | control   |                                                    |
| `set_brightness`   | control   |                                                    |
| `set_display_face` | control   |                                                    |
| `set_profile`      | control   | Apply only; cannot save                            |
| `activate_scene`   | control   |                                                    |
| `create_scene`     | authoring | Automation flavor only: `name + profile_id + trigger` (`mcp/tools/scenes.rs:328`) |

`create_scene` is the lone authoring verb, and it creates the
*automation* shape of a scene (trigger + profile). It cannot create
zones, assign devices, or stack layers — the compositor model from
Specs 64/65 is invisible to MCP.

Resources are similarly thin: `hypercolor://state`, `devices`,
`effects`, `profiles`, `audio` (`mcp/resources.rs:31`). No layouts, no
scenes, no components, no previews.

### 2.2 The Gap Is Parity, Not Capability

Every operation the setup flow needs already exists on REST and is
already wrapped by the CLI (`layouts create/update/apply/preview`,
`devices discover/pair/identify`, scene zone CRUD). The daemon-side
logic, validation, and persistence are done. This spec is mostly
plumbing well-designed MCP tools onto existing handlers — which is why
"full control" is achievable in one spec rather than a quarter.

### 2.3 No Rig-Scale Geometry

When an agent wants to place nine Uni Fans, it needs to answer: where
on the canvas does "side rack, fan 2 of 3" go? Today the answer
requires a human dragging boxes in the layout editor. The knowledge
exists in the world (case manuals, product pages, community builds) and
agents are *good* at retrieving it — but there is no schema to retrieve
it *into*, so each retrieval would be a one-off prose answer rather
than a durable, reusable artifact.

### 2.4 No Encoded Workflow

Even with tools and templates, an agent needs to know the choreography:
that attachments must be configured before suggested zones exist, that
identify flashes disambiguate "which fan is fan 1," that a directional
sweep effect verifies orientation, that the layout should be applied
before the scene is built on top of it. Without encoding this, every
agent reinvents the flow and most get it wrong.

## 3. Goals and Non-Goals

### 3.1 Goals

1. **MCP full control.** Every domain the REST API can author — devices,
   pairing, attachments, layouts, scenes, zones, layers, effects
   controls, library, profiles, config — is authorable over MCP with
   composite, agent-ergonomic tools.
2. **Rig template library.** A typed, validated TOML schema for chassis/
   desk/room geometry; a loader merging built-in and user templates; seed
   templates for popular cases; exposure over REST, MCP, and CLI.
3. **Calibration primitives over MCP.** Identify flashes (device / zone /
   slot), layout preview rendering, and discovery — enough for an agent
   to run an interactive verify loop with the user.
4. **Encoded workflow.** A `setup_rig` MCP prompt and a user-facing
   `rig-setup` agent skill that share one canonical workflow document.
5. **Idempotent, declarative authoring.** Mutating tools accept desired
   state and reconcile, so agent retries and partial failures are safe.

### 3.2 Non-Goals

- **Vision/photo-based placement.** A user photographing their rig and a
  vision model proposing the layout is a sick follow-up, not this spec.
- **Cloud template registry.** Community template sharing through
  hypercolor.lighting belongs to RFC 53's timeline. The TOML schema here
  is designed so those templates sync down later without migration.
- **New engine capability.** No render-pipeline, sampling, or device
  protocol changes. (The preview renderer in §5.5 is a read-side
  addition, not an engine change.)
- **UI changes.** Studio remains the human authoring surface; nothing
  here touches `hypercolor-ui` beyond optionally listing rig templates
  later.
- **Auto-placement without confirmation.** The agent proposes, flashes,
  and asks. Silent placement guesses presented as fact are explicitly
  out of contract (encoded in the workflow doc, §7.1).

## 4. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│  Layer 3: EXPERIENCE                                            │
│  setup_rig MCP prompt · rig-setup skill · calibration loop      │
└──────────────────────────────┬──────────────────────────────────┘
                               │ drives
┌──────────────────────────────▼──────────────────────────────────┐
│  Layer 1: SURFACE (MCP tools & resources)                       │
│  discover · pair · identify · configure_attachments             │
│  upsert_layout · place_outputs · apply_layout · preview_layout  │
│  configure_scene · configure_zones · configure_layers           │
│  set_effect_controls · save_profile · config · library          │
└──────────┬───────────────────────────────────────┬──────────────┘
           │ existing handlers                     │ reads
┌──────────▼──────────────────┐   ┌────────────────▼──────────────┐
│  Existing REST/daemon logic │   │  Layer 2: KNOWLEDGE           │
│  (no engine changes)        │   │  data/rigs/*.toml templates   │
│                             │   │  ComponentTemplate library    │
│                             │   │  vendor DB (unchanged)        │
└─────────────────────────────┘   └───────────────────────────────┘
```

The dependency direction matters: Layer 3 is pure content (prompt text,
skill markdown, docs), Layer 1 is daemon plumbing onto existing logic,
Layer 2 is new types + data + a loader. They ship independently and in
any order, though the experience layer is only as good as the surface
beneath it.

## 5. Layer 1 — MCP Full Control Surface

### 5.1 Design Principles

1. **Composite verbs, not a REST mirror.** Agents reason better over
   "configure the zones of this scene to look like X" than over nine
   sequenced CRUD calls. Each tool wraps as many REST handlers as one
   *intent* needs. A 1:1 mirror (~40 fine-grained tools) is rejected:
   tool-list bloat measurably degrades agent tool selection, and
   sequencing burden moves error handling into the agent's context
   window where it does not belong.
2. **Declarative reconcile.** Mutating tools accept desired state, diff
   against current state, apply the difference, and return the resulting
   state. Calling the same tool twice with the same arguments is a
   no-op. This makes agent retries safe by construction.
3. **Return the post-state.** Every mutation returns the affected
   object's canonical form (the same shape a read tool would return), so
   the agent never needs a follow-up read to confirm.
4. **Warnings, not refusals.** Authoring tools accept imperfect input
   where the system can proceed (e.g., an Output referencing a
   not-yet-connected device) and return structured `warnings` alongside
   the result. Hard errors are reserved for schema violations and
   impossible states. See §9.
5. **Honest annotations.** Every tool sets `read_only` and `idempotent`
   truthfully (`ToolDefinition` at `mcp/tools/mod.rs:20` already carries
   both; `rmcp` maps them to MCP tool annotations). Clients use these to
   gate confirmation prompts.

### 5.2 New Tool Inventory

Eighteen new tools across six domains, bringing the total to 34.
Existing tools are untouched (no breaking changes); `create_scene`
remains for automation scenes and is documented as such.

| #  | Tool                       | Domain      | RW | Wraps (REST)                                                       |
| -- | -------------------------- | ----------- | -- | ------------------------------------------------------------------ |
| 1  | `discover_devices`         | devices     | W  | `POST /devices/discover`                                           |
| 2  | `pair_device`              | devices     | W  | `POST /devices/{id}/pair`                                          |
| 3  | `identify`                 | devices     | W  | `POST /devices/{id}/identify` + `/zones/{z}/identify` + `/attachments/{s}/identify` |
| 4  | `configure_device`         | devices     | W  | `PATCH /devices/{id}` + `/devices/{id}/controls`                   |
| 5  | `configure_attachments`    | devices     | W  | `PUT /devices/{id}/attachments`                                    |
| 6  | `list_component_templates` | components  | R  | component template registry (Spec 66)                              |
| 7  | `get_layouts`              | layouts     | R  | `GET /layouts`, `GET /layouts/{id}`                                |
| 8  | `upsert_layout`            | layouts     | W  | `POST /layouts`, `PUT /layouts/{id}`                               |
| 9  | `place_outputs`            | layouts     | W  | layout mutation (server-side merge)                                |
| 10 | `apply_layout`             | layouts     | W  | `POST /layouts/{id}/apply`                                         |
| 11 | `preview_layout`           | layouts     | R  | new render endpoint (§5.5)                                         |
| 12 | `search_rig_templates`     | rigs        | R  | new `GET /rigs` (§6)                                               |
| 13 | `save_rig_template`        | rigs        | W  | new `POST /rigs` (§6)                                              |
| 14 | `configure_scene`          | scenes      | W  | `POST/PATCH /scenes/{id}` + unassigned-behavior                    |
| 15 | `configure_zones`          | scenes      | W  | zone CRUD + device assignment + zone layout                        |
| 16 | `configure_layers`         | scenes      | W  | layer CRUD + order + controls                                      |
| 17 | `set_effect_controls`      | effects     | W  | `PATCH /effects/current/controls`                                  |
| 18 | `save_profile`             | library     | W  | `POST /profiles`                                                   |

Plus two smaller parity items folded into existing tools rather than
new ones:

- `get_devices` gains `detail: bool` — when true, payloads include
  `DeviceAuthSummary` (pairing state + `PairingDescriptor` field
  requirements) and the attachment profile (slots, bindings,
  suggested zones). The agent learns *what pairing needs* and *what's
  plugged in where* from the same read it already makes.
- `get_status` gains daemon config echo (canvas dimensions, FPS tier,
  MCP authoring mode per §10) so agents stop guessing canvas size.

Deliberately **deferred** (not in the 18): library favorites/presets/
playlists management and daemon config writes. Both are full-control
items but neither blocks rig setup; they ship in the follow-up wave
(§11, W4) as `manage_library` and `set_config` once the authoring
patterns above have soaked.

### 5.3 Key Tool Schemas

The four tools that carry the setup flow, in full. Remaining schemas
follow the same conventions and live with the implementation.

#### `discover_devices`

```jsonc
{
  "name": "discover_devices",
  "read_only": false,            // triggers radio/bus scans
  "idempotent": true,
  "input": {
    "targets": {                  // optional; omit = all enabled targets
      "type": "array",
      "items": { "enum": ["usb", "wled", "hue", "nanoleaf", "govee", "openrgb"] }
    },
    "wait_secs": { "type": "number", "default": 5, "maximum": 30 }
  },
  "output": {
    "discovered": [ /* DeviceInfo summaries, incl. auth_state */ ],
    "connected":  [ /* already-managed devices seen this sweep */ ],
    "targets_scanned": ["usb", "wled"]
  }
}
```

#### `identify`

One tool, three granularities — the composite shape mirrors how agents
ask the question ("flash that thing"):

```jsonc
{
  "name": "identify",
  "input": {
    "device_id": { "type": "string" },
    "zone_name": { "type": "string", "description": "Optional sub-device channel (e.g. 'ch1'). Flashes only that channel." },
    "slot_id":   { "type": "string", "description": "Optional attachment slot. Flashes only LEDs bound to that slot. Mutually exclusive with zone_name." },
    "color":     { "type": "string", "description": "Hex color, default #FF00FF" },
    "duration_ms": { "type": "number", "default": 3000, "maximum": 15000 }
  },
  "output": { "flashed_led_count": 16, "duration_ms": 3000 }
}
```

#### `upsert_layout`

```jsonc
{
  "name": "upsert_layout",
  "idempotent": true,
  "input": {
    "layout": { "$ref": "SpatialLayout" }   // full document; id present = update, absent = create
  },
  "output": {
    "layout": { /* normalized SpatialLayout, server-assigned id */ },
    "warnings": [
      { "code": "unknown_device", "output_id": "fan-7", "detail": "usb:lianli-hub-2 is not currently connected" },
      { "code": "overlap", "output_ids": ["strimer-atx", "gpu-block"], "detail": "zones overlap 64% — intentional?" }
    ]
  }
}
```

`SpatialLayout` and `Output` (`types/src/spatial.rs:469`,
`spatial.rs:337`) serialize cleanly today; the tool schema is generated
from the same `ToSchema` derivations the OpenAPI surface uses, so the
MCP contract cannot drift from the REST contract.

#### `place_outputs`

The incremental editor — lets an agent move one fan without re-sending
a 40-Output document, and snaps to rig template mounts:

```jsonc
{
  "name": "place_outputs",
  "idempotent": true,
  "input": {
    "layout_id": { "type": "string" },
    "operations": {
      "type": "array",
      "items": {
        "oneOf": [
          {  // add or replace a placement
            "op": "place",
            "output": { /* full Output, or */ },
            "device_id": "usb:lianli-hub-1", "zone_name": "ch2",
            "mount_id": "side-fan-2",        // resolve position/size/rotation from the
            "rig_template_id": "lianli-o11d-evo-rgb",  // template mount (§6.3)
            "component_template_id": "lianli-uni-sl120"  // resolve topology from component
          },
          { "op": "move",   "output_id": "fan-3", "position": {"x": 0.18, "y": 0.42}, "rotation": 1.5708 },
          { "op": "remove", "output_id": "fan-3" }
        ]
      }
    }
  },
  "output": { "layout": { /* full post-state */ }, "warnings": [] }
}
```

The `mount_id` + `component_template_id` pair is the payoff of the
whole spec: *"put a Uni Fan SL120 in the O11D EVO's second side mount"*
becomes one operation with zero coordinate math in the agent's head.

#### `configure_zones`

Declarative reconcile over a scene's render partitions:

```jsonc
{
  "name": "configure_zones",
  "idempotent": true,
  "input": {
    "scene_id": { "type": "string" },
    "zones": [
      {
        "name": "Case",
        "color": "#e135ff",
        "devices": [               // OutputAssignment semantics: ids or inline Outputs
          { "id": "fan-1" }, { "id": "fan-2" }, { "id": "strimer-atx" }
        ]
      },
      { "name": "Desk", "devices": [ { "id": "razer-kb" }, { "id": "monitor-strip" } ] }
    ],
    "prune": { "type": "boolean", "default": false,
               "description": "Remove zones not named here. Default keeps unmentioned zones untouched." }
  },
  "output": { "zones": [ /* ZoneResponse list */ ], "warnings": [] }
}
```

`prune: false` by default keeps the tool additive unless the agent
explicitly asks for exact reconciliation — the safe default for a
shared scene another surface (Studio) may also be editing.

### 5.4 New Resources and Prompt

| Resource                       | Content                                                        |
| ------------------------------ | -------------------------------------------------------------- |
| `hypercolor://layouts`         | All layouts (summaries) + active layout id                     |
| `hypercolor://scenes`          | Scene summaries incl. zone/group structure                     |
| `hypercolor://components`      | Component template catalog (Spec 66 registry)                  |
| `hypercolor://rigs`            | Rig template catalog (§6)                                      |
| `hypercolor://layout-preview`  | PNG render of the active layout (image content, §5.5)          |

One new prompt joins `mood_lighting` / `troubleshoot` /
`setup_automation` in `mcp/prompts.rs`:

- **`setup_rig`** — arguments: `hardware` (free-text description of
  case + devices, optional) and `scope` (`case` | `desk` | `room`,
  optional). Expands to the workflow contract in §7.1.

### 5.5 Layout Preview Rendering

Agents (and the humans they're talking to) need to *see* a proposed
layout. The daemon gains a server-side rasterizer:

- `GET /api/v1/layouts/{id}/render?width=640` → `image/png`
- Draws: canvas bounds, each Output's bounding box (rotated), its LED
  positions as dots (computed via the existing topology generators),
  name labels, and zone tint when the layout is scene-bound.
- Implementation: pure-CPU rasterization with `tiny-skia` (small,
  already in the Servo dependency universe; no GPU, no new threads).
  Target: < 10 ms for a 40-Output layout at 640 px.
- The MCP `preview_layout` tool and the `hypercolor://layout-preview`
  resource return the PNG as base64 MCP image content, so chat clients
  display it inline. This is the agent's "show, then ask" primitive.

## 6. Layer 2 — Rig Templates (Geometry Knowledge)

### 6.1 Schema

New module `hypercolor-types/src/rig.rs`:

```rust
/// Physical scale a rig template describes.
#[serde(rename_all = "snake_case")]
pub enum RigKind { Case, Desk, Room }

/// What a mount physically accepts.
#[serde(rename_all = "snake_case")]
pub enum MountKind {
    Fan120, Fan140, FanSlim120,
    StripChannel,        // addressable strip run (front edge, PSU shroud…)
    Motherboard, Gpu, CpuBlock, PsuShroud, DistroPlate,
    Keyboard, Mouse, Mousepad, MonitorBack, DeskEdge, Speaker,
    Bulb, LightBar, Panel,
    Other,
}

/// One physical location a rig offers.
pub struct RigMount {
    pub id: String,                       // "side-fan-2"
    pub name: String,                     // "Side Intake — Middle"
    pub kind: MountKind,
    /// Center position in normalized [0,1] canvas space.
    pub position: NormalizedPosition,
    /// Footprint in normalized canvas space.
    pub size: NormalizedPosition,
    /// Radians, counter-clockwise; the orientation a component placed
    /// here should inherit.
    #[serde(default)]
    pub rotation: f32,
    /// Component categories that make sense here (reuses Spec 66 vocab).
    #[serde(default)]
    pub suggested_categories: Vec<ComponentCategory>,
    /// Free-text physical hint surfaced to agents and the UI.
    #[serde(default)]
    pub notes: String,
}

/// The geometry of a case, desk, or room as a named set of mounts.
pub struct RigTemplate {
    pub id: String,                       // "lianli-o11d-evo-rgb"
    pub name: String,                     // "Lian Li O11 Dynamic EVO RGB"
    pub kind: RigKind,
    #[serde(default)]
    pub vendor: String,
    /// Recommended canvas aspect ratio (w / h) for this rig viewed
    /// front-on. Layout creation may letterbox to honor it.
    #[serde(default)]
    pub aspect_hint: Option<f32>,
    pub mounts: Vec<RigMount>,
    #[serde(default)]
    pub origin: ComponentOrigin,          // BuiltIn | User (reused from Spec 66)
    /// Provenance: product page, manual, community build the geometry
    /// was derived from. Required for built-in templates.
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: u32,
}
```

Design notes:

- **Normalized coordinates, same as everything else.** Mounts live in
  the same `[0,1]` space as `Output.position`, so mount → placement is
  a copy, not a projection. Millimeter fidelity is deliberately *not*
  the goal at rig scale; relative position is what spatial effects
  consume. (`ComponentTemplate.physical_size_mm` stays the place for
  accessory-scale physical data.)
- **The canvas is the front view.** For `Case` templates, coordinates
  describe the case viewed from its showcase side (the glass). This is
  the convention the layout editor already implies; the spec makes it
  explicit so independently-authored templates compose.
- **`sources` is load-bearing.** Templates are research artifacts. An
  agent that web-derives a template records where the geometry came
  from; review of community contributions starts there.

### 6.2 Storage and Loading

```
data/rigs/                          # built-in, shipped, validated in CI
  lianli-o11d-evo-rgb.toml
  lianli-o11-vision.toml
  hyte-y70.toml
  corsair-5000d.toml
  fractal-north.toml
  generic-atx-mid.toml              # the fallback everyone matches
  desk-single-monitor.toml
  desk-dual-monitor.toml
~/.local/share/hypercolor/rigs/     # user/agent-authored, wins on id conflict
```

A `RigTemplateRegistry` in `hypercolor-core` (sibling to the Spec 66
component registry) loads both trees at startup and on a
`rescan`-style nudge, validates (§9), and serves search. Search reuses
the existing `mcp/fuzzy.rs` matcher so "o11 evo" and "lian li evo rgb"
both resolve.

Seed set for this spec: the eight templates above. The deliberate
inclusion of `generic-atx-mid` and the two desk templates means the
flow degrades gracefully for cases we haven't mapped — the agent
starts from generic mounts and adjusts via the calibration loop.

### 6.3 The Agent Research Loop

The schema turns web research into a durable artifact:

1. Agent calls `search_rig_templates("O11 Dynamic EVO RGB")` → no hit.
2. Agent researches the case (product page, manual) with whatever web
   tools its host provides.
3. Agent drafts a `RigTemplate`, calls `save_rig_template` →
   validation errors/warnings come back structured (mounts outside
   `[0,1]`, overlapping fan mounts, missing sources) → agent fixes.
4. Template persists in the user rigs dir; `place_outputs` can now snap
   to its mounts; the *next* session — or the next user, once RFC 53's
   registry exists — skips steps 2–3.

Example seed template (illustrative geometry):

```toml
# data/rigs/lianli-o11d-evo-rgb.toml
schema_version = 1
id = "lianli-o11d-evo-rgb"
name = "Lian Li O11 Dynamic EVO RGB"
kind = "case"
vendor = "Lian Li"
aspect_hint = 1.1
tags = ["dual-chamber", "atx"]
sources = ["https://lian-li.com/product/o11-dynamic-evo-rgb/"]
version = 1

[[mounts]]
id = "side-fan-1"
name = "Side Intake — Top"
kind = "fan120"
position = { x = 0.82, y = 0.22 }
size = { x = 0.16, y = 0.20 }
suggested_categories = ["fan"]

[[mounts]]
id = "side-fan-2"
name = "Side Intake — Middle"
kind = "fan120"
position = { x = 0.82, y = 0.46 }
size = { x = 0.16, y = 0.20 }
suggested_categories = ["fan"]

# … side-fan-3, bottom-fan-1..3, top-fan-1..3, rear-fan-1,
#   strip channels for the front edge …

[[mounts]]
id = "mobo"
name = "Motherboard"
kind = "motherboard"
position = { x = 0.45, y = 0.40 }
size = { x = 0.34, y = 0.42 }
suggested_categories = ["motherboard"]
```

### 6.4 What This Layer Does NOT Change

The vendor database keeps its single job (device identification).
`ComponentTemplate` keeps its single job (accessory geometry + slot
compatibility). Rig templates compose with both — a mount suggests
categories, a component template fills the mount, the device database
identifies the controller driving it — without any of the three
schemas absorbing the others.

## 7. Layer 3 — Setup Experience

### 7.1 The Canonical Workflow

One markdown document, `docs/design/agent-rig-setup-workflow.md`, is
the single source for both the MCP prompt expansion and the agent
skill. The workflow it encodes:

```
1. DISCOVER    discover_devices (all targets) → enumerate what answered
2. PAIR        for each auth_required device: get pairing fields from
               get_devices(detail), walk the user through pair_device
               (Hue button press, Govee API key, …)
3. INVENTORY   ask the user what the hardware physically is; match
               controllers → component templates; configure_attachments
               for hub-style controllers (what's on each channel)
4. DISAMBIGUATE identify-flash each channel/slot: "which fans just lit
               up purple?" → name and order the physical instances
5. GEOMETRY    search_rig_templates; on miss, research the case and
               save_rig_template (§6.3); pick scope (case/desk/room)
6. PLACE       upsert_layout (skeleton) + place_outputs with
               mount_id/component_template_id snapping
7. VERIFY      preview_layout → show the render, ask; then run a
               directional sweep (set_effect with a left-to-right
               wipe) and ask "did it sweep front-to-back?" — fix
               rotation/winding with place_outputs until yes
8. COMPOSE     apply_layout; configure_scene + configure_zones
               (sensible default: one zone per physical cluster);
               configure_layers with a starter effect per zone
9. SAVE        save_profile; offer create_scene automation (existing
               tool) for schedules
```

Contract clauses baked into the document: never present a placement
guess as fact (flash and confirm); prefer asking over assuming when
channel→fan mapping is ambiguous; always end with a saved profile so
the result survives; degrade to `generic-atx-mid` rather than stalling
when the case is unknown.

### 7.2 Surfaces

- **MCP prompt `setup_rig`** — the workflow document inlined, with the
  user's `hardware` argument prepended. Works in any MCP client with
  prompt support; zero Hypercolor-side session state.
- **Agent skill `rig-setup`** — user-facing skill (distributed alongside
  the existing `hypercolor-control` skill) for CLI-capable agents.
  Same workflow document, plus CLI command equivalents so it works
  against a daemon without MCP enabled.
- **Docs page** — "Set up with AI" under `docs/content/`, showing the
  claude.ai / Claude Desktop / Claude Code connection paths and a real
  transcript.

### 7.3 What the Flow Feels Like

Abbreviated target transcript (the docs page carries the full one):

> **User:** i've got an o11d evo rgb with 9 uni fan sl120s on two hubs,
> a strimer 24-pin, and a blackwidow keyboard
>
> **Agent:** Found both Uni Hubs (USB), the Strimer controller, and the
> BlackWidow. Pairing's clean — nothing needs auth. Your O11D EVO RGB
> template is in the library. Two hubs though — let me figure out which
> is which. *(identify: hub 1, ch1)* Which fans just flashed purple?
>
> **User:** bottom three
>
> **Agent:** Hub 1 is your bottom rack. *(continues per channel, places
> outputs via mounts, renders preview)* Here's your layout — fans on
> side/bottom/top racks, Strimer across the mobo, keyboard on the desk
> band below. Look right? I'll run a sweep so you can sanity-check
> direction…

## 8. REST and CLI Additions

MCP full control is the headline, but the two new capability areas land
on all three surfaces (per the `hypercolor-types::api` shared-domain
rule, request/response types live in `types/src/api/rigs.rs`):

| Surface | Addition                                                                 |
| ------- | ------------------------------------------------------------------------ |
| REST    | `GET /api/v1/rigs`, `GET /api/v1/rigs/{id}`, `POST /api/v1/rigs`, `DELETE /api/v1/rigs/{id}` (user-origin only), `GET /api/v1/layouts/{id}/render` |
| CLI     | `hypercolor rigs list/show/search/import`, `hypercolor layouts render <id> -o preview.png` |
| MCP     | Everything in §5                                                          |

## 9. Validation and Error Design

All authoring inputs flow through one validation layer (shared between
REST and MCP paths — same handlers, so this is free):

**Hard errors** (tool call fails, nothing applied):
- Schema violations (missing required fields, malformed topology)
- Coordinates outside `[0,1]`, non-positive sizes, NaN rotation
- `led_mapping` length ≠ topology `led_count()`
- Unknown `layout_id` / `scene_id` / `mount_id` / `template_id` references
- Zone reconcile that would orphan the primary zone (Spec 64 §6.2)

**Warnings** (applied, surfaced in `warnings[]`):
- Output references a device not currently connected (legit: authoring
  before hardware arrives, or USB re-enumeration mid-session)
- Output overlap above 50% IoU between distinct devices
- Topology LED count ≠ device-reported LED count for the bound zone
- Mount kind / component category mismatch (140mm fan in a 120 mount)
- Rig template without `sources`

Error payloads are structured (`code`, `field_path`, `detail`) — agents
repair from `field_path`, humans read `detail`. This mirrors the
existing `ToolError::InvalidParam { param, reason }` shape
(`mcp/tools/scenes.rs:344`) extended with paths for nested documents.

## 10. Security Posture

Full control over MCP means a network-reachable port can now mutate
configuration, not just toggle effects. Posture:

- **Same trust boundary as REST.** The MCP endpoint mounts inside the
  existing Axum app (`mcp/mod.rs:44`) behind the same bind address and
  the same auth middleware (`api/security.rs`). MCP adds *no* new
  network exposure — anyone who can reach `/mcp` can already reach
  `PUT /api/v1/layouts/{id}`. This spec changes the agent-visible
  surface, not the attack surface.
- **`mcp.mode` config knob** — `full` (default) | `control` | `read`.
  `control` clamps to today's 16 tools; `read` clamps to read-only
  tools and resources. The default is `full` because that is the
  product; the knob exists for shared-network setups, not as a hedge.
  Tool *listing* respects the mode (clamped tools are absent, not
  erroring), so agents never plan against tools they can't call.
- **Annotations as client hints.** Mutating tools are annotated
  truthfully so MCP clients can apply their own confirmation UX;
  `identify` and `discover_devices` are flagged idempotent so clients
  don't over-prompt on the highest-frequency calls.
- **No new persistence of secrets.** Pairing credentials continue to
  flow through the existing pairing store; `pair_device` accepts
  secrets as transient call params and never echoes them back in
  post-state or errors.

## 11. Implementation Plan

Four waves, independently shippable, ordered by leverage:

**W1 — MCP authoring parity** (`hypercolor-daemon`)
- Tools 1–5, 7–11, 14–18 (§5.2) wrapping existing handlers; shared
  validation layer (§9); `get_devices(detail)` + `get_status` config
  echo; new resources except `rigs`; tool schema generation from
  `ToSchema`.
- Exit: an MCP client with no REST access completes discover → pair →
  place → zone → layers → profile on a live daemon.

**W2 — Rig templates** (`hypercolor-types`, `hypercolor-core`, data)
- `rig.rs` types + `types::api::rigs`; registry + loader + fuzzy
  search; REST routes; MCP tools 6, 12, 13 + `hypercolor://rigs`;
  eight seed templates; CI validation of `data/rigs/`.
- Exit: `place_outputs` with `mount_id` snapping works end-to-end;
  `save_rig_template` round-trips an agent-authored template.

**W3 — Experience**
- Workflow document; `setup_rig` prompt; `rig-setup` skill; preview
  renderer (§5.5) + `preview_layout` + image resource; docs page with
  transcript.
- Exit: the §7.3 transcript is reproducible against real hardware
  (the dogfood rig is conveniently a Lian Li + Razer household).

**W4 — Full-control completion & polish**
- `manage_library`, `set_config`; component template data pass for
  popular SKUs (Uni Fan SL/AL ring defs, Strimer matrices already
  exist per Spec 66 — audit coverage); CLI `rigs` subcommands;
  `hypercolor://layout-preview` cache headers; telemetry counters for
  tool usage so we learn which verbs agents actually lean on.

## 12. Testing Strategy

Per workspace convention: tests in `tests/`, named `{feature}_tests.rs`.

- **Tool contract tests** (`hypercolor-daemon/tests/mcp_authoring_tests.rs`)
  — every new tool: schema validity (input/output schemas parse as JSON
  Schema), happy path against a seeded `AppState`, idempotence
  (double-call equals single call for every `idempotent: true` tool),
  warning and hard-error paths from §9.
- **Reconcile semantics** — `configure_zones` with `prune` on/off;
  `place_outputs` op sequences (place → move → remove → place) land in
  the same state as the equivalent single `upsert_layout`.
- **Rig template validation** (`hypercolor-core/tests/rig_template_tests.rs`)
  — schema round-trip, mount bounds, conflict resolution (user dir
  shadows built-in), fuzzy search hits for vendor aliases.
- **Seed data CI gate** — `just compat`-style check: every TOML in
  `data/rigs/` parses, validates, and carries `sources`.
- **Preview renderer** — golden-image tests at fixed layouts (tolerant
  comparison), latency assertion (< 10 ms at 640 px, 40 Outputs).
- **Mode clamping** — tool listing under `mcp.mode = control/read`
  excludes exactly the right sets.
- **End-to-end script** — a test MCP client (rmcp client side) drives
  the full §7.1 sequence against a daemon with mock devices; asserts a
  profile exists at the end whose layout matches the placed outputs.

## 13. Open Questions

1. **Mount occupancy.** Should `place_outputs` track which mounts are
   filled and warn on double-booking? Leaning yes-as-warning (it's
   legitimate to stack a strip behind a fan).
2. **Desk/room scale and `spaces`.** `SpatialLayout.spaces` +
   `RoomDimensions` exist but are unused. Room-kind rig templates can
   target a flat canvas now and adopt spaces when that subsystem wakes
   up — confirm we're happy deferring.
3. **Template versioning across sync.** When RFC 53's registry lands,
   id collisions between local user templates and community templates
   need a precedence rule. Proposal: local always wins, registry pulls
   are explicit.
4. **`create_scene` naming debt.** With `configure_scene` landing, the
   automation-flavored `create_scene` name gets confusing. Rename is a
   breaking MCP change; alias + deprecation note in the description is
   probably enough for agents.
