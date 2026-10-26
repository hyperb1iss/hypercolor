+++
title = "Vocabulary & naming"
description = "The locked Studio vocabulary, the kill list (never 'rooms', 'All Lights', or 'Available'), the plain-words rendering rules, and how each wire surface spells the concept."
weight = 130
+++

The Studio vocabulary is locked. A **scene** is a whole-rig config, a **zone** is a partition of that scene, and the primary zone of a fresh scene is called **"Main."** Smart-home language ("rooms," "All Lights," "Lights") never appears in user-facing strings, and a layer's content kind always renders in plain words ("Effect," "Media," "Color"), never as a raw enum name or a UUID. This page is the canonical reference for that vocabulary, the words that are banned, and the small set of rendering rules in `crates/hypercolor-ui/src/zones/surface.rs` that enforce them.

The internal Rust type names that back these concepts are documented in [Studio architecture](@/studio/architecture.md). They differ from the user-facing words by design; which wire surface spells the concept which way is summarized at the end of this page.

{% <callout type="warning"> %}
This is not a style preference. The model and its naming were locked by the project owner in design doc 55, and revised in design doc 73, to keep one term per concept across the UI, the API, the TUI, the CLI, and the docs. A new label for an existing concept is a regression, not a synonym.
{% </callout> %}

## The locked model 💜

Five nouns, in a strict containment hierarchy. Everything in Studio is one of these or a property of one of these.

```text
Scene  ── top-level object. Exactly one is active. Owns everything.
 └─ Zone  ── a part of a scene. One tab in Studio.
     ├─ Layer   ── the zone's inputs: an effect, a face, media, screen capture, a web page, a color
     └─ Layout  ── the zone's OWN spatial canvas; device outputs placed on it
         └─ Output ── one device output / segment, positioned on the canvas

Device ── physical hardware.
 └─ Channel  ── an addressable run on the device.
     └─ Component ── what is wired to a channel: a strip, an infinity fan, an LED area.
```

Consequences of the hierarchy:

- **A scene owns everything.** Pick a scene and you have its zones, their layers, and their device placement. Nothing meaningful exists outside a scene. See [Scenes](@/studio/scenes.md).
- **Each zone owns its own layout.** A zone's layout is its own spatial canvas; switching tabs switches the canvas. See [Layouts](@/studio/layouts.md).
- **Zones claim outputs; they do not own devices.** A device output lives in exactly one zone at a time, and one device can split its outputs across zones. Moving an output to another zone removes it from the first. See [Device assignment](@/studio/device-assignment.md).
- **The Main zone is just a zone.** A fresh scene has one, holding your hardware, named "Main." The user can rename it. There is no "All Lights." See [Zones](@/studio/zones.md).
- **There is no standalone "layouts library" in Studio.** A zone's arrangement is part of its scene. What the user picks is a scene.

## The vocabulary table

The single source of truth for the words. The user-facing term is what the UI, docs, CLI help, and any prose must say. The plain-words rule means the UI never leaks the internal name.

| Concept | Always say | Never say |
| --- | --- | --- |
| Top-level container, one active | **Scene** | "preset" (for the whole rig), "room," "world" |
| A partition with its own outputs, layers, and layout | **Zone** | "room," "group," "area" in prose |
| The primary zone of a fresh scene | **Main** | "All Lights," "Lights," "Primary," "Default zone" |
| Hardware no zone claims | **Unassigned** | "Available," "free," "unused" |
| A device's display, composed on its own | **Screen** (the **Screens** tab) | "zone," "display zone" in user prose |
| A zone's input | **Layer** | "channel" (channel means something else) |
| A layer's content kind | **Source** (rendered as Effect / Media / Screen capture / Web page / Color) | the enum name, a UUID |
| A zone's spatial canvas | **Layout** | "room," "scene" (a scene is not a layout) |
| One device output placed on a layout | **Output** | "device zone," "slot" in user prose |
| Physical hardware | **Device** | none |
| An addressable run on a device | **Channel** (or **segment**) | "zone" (a channel is not a zone) |
| What is wired to a channel | **Component** | "attachment" in user-facing text |

The left column tracks design doc 55 §3, the locked source, with the revisions from design doc 73: a fresh scene's primary zone is "Main," hardware in no zone is always "Unassigned," and LCD surfaces are "Screens." When a concept gains a new affordance, the word does not change; only a design doc may revise this table.

{% <callout type="info" title="A word collision to manage"> %}
OpenRGB, iCUE, and Razer use "zone" for a hardware region below the device, and the OpenRGB bridge brings that word into Hypercolor's data. Studio never surfaces a vendor's raw "zone": device sub-units are always outputs, channels, or segments, and "zone" means only a scene's partition.
{% </callout> %}

## The kill list

These strings are banned from user-facing surfaces. They are smart-home vocabulary for a professional tool, or second words for a concept that already has one, and several of them are flatly wrong under the locked model. If you find one in the UI, the docs, or CLI output, it is a bug.

