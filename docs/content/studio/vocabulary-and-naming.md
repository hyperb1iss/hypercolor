+++
title = "Vocabulary & naming"
description = "The locked Studio vocabulary, the kill list (never 'rooms' or 'All Lights'), the plain-words rendering rules, and how the wire-safe Rust rename maps onto it."
weight = 130
+++

The Studio vocabulary is locked. A **scene** is a whole-rig config, a **zone** is a flexible partition of that scene's canvas, and the default zone is always called **"Default zone."** Smart-home language — "rooms," "All Lights," "Lights" — never appears in user-facing strings, and a layer's content kind always renders in plain words ("Effect," "Media," "Color"), never as a raw enum name or a UUID. This page is the canonical reference for that vocabulary, the words that are banned, and the small set of rendering rules in `crates/hypercolor-ui/src/zones/surface.rs` that enforce them.

The internal Rust type names that back these concepts are documented in [Studio architecture](@/studio/architecture.md). They differ from the user-facing words by design, and the difference is wire-safe; the relevant subtlety is summarized at the end of this page.

{% callout(type="warning") %}
This is not a style preference. The model and its naming were locked by the project owner in design doc 55 to keep one term per concept across the UI, the API, the TUI, the CLI, and the docs. A new label for an existing concept is a regression, not a synonym.
{% end %}

## The locked model 💜

Five nouns, in a strict containment hierarchy. Everything in Studio is one of these or a property of one of these.

```text
Scene  ── top-level object. Exactly one is active. Owns everything.
 └─ Zone  ── a part of a scene. The switchable unit.
     ├─ Layer   ── the zone's inputs: an effect, a face, media, screen capture, a web page, a color
     └─ Layout  ── the zone's OWN spatial canvas; device outputs placed on it
         └─ Output ── one device output / segment, positioned on the canvas

Device ── physical hardware.
 └─ Channel  ── an addressable run on the device.
     └─ Component ── what is wired to a channel: a strip, an infinity fan, an LED area.
```

Load-bearing consequences of the hierarchy:

- **A scene owns everything.** Pick a scene and you have its zones, their layers, and their device placement. Nothing meaningful exists outside a scene. See [Scenes](@/studio/scenes.md).
- **Each zone owns its own layout.** A zone's layout is its own spatial canvas; switching zones switches the canvas. See [Layouts](@/studio/layouts.md).
- **A device output lives in exactly one zone's layout at a time.** Adding it to another zone removes it from the first. See [Device grouping](@/studio/device-grouping.md).
- **The default zone is just a zone.** A fresh scene has one, holding every device, named "Default zone." The user can rename it. There is no "All Lights." See [Zones](@/studio/zones.md).
- **There is no standalone "layouts library."** A saved arrangement is part of a scene. What the user picks is a scene.

## The vocabulary table

The single source of truth for the words. The user-facing term is what the UI, docs, CLI help, and any prose must say. The plain-words rule means the UI never leaks the internal name.

| Concept | Always say | Never say |
| --- | --- | --- |
| Top-level container, one active | **Scene** | "preset" (for the whole rig), "room" |
| A device partition with layers and a layout | **Zone** | "room," "group," "area" in prose |
| The default zone of a fresh scene | **Default zone** | "All Lights," "Lights," "Primary," "Main" |
| A zone's input | **Layer** | "channel" (channel means something else) |
| A layer's content kind | **Source** (rendered as Effect / Media / Screen capture / Web page / Color) | the enum name, a UUID |
| A zone's spatial canvas | **Layout** | "room," "scene" (a scene is not a layout) |
| One device output placed on a layout | **Output** | "device zone," "slot" in user prose |
| Physical hardware | **Device** | — |
| An addressable run on a device | **Channel** | "zone" (a channel is not a zone) |
| What is wired to a channel | **Component** | "attachment" in user-facing text |

The left column tracks design doc 55 §3, which is the locked source. When a concept here gains a new affordance, the word does not change; only design doc 55 may revise this table.

## The kill list

These strings are banned from user-facing surfaces. They are smart-home vocabulary for a professional tool, and several of them are flatly wrong under the locked model. If you find one in the UI, the docs, or CLI output, it is a bug.

- **"Room" / "rooms."** Hypercolor has no concept of a room. The partition is a zone, the spatial surface is a layout. ("Room" was the old `StageLayoutBar` label for the dead layouts-library picker; the picker is gone.)
- **"All Lights" / "Lights."** The default zone is "Default zone." "All Lights" implied a magic everything-bucket that does not exist in the model; every device lives in exactly one zone.
- **"Zones & Devices."** The old left-column header. The left column is the zone tree; it does not need a generic catch-all label.
- **"Primary" (user-facing).** `ZoneRole::Primary` is the internal role of the default zone. Fresh scenes seed the zone's stored name as `"Default zone"` already, but older persisted scenes may still carry the legacy `"Primary"` seed; either way the UI substitutes **"Default zone"** before showing a blank-or-legacy name (see the rendering rules below). "Primary" must never reach the screen.
- Any other smart-home framing: "scenes for the living room," "turn the lights on," "the lighting group." Scenes are whole-rig configs and zones are canvas partitions. Keep the home-automation register out.

