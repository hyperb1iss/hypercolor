+++
title = "Studio"
description = "The creative workspace: compose your whole rig with the Scene, Zone, Layers, and Layout model, one zone tab at a time."
weight = 20
sort_by = "weight"
template = "section.html"
+++

Studio is where you compose your whole rig. It is the web UI's creative workspace at `/studio`: pick a scene, split your hardware into zones, give each zone its own canvas and its own stack of effects and media, and watch every change render live.

{{< img path="img/ui/studio.webp" alt="Studio: zone tabs across the header, the device pane on the left, and the live Stage canvas in the center" />}}

## The model 🔮

Four nouns compose everything in Studio. Learn these once and the whole workspace clicks into place.

- **Scene**: a whole-rig configuration. Exactly one scene is active at a time, and it owns everything below it. Switching scenes reconfigures your entire setup at once.
- **Zone**: a partition of a scene. Each zone claims its own device outputs and carries its own layers and its own layout. A fresh scene has one zone, **Main**; a multi-zone scene splits your hardware across several.
- **Layers**: a zone's inputs, stacked. An effect under a gif, a screen capture, a solid color: each is a layer with its own blend mode, opacity, and transform.
- **Layout**: a zone's own spatial canvas. Each output the zone claims sits somewhere on it, and Studio samples the composited pixels onto your LEDs.

```text
Scene  ── one active config. Owns everything.
 └─ Zone  ── a partition of the scene; one tab in Studio
     ├─ Layers  ── effects, media, screen capture, color
     └─ Layout  ── the zone's spatial canvas
         └─ Output ── one device output, placed on the canvas
```

Scenes are whole-rig configs; zones are how you carve that rig into parts. For the precise vocabulary, including the internal Rust type names, see [Vocabulary and naming](@/studio/vocabulary-and-naming.md).

## The workspace

Studio shows one zone at a time. Each LED zone is a tab in the page header, followed by a **Screens** tab when the scene drives display screens, then a **+** that makes a new zone. Below the tabs sit two columns about the open tab:

- The **device pane** on the left lists the devices in the open zone, the scene's other zones, and the hardware no zone claims yet.
- The **Stage** in the center is the open zone's live canvas. The effect renders under the device boxes, and you drag, resize, and rotate them in place.

The **composition panel** for the zone's layers slides in over the Stage when you click the now-playing chip.

Every edit commits as you make it. There is no Save button: device moves, canvas edits, and layer changes land in the active scene immediately, and **Undo** (`Ctrl+Z`) takes back device moves, canvas edits, and zone changes one step at a time.

Studio shares one app-wide active scene, so a change made from the dashboard, another client, or the CLI lands here without a refresh. The tab you have open is also the app's target zone: apply an effect from the Effects page, a favorite, or the command palette and it lands in that zone.

## Start here

New to Studio? Walk these pages in order.

- [Overview](@/studio/overview.md): the Scene to Zone to Layers-plus-Layout model in full, and how it composes.
- [Workspace tour](@/studio/workspace-tour.md): a guided pass over the scene picker, the zone tabs, the device pane, the Stage, and the composition panel.
- [Scenes](@/studio/scenes.md): switch, create, duplicate, rename, and delete scenes with the scene picker; the Default scene.
- [Zones](@/studio/zones.md): add, rename, color, pause, reorder, and delete zones from the tab strip; the Main zone.

## Compose your rig

- [Device assignment](@/studio/device-assignment.md): the device pane, moving devices and single channels between zones by drag or **Move to**, and the Unassigned section.
- [Layers](@/studio/layers.md): build the layer stack: blend modes, opacity, transform and color, reorder, and per-layer health.
- [Layouts](@/studio/layouts.md): the zone canvas: placement from the rig, Fit, drag, resize, and rotate, live commit, and undo.
- [Effects and controls](@/studio/effects-and-controls.md): apply effects to a zone and drive the live control panel inside a layer.
- [Multi-zone walkthrough](@/studio/multi-zone-walkthrough.md): build a multi-zone scene step by step, from a new scene to a different look in every zone.
- [Now playing and transport](@/studio/now-playing-transport.md): the now-playing chip, per-zone sidebar rows, the preview's zone tabs, and pause and resume.

## Under the hood

For contributors working on Studio itself:

- [Architecture](@/studio/architecture.md): `StudioContext`, the one Studio history, live layout commit, and the reused `LayerPanel` and `LayoutWorkspace` contracts.
- [Zone API and concurrency](@/studio/zone-api-and-concurrency.md): the zone, scene, layer, and layout routes, the `If-Match` revision preconditions, and the per-zone WebSocket preview protocol.
- [Vocabulary and naming](@/studio/vocabulary-and-naming.md): the locked vocabulary, the kill list, and the plain-words rendering rules.

## Related sections

Studio applies effects but does not build them. To author your own, head to the [Effects](@/effects/_index.md) section. For the underlying REST and WebSocket contracts, see the [API reference](@/api/_index.md). If a zone is not rendering as expected, [Studio troubleshooting](@/troubleshooting/studio.md) has the common fixes.
