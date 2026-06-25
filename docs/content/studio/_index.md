+++
title = "Studio"
description = "The creative workspace: compose your whole rig with the Scene, Zone, Layers, and Layout model."
weight = 20
sort_by = "weight"
template = "section.html"
+++

Studio is where you compose your whole rig. It is the web UI's creative workspace at `/studio`: pick a scene, partition your hardware into zones, arrange each zone's devices on its own spatial canvas, stack layers of effects and media, and watch every change render live.

![Studio: the zone tree on the left, the live Stage in the center](/img/ui/studio.webp)

## The model 🔮

Four nouns compose everything in Studio. Learn these once and the whole workspace clicks into place.

- **Scene** — a whole-rig configuration. Exactly one scene is active at a time, and it owns everything below it. Switching scenes reconfigures your entire setup at once.
- **Zone** — a flexible partition of a scene. Each zone is its own switchable unit with its own devices, its own layers, and its own layout. A single-zone scene has just the **Default zone**; a multi-zone scene splits devices across several.
- **Layers** — a zone's inputs, stacked. An effect under a gif, a screen capture, a solid color: each is a layer with its own blend mode, opacity, and transform.
- **Layout** — a zone's own spatial canvas. You place each device output on it, and Studio samples the composited pixels onto your LEDs.

```text
Scene  ── one active config. Owns everything.
 └─ Zone  ── a switchable partition of the scene
     ├─ Layers   ── effects, media, screen capture, color
     └─ Layout   ── the zone's spatial canvas
         └─ Output ── one device output, placed on the canvas
```

Scenes are whole-rig configs; zones are how you carve that rig into parts. For the precise vocabulary, including the internal Rust type names, see [Vocabulary and naming](@/studio/vocabulary-and-naming.md).

## The workspace

Studio is a two-column workspace. The **zone tree** on the left lists your zones and screens; selecting one drives the center **Stage**. For a lighting zone the Stage *is* the live spatial layout editor, with the effect rendering under the device boxes in real time. The **composition panel** for editing layers slides in over the Stage on demand rather than taking a permanent rail.

Studio shares one app-wide active scene, so a zone change you make from the dashboard, another client, or the CLI lands here without a refresh. The zone you have selected in Studio is also the app-wide effect apply-target: a quick-apply from anywhere drops into the zone you are composing.

{% callout(type="info") %}
Studio is the default workspace in the shipped build. If a browser turned it off in Settings, the `/studio` route falls back to the legacy `/assets` page. The standalone `/layout` page still exists as a soak-gated legacy surface; treat the Studio Stage as the canonical place to edit a zone's layout.
{% end %}

## Start here

New to Studio? Walk these pages in order.

- [Overview](@/studio/overview.md) — the Scene to Zone to Layers-plus-Layout model in full, and how it composes.
- [Workspace tour](@/studio/workspace-tour.md) — a guided pass over the page header, zone tree, Stage, and the composition slide-over.
- [Scenes](@/studio/scenes.md) — switch, create, rename, and delete scenes; the ephemeral default scene; the apply-target relationship.
- [Zones](@/studio/zones.md) — create, color, enable, make-default, and delete zones; single- versus multi-zone behavior; the Unassigned entry.

## Compose your rig

- [Device grouping](@/studio/device-grouping.md) — add and move devices between zones, read the device card, and assign at output granularity.
- [Layers](@/studio/layers.md) — build the layer stack: blend modes, opacity, transform and color, reorder, and per-layer health.
- [Layouts](@/studio/layouts.md) — the spatial canvas: drag, resize, and rotate outputs, with compound selection, undo, and live preview.
- [Effects and controls](@/studio/effects-and-controls.md) — apply effects to a zone and drive the live control panel inside a layer.
- [Multi-zone walkthrough](@/studio/multi-zone-walkthrough.md) — end to end: build a second zone, split devices, and run different effects side by side.
- [Now playing and transport](@/studio/now-playing-transport.md) — the now-playing chip, per-zone sidebar rows, and pause/resume.

## Under the hood

For contributors working on Studio itself:

- [Architecture](@/studio/architecture.md) — `StudioContext`, the shared-versus-local state map, and the reused `LayerPanel` and `LayoutWorkspace` contracts.
- [Zone API and concurrency](@/studio/zone-api-and-concurrency.md) — the zone, scene, layer, and layout routes, the `If-Match` revision preconditions, and the per-zone WebSocket preview protocol.
- [Vocabulary and naming](@/studio/vocabulary-and-naming.md) — the locked vocabulary, the kill list, and the plain-words rendering rules.

## Related sections

Studio applies effects but does not build them. To author your own, head to the [Effects](@/effects/_index.md) section. For the underlying REST and WebSocket contracts, see the [API reference](@/api/_index.md). If a zone is not rendering as expected, [Studio troubleshooting](@/troubleshooting/studio.md) has the common fixes.
