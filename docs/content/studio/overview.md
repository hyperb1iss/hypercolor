+++
title = "Studio overview"
description = "What Studio is: the scene-to-zone-to-layers-plus-layout model, one tab per zone, and a workspace that commits every edit live."
weight = 10
+++

Studio is where you compose what your lights actually do. Open it from the sidebar and you land on one zone of the active scene: its tab is selected in the header, its devices fill the pane on the left, and its canvas renders live on the Stage. Pick a zone, see it render, and build it up from there. Everything you arrange in Studio belongs to one **scene**, the whole-rig configuration that drives every connected device.

{{< img path="img/ui/studio.webp" alt="The Studio workspace: zone tabs in the header, the device pane on the left, and the open zone's live canvas on the Stage" />}}

The mental model is four words deep, and it is worth holding onto before you touch anything:

- A **scene** is the top-level object. Exactly one is active at a time, and it owns everything below it.
- A scene contains one or more **zones**. A zone is a partition of your rig with its own canvas and its own looks, and Studio gives each one a tab.
- Each zone carries a stack of **layers** (its inputs: an effect, a face, media, a screen capture, a color) and its own **layout** (the spatial canvas where the zone's device outputs sit).
- A **device** is the physical hardware. Each of its outputs belongs to at most one zone at a time.

That hierarchy is the entire model. The rest of Studio is the surface that lets you edit it.

## Scenes are whole-rig configs

A scene is the complete picture of what your lighting is doing right now. When you switch scenes, the whole rig changes at once. There is always exactly one active scene, and Studio shares it with the rest of the app. Change a zone from the dashboard, the command palette, or the CLI, and Studio reflects it immediately because all of them read the same active scene.

This is deliberately not smart-home language. A scene is not a preset for one corner of your desk. It is the full configuration of everything you own, captured as one switchable unit. The scene picker in the page header switches, creates, renames, and deletes scenes; see [scenes](@/studio/scenes.md) for the full lifecycle.

## Zones partition the scene

Inside a scene, zones let you drive disjoint parts of your rig independently. One zone can run a screen-mirror effect on the keyboard while another runs ambient plasma on the case fans and a third pulses desk strips to music. Each zone is a name, a set of claimed device outputs, a canvas, and a layer stack.

Every scene starts with one zone, **Main**, which holds your LED hardware until you split things out. A single-zone scene stays quiet: one **Main** tab and a **+** after it, and no zone controls anywhere else in the app. Add a second zone and the per-zone surfaces appear, reusing the controls you already know.

The unit of assignment is a device **output**, not a whole physical device. A multi-channel controller can have one channel in one zone and another channel somewhere else. An output lives in exactly one zone at a time, so moving it into another zone takes it out of the first.

{% <callout type="info"> %}
Creating zones and moving devices between them light up only when the running daemon advertises the matching capabilities. On a daemon without them you see just the Main zone, which is the complete, working baseline rather than a stripped-down mode.
{% </callout> %}

For the zone lifecycle (add, rename, color, pause, reorder, delete), see [zones](@/studio/zones.md). For moving devices between zones, see [device assignment](@/studio/device-assignment.md).

## Layers and layout are what a zone holds

Opening a zone's tab loads two things: its layer stack and its layout.

The **layer stack** is the zone's set of inputs. You stack an effect, drop a gif on top, blend a screen capture underneath, and the compositor renders the result. Layers are authored bottom to top, with blend modes, opacity, and per-layer transform and color controls. The stack opens as a slide-over panel over the Stage when you want it, so it never crowds the canvas while you are just watching. See [layers](@/studio/layers.md) for the stack editor.

The **layout** is the zone's own spatial canvas, where each of the zone's outputs is placed, sized, and rotated. For an LED zone the Stage *is* the layout editor: the live effect renders right under the device boxes, so you see exactly how the composition maps onto your hardware. A device that joins a zone lands where it physically sits in your rig, and **Fit** scales the zone's devices to fill the canvas. Layouts use normalized `[0.0, 1.0]` coordinates, so they stay resolution-independent. See [layouts](@/studio/layouts.md) for the canvas editor.

LCDs and other device displays are **Screens**, not zones. They share one Screens tab, where the Stage shows the selected screen's live face instead of a spatial layout, because a single screen has nothing to place.

## The workspace

The workspace is a header, two columns, and an on-demand panel.

```text
┌ Studio ──────────────────────────────────────── Scene  Studio Session ▾ ┐
│ ● Main   ● Keyboard   ● Desk strips  ┊  ▢ Screens 2   +                 │
├─────────────────────────┬───────────────────────────────────────────────┤
│ ⌕ Search devices      / │ ◉ Bubble Garden ›                ↶  ↷  ⤢ Fit  │
│                         │                                               │
│ MAIN · 6                │      the Main zone's canvas,                  │
│  ▣ Nollie 32        ⋯   │      the Main zone's devices,                 │
│  ▣ Corsair K70      ⋯   │      nothing else                             │
│                         │                                               │
│ OTHER ZONES             ├───────────────────────────────────────────────┤
│  ▸ ● Keyboard       1   │ selected output properties                    │
│  ▸ ● Desk strips    2   │                                               │
│                         │                                               │
│ UNASSIGNED · 1   Off ▾  │                                               │
│  ▣ WLED Theater   ⚡ +  │                                               │
└─────────────────────────┴───────────────────────────────────────────────┘
```

{{< img path="img/ui/studio.webp" alt="The Hypercolor Studio workspace with one zone tab open" />}}

The header carries the scene picker on its title row and the zone tabs on the row below. The tab you select is the zone everything else describes.

The left column is the **device pane**: a search field, the open zone's devices, one collapsed row per other zone, and the **Unassigned** section with the scene's policy for hardware no zone claims. Its width is draggable and persists per browser. On narrow viewports it collapses into a slide-over drawer behind a **Devices** button.

The center column is the **Stage**. For an LED zone it is the live canvas with the effect under the device boxes, a properties panel for the selected output below it, and **Undo**, **Redo**, and **Fit** in its header. The now-playing chip on the Stage opens the **composition panel**, a slide-over that hosts the layer editor over the Stage instead of occupying a permanent rail.

Every edit commits when you make it. Moving a device, releasing a drag on the canvas, and changing a layer all write to the active scene right away, and one undo history covers device moves and canvas edits. There is nothing to save before you switch tabs or leave the page.

The tab you have open is also the app-wide target zone. Apply an effect from the Effects page, a favorite, or the command palette after working in Studio, and it lands in the zone you were composing. The Screens tab is not a target, so opening it leaves the target on the last LED zone you had open.

## Where to go next

- Take the guided [workspace tour](@/studio/workspace-tour.md) of every part of the live UI.
- Learn [scenes](@/studio/scenes.md) and [zones](@/studio/zones.md), the two halves of the model.
- Build a composition with [layers](@/studio/layers.md) and arrange it with [layouts](@/studio/layouts.md).
- Build a full multi-zone scene step by step in the [multi-zone walkthrough](@/studio/multi-zone-walkthrough.md).
- For the architecture behind it all, see [Studio architecture](@/studio/architecture.md).