{% callout(type="danger") %}
"Scene" and "zone" are not interchangeable and not smart-home words here. A scene is the entire rig's configuration with exactly one active at a time. A zone is one partition of that scene's canvas. Writing "scene" where you mean "zone" (or vice versa) breaks the model for the reader.
{% end %}

## Plain-words rendering rules

Three small functions in `crates/hypercolor-ui/src/zones/surface.rs` are where the vocabulary is enforced in code. They are kept Leptos-free so they can be `#[path]`-tested directly, and they are the canonical place to look when a label looks wrong.

### Default-zone naming

A fresh scene's default zone is seeded with the name `"Default zone"` already, but `surface_name` also defends against an unnamed zone or a legacy scene that still carries the old `"Primary"` seed, substituting the friendly label so the internal role string never leaks:

```rust
fn surface_name(group: &Zone, kind: SurfaceKind) -> String {
    if kind != SurfaceKind::Light || group.role != ZoneRole::Primary {
        return group.name.clone();
    }
    if is_blank_default_name(&group.name) {
        "Default zone".to_owned()
    } else {
        group.name.clone()
    }
}

fn is_blank_default_name(name: &str) -> bool {
    let trimmed = name.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case("primary")
}
```

A `Primary`-role LED zone whose name is blank or the legacy `"Primary"` string renders as **"Default zone."** Fresh scenes already store `"Default zone"`, so this rule mostly guards persisted scenes and freshly cleared names. The moment the user types a real name, that name wins. Any other zone shows its stored name verbatim.

### Layer-source labels

A layer's content kind is rendered in plain words by `layer_source_kind`, never as the `LayerSource` enum variant and never as a raw asset UUID:

```rust
fn layer_source_kind(source: &LayerSource) -> &'static str {
    match source {
        LayerSource::Effect { .. } => "Effect",
        LayerSource::Media { .. } => "Media",
        LayerSource::ScreenRegion { .. } => "Screen capture",
        LayerSource::WebViewport { .. } => "Web page",
        LayerSource::ColorFill { .. } => "Color",
    }
}
```

This is the fallback. A layer's top-line caption prefers the user's own name for the layer (`top_layer_label`); only when the layer is unnamed does the source kind stand in. So a tile reads "Aurora" when the user named it that, and "Effect" when they did not — never `Effect { id: "…" }` and never the UUID. See [Layers](@/studio/layers.md) for the full layer model.

### The Unassigned entry is not a zone

`UNASSIGNED_SURFACE_ID = "__unassigned__"` is a synthetic rail entry, deliberately not a UUID so it can never collide with a real `ZoneId`. It is not a surface: it has no layer stack and no Stage editor. It only appears in a genuinely multi-zone scene, and it surfaces the scene's unassigned-lights policy rather than acting as a catch-all "All Lights" bucket. Treat it as a status row, not a zone, in both prose and screenshots.

## The internal rename is wire-safe

The headline domain types in `crates/hypercolor-types/src/scene.rs` already carry the locked names: `Zone`, `ZoneRole`, and (in `spatial.rs`) `Output` and `OutputComponent`. The user-facing word and the Rust type now agree for these. The full type map, including the satellite renames and the in-flight crates, lives in [Studio architecture](@/studio/architecture.md); do not duplicate it here.

The one subtlety worth flagging when you write docs: the rename is a **Rust-identifier rename, not a wire change.** Serialized field names and enum-variant strings are frozen so persisted scenes and the REST and WebSocket contracts never shift. The clearest live example is the scene-change event, whose `event_type` string is still `"render_group_changed"` on the wire even though the Rust enum that classifies it is `ZoneChangeKind`:

```rust
// crates/hypercolor-ui/src/ws/messages.rs — the wire literal is frozen
let is_render_group_changed = event_type == "render_group_changed";
```

So when you document the API, use the **wire** names (`groups`, `render_group_changed`, `ZoneRole`'s `primary` / `custom` / `display` variant strings), not the renamed Rust identifiers. When you write user-facing or conceptual prose, use the **vocabulary table** words. The two never contradict each other as long as you pick the register that matches the surface you are documenting. For the binary frame and event details, see [Zone API and concurrency](@/studio/zone-api-and-concurrency.md) and [the WebSocket reference](@/api/websocket.md).

## Quick reference

- Scene = whole-rig config. Zone = a partition of that scene's canvas. Default zone = "Default zone," never "Primary," never "All Lights."
- Layer source renders in plain words: Effect, Media, Screen capture, Web page, Color.
- Never "rooms," never smart-home framing. Scenes and zones are not home-automation nouns.
- User-facing prose follows the vocabulary table; API docs follow the frozen wire names; Rust type names are in [architecture](@/studio/architecture.md).
- Enforcement lives in `zones/surface.rs`: `surface_name`, `is_blank_default_name`, `layer_source_kind`.
