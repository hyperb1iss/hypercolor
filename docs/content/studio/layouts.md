+++
title = "Layouts"
description = "Map each zone's devices onto the canvas: drag, resize, and rotate device outputs, then save. No layout means no light."
weight = 70
template = "page.html"
+++

A layout tells the engine where each light sits on the canvas. The effect paints a 2D canvas, and the spatial sampler reads one pixel position per LED to decide its color. Place an output on the canvas and it picks up whatever the effect paints there. Leave the canvas empty and the effect still renders, but every LED stays dark because nothing is sampling it.

This is the single most common "my effect runs but nothing lights up" cause. If a zone glows in the live preview yet your hardware is black, the fix is almost always here: drag the device's outputs onto the canvas and hit Save.

{% callout(type="info", title="Where the editor lives") %}
In Studio, the layout editor **is** the Stage for a Light. Select a Light zone in the left tree and the canvas fills the center, with the live effect rendering under the device boxes. There is no Preview/Layout toggle. The standalone `/layout` page edits a separate, legacy layouts library and is being retired. Treat the Studio Stage as the canonical place to build a zone's layout.
{% end %}

![Studio with the spatial layout editor filling the Stage](/img/ui/studio.webp)

## The model in one picture

A scene has one shared canvas. Each zone partitions it and owns a layout that places its outputs on that canvas.

{% mermaid() %}
graph TD
    Scene["Scene (one active)"] --> Zone["Zone (a canvas partition)"]
    Zone --> Layout["Layout — the zone's output placements"]
    Layout --> O1["Output (device segment)"]
    Layout --> O2["Output (device segment)"]
    Layout --> O3["Output (device segment)"]
{% end %}

An **output** is one addressable run on a device: a fan, a strip segment, a keyboard. The canvas uses normalized coordinates from `0.0` to `1.0`, so a layout is resolution-independent: an output centered at `x = 0.5` stays centered whatever the canvas pixel size. The scene's canvas defaults to 640×480 and is tunable daemon-wide via `daemon.canvas_width` and `daemon.canvas_height`.

A device output lives in exactly one zone's layout at a time. Adding it to another zone moves it.

## Placing outputs on the canvas

In Studio, every output assigned to the selected Light zone already appears on its canvas. Use the device-grouping controls (covered in [Device grouping](@/studio/device-grouping.md)) to add a device's outputs to the zone, and they show up as draggable boxes.

On the standalone `/layout` page, a device palette runs down the left side and you drag devices onto the canvas. The Studio Stage hides that permanent palette to keep the canvas as the hero, but the same editor drives both.

{% callout(type="warning", title="Generic ARGB channels need a component first") %}
An unattached generic ARGB controller channel does not draw on the canvas until you attach a component (a strip, a fan, an LED area) to it. The channel is just raw wiring until then. Fixed devices like keyboards and AIO coolers always render, because they have meaningful LEDs without any component setup.
{% end %}

## Moving, resizing, rotating

Every box on the canvas is a direct-manipulation handle.

- **Drag** anywhere on a box to move it. Selected outputs move together.
- **Resize** by grabbing one of the four corner handles that appear on a selected box. Toggle the aspect-ratio link in the properties panel to keep width and height proportional while you drag.
- **Rotate** with the rotation slider in the properties panel below the canvas, or type an exact degree value.
- **Scale** uniformly with the scale slider — a quick way to grow or shrink without touching width and height directly.

Ring and arc shapes always resize as perfect circles regardless of the canvas aspect ratio, so a fan ring never squashes into an ellipse while you drag.

The drag and resize path is built for a smooth 60 Hz feel: while you drag, positions paint straight to the canvas and the LED preview updates live, and the change is committed once when you release the pointer. You never see a stutter mid-drag even on a dense canvas.

## The properties panel

Select a single output and the panel below the canvas opens its full property set.

| Control | What it does |
| --- | --- |
| Name + Ch | Rename the output; set its channel label. A reset arrow restores the default name. |
| Pos X / Y | Exact pixel position of the output's center, plus center-horizontally and center-vertically buttons. |
| Size W / H | Exact pixel size, with an aspect-ratio link toggle. |
| Rot | Rotation in degrees, slider plus numeric entry. |
| Scale | Uniform scale from 0.5× to 3×. |
| Layer | Bring to front, send to back, or nudge one step in stacking order. |
| Brightness | Per-output brightness; a multi-select shows "Mixed" until you collapse them to one value. |

The panel also carries quick actions: **identify** (flash the physical output so you can find it), **reset to defaults** (recenter, unrotate, restore default size), and **remove** (pull the output off this layout). All pixel values convert against the canvas dimensions, so they read in real pixels even though the layout stores normalized coordinates.

## Selecting and compound selection

Click an output to select it. Shift-click adds or removes outputs from a multi-selection without starting a drag. Click empty canvas to clear the selection.

Compound selection lets you treat a whole device, or a wired component, as one unit:

- **Double-click** descends one level: from the whole canvas into a device, then into a component slot.
- **Escape** ascends one level back out.
- A breadcrumb in the corner shows your current depth.

When more than one output is selected, the properties panel switches to group controls for aligning, distributing, and transforming the whole set together.

## Undo, save, and revert

The Stage header carries the canvas controls.

- **Undo / Redo** with the toolbar buttons or `Ctrl+Z` / `Ctrl+Shift+Z` (and `Ctrl+Y` for redo). The shortcuts are suppressed while you are typing in a text field.
- **Save** writes the layout to the zone. The Save button doubles as the dirty indicator: it glows green when you have unsaved changes and dims when the layout is clean.
- **Revert** discards every change since the last save and restores the canvas to its saved state.

{% callout(type="warning", title="Edits are not live until you save") %}
Dragging an output pushes a live preview to the daemon so you can see the result on your hardware immediately, but that preview is temporary. The placement is not persisted to the zone until you hit Save. If you switch zones or close Studio with the Save button still glowing, your arrangement is lost. Revert is the safety net while you experiment; Save is the commit.
{% end %}

If someone changes the same scene from another client or the CLI while you are editing, a save can come back stale. Studio reloads the scene and asks you to try again rather than clobbering the other change. Your in-flight edits to placement survive an unrelated refetch, so a device assigned elsewhere does not wipe the box you are dragging.

## Why an effect can run with dark LEDs

To make the dark-LED failure concrete, here is the full chain from effect to photons:

{% mermaid() %}
graph LR
    FX["Effect paints canvas"] --> SAMP["Sampler reads one pixel per LED"]
    SAMP --> OUT["Output's canvas position"]
    OUT --> HW["LEDs light up"]
{% end %}

If the output is not on the canvas, the sampler has no position to read for it, so it sends nothing, so the LED stays dark. The effect is rendering perfectly the whole time. The break is the missing placement. Drop the output on the canvas, Save, and the chain completes.

## Where to go next

- [Zones](@/studio/zones.md) — how a zone partitions the canvas and why each one owns a layout.
- [Device grouping](@/studio/device-grouping.md) — adding device outputs to a zone so they appear on its canvas.
- [Layers](@/studio/layers.md) — stacking the effects, faces, and media that paint the canvas you are mapping onto.
- [Multi-zone walkthrough](@/studio/multi-zone-walkthrough.md) — building a second zone and splitting outputs across zones end to end.