- **"Room" / "rooms."** Hypercolor has no concept of a room. The partition is a zone, the spatial surface is a layout. ("Room" was the old `StageLayoutBar` label for the dead layouts-library picker; the picker is gone.)
- **"All Lights" / "Lights."** The primary zone is "Main." "All Lights" implied a magic everything-bucket that does not exist in the model; every output lives in at most one zone.
- **"Available."** Hardware in no zone is "Unassigned," in a single-zone scene and a multi-zone scene alike.
- **"Zones & Devices."** The old left-column header. The left column is the device pane, and its sections name themselves: the open zone, Other zones, Unassigned.
- **"Primary" or "Default zone" (user-facing).** `ZoneRole::Primary` is the internal role of the Main zone. Fresh scenes store the name `"Main"` already, but older persisted scenes may still carry `"Primary"` or `"Default zone"`; either way the UI substitutes **"Main"** before showing it (see the rendering rules below). Neither string reaches the screen as a zone name.
- Any other smart-home framing: "scenes for the living room," "turn the lights on," "the lighting group." Scenes are whole-rig configs and zones are partitions. Keep the home-automation register out.

{% <callout type="danger"> %}
"Scene" and "zone" are not interchangeable and not smart-home words here. A scene is the entire rig's configuration with exactly one active at a time. A zone is one partition of that scene. Writing "scene" where you mean "zone" (or the reverse) breaks the model for the reader.
{% </callout> %}

## Plain-words rendering rules

Two small functions in `crates/hypercolor-ui/src/zones/surface.rs` are where the vocabulary is enforced in code. They are Leptos-free and covered from `crates/hypercolor-ui/tests/`, and they are the canonical place to look when a label looks wrong.

### Zone names

Every place the UI shows a zone's name goes through one rule, `zone_display_name`:

```rust
pub fn zone_display_name(name: &str, role: ZoneRole) -> String {
    let trimmed = name.trim();
    let unnamed = trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("primary")
        || trimmed.eq_ignore_ascii_case("default zone");
    if role == ZoneRole::Primary && unnamed {
        "Main".to_owned()
    } else {
        trimmed.to_owned()
    }
}
```

A primary zone whose stored name is blank, `"Primary"`, or `"Default zone"` (in any case) renders as **"Main."** Fresh scenes already store `"Main"`, so this rule guards persisted scenes and freshly cleared names. The moment the user types a real name, that name wins. Any other zone shows its stored name, trimmed.

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

This is the fallback. A layer's caption prefers the user's own name for the layer, then the name of the effect it runs; only when neither is known does the source kind stand in. So a tile reads "Aurora" when the user named it that, the effect's name when it runs a known effect, and "Effect" otherwise: never `Effect { id: "…" }` and never the UUID. See [Layers](@/studio/layers.md) for the full layer model.

### Unassigned is a section, not a zone

Hardware in no zone shows in the device pane's **Unassigned** section. It has no tab, no layer stack, and no canvas; it carries the scene's policy for unclaimed outputs (off, hold last colors, or follow a zone). Treat it as a place devices wait, not a zone, in both prose and screenshots.

## Where the wire agrees with the vocabulary

The headline domain types in `crates/hypercolor-types/src/scene.rs` already carry the locked names: `Zone`, `ZoneRole`, and (in `spatial.rs`) `Output` and `OutputComponent`. The user-facing word and the Rust type agree for these. The full type map lives in [Studio architecture](@/studio/architecture.md); do not duplicate it here.

The REST surface agrees too. `GET /scene` returns the full live tree, and every fine-grained path stays under `/scene/zones/{zone}`. Zone membership routes use `members`, and each member identifies a device segment assignment. A layer control patch uses the real layer id embedded in the scene document. The one wire concurrency token is the scene document's `revision`.

The WebSocket surface agrees as well: zone-change events are named `zone_changed` and carry `scene_id`, `zone_id`, `role`, and `kind`.

Persisted scene files may still carry legacy compatibility field names. Those names are private storage details. The daemon projects them into the canonical `zones` tree and single `revision` token on the wire.

So when you document a surface, use `zones`, `members`, `layers`, and `revision` on the wire. When you write user-facing or conceptual prose, use the **vocabulary table** words. For the binary frame and event details, see [Zone API and concurrency](@/studio/zone-api-and-concurrency.md) and the [WebSocket reference](@/api/websocket.md).

## Quick reference

- Scene = whole-rig config. Zone = a partition of that scene, one tab in Studio. Primary zone = "Main," never "Primary," "Default zone," or "All Lights."
- Hardware in no zone = "Unassigned," never "Available." LCD surfaces = "Screens."
- Layer source renders in plain words: Effect, Media, Screen capture, Web page, Color.
- Never "rooms," never smart-home framing. Scenes and zones are not home-automation nouns.
- User-facing prose follows the vocabulary table; API docs follow each surface's own wire names; Rust type names are in [architecture](@/studio/architecture.md).
- Enforcement lives in `zones/surface.rs`: `zone_display_name` and `layer_source_kind`.
