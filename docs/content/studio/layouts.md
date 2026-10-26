+++
title = "Layouts"
description = "Each zone's own canvas: where arriving devices land, Fit, drag, resize, and rotate, live commit with undo, and why no layout means no light."
weight = 70
template = "page.html"
+++

A layout tells the engine where each light sits on a zone's canvas. The zone's layers paint a 2D canvas, and the spatial sampler reads one pixel position per LED to decide its color. Place an output on the canvas and it picks up whatever the effect paints there. An output with no place on the canvas stays dark, because nothing samples it.

That is the most common "my effect runs but nothing lights up" cause. If a zone glows in the live preview yet your hardware is black, the fix is almost always here: bring the device into the zone so its outputs land on the canvas, then check where they sit.

{% <callout type="info" title="Where the editor lives"> %}
In Studio, the layout editor **is** the Stage. Open an LED zone's tab and its canvas fills the center, with the live effect rendering under the device boxes. There is no Preview/Layout toggle and no Save button: every canvas edit commits when you finish it.
{% </callout> %}

{{< img path="img/ui/studio.webp" alt="Studio with the open zone's canvas filling the Stage and the Undo, Redo, and Fit actions in its header" />}}

## The model in one picture

Every zone has its own canvas. Its layout places the outputs the zone claims on that canvas, and nothing else appears there.

{% <mermaid> %}
graph TD
    Scene["Scene (one active)"] --> Zone["Zone (one tab)"]
    Zone --> Layout["Layout: the zone's own canvas"]
    Layout --> O1["Output (device segment)"]
    Layout --> O2["Output (device segment)"]
    Layout --> O3["Output (device segment)"]
{% </mermaid> %}

An **output** is one addressable run on a device: a fan, a strip segment, a keyboard. The canvas uses normalized coordinates from `0.0` to `1.0`, so a layout is resolution-independent: an output centered at `x = 0.5` stays centered whatever the canvas pixel size. Canvases default to 640×480 and are tunable daemon-wide via `daemon.canvas_width` and `daemon.canvas_height`.

An output lives in exactly one zone at a time. Moving it into another zone takes it off the first zone's canvas.

## Where arriving devices land

Your **rig** is the active named layout: the record of where your hardware physically sits. Each zone's canvas is independent, but devices arrive on it from the rig:

- **A zone's first devices** fit the canvas together, keeping their physical arrangement, so the zone's effect covers them completely.
- **Later arrivals** keep their physical relation to what is already there. They are carried onto the canvas with the same scale and offset as the zone's current devices, so a fan that sits above a strip in your case lands above it here.
- **A device dropped on the canvas** is centered on the drop point.
- **A device the rig does not place** still joins the fit when it is among a zone's first devices. Arriving later, it takes the device's own built-in footprint when it has one (a Push 2 does), and otherwise the daemon's default placement. One moved from another zone keeps the spot it had.

Nothing lands off the canvas. Arrivals that would overflow it shrink and slide until they fit.

### Fit

**Fit** in the Stage header scales and centers every output in the zone so the group fills the canvas inside a small margin, keeping the outputs' arrangement. It frames the boxes the canvas shows; hidden outputs move along with the group. Fit is a single edit, so one **Undo** takes it back.

Fit is the fix for a zone whose devices sit small in a corner while the effect paints the whole canvas: one click and the effect covers them.

## Where a canvas edit lands

Which layout an edit changes depends on how many LED zones the scene has:

- **In a single-zone scene**, the Main zone's canvas is your rig. Moving a device there updates the active named layout, which every single-zone scene reads and which seeds the placement of devices joining zones later.
- **In a multi-zone scene**, every zone's canvas belongs to that zone in that scene. Moving a fan in the Keyboard zone changes the Keyboard zone of this scene only; other zones and other scenes keep their own placements.

{% <callout type="tip"> %}
Set your rig up once in a single-zone scene, with every device at its physical position. Every zone you build afterward places its devices from that layout.
{% </callout> %}

## Moving, resizing, rotating

In Studio, every output the open zone claims already appears on its canvas. Bring devices into the zone from the device pane (see [Device assignment](@/studio/device-assignment.md)), and they show up as boxes you can move.

{% <callout type="warning" title="Generic ARGB channels need a component first"> %}
An unattached generic ARGB controller channel does not draw on the canvas until you attach a component (a strip, a fan, an LED area) to it. The channel is raw wiring until then. Fixed devices like keyboards and AIO coolers always render, because they have meaningful LEDs without any component setup.
{% </callout> %}

Every box on the canvas is a direct-manipulation handle.

- **Drag** anywhere on a box to move it. Selected outputs move together.
- **Resize** by grabbing one of the four corner handles that appear on a selected box. Toggle the aspect-ratio link in the properties panel to keep width and height proportional while you drag.
- **Rotate** with the rotation slider in the properties panel below the canvas, or type an exact degree value.
- **Scale** uniformly with the scale slider, a quick way to grow or shrink without touching width and height directly.

Ring and arc shapes always resize as perfect circles regardless of the canvas aspect ratio, so a fan ring never squashes into an ellipse while you drag.

The drag and resize path is built for a smooth 60 Hz feel: while you drag, positions paint straight to the canvas and the LED preview updates live, and the change commits once when you release the pointer.

Drag a box off the canvas and onto another zone's tab to move that output to the other zone.

### Busy canvases

Real rigs overlap. A press on the canvas picks the **smallest** box under the pointer, so a small device under a long strip stays clickable without any reordering. **Alt**-click steps down through the boxes stacked beneath it.

Labels declutter the same way. A box's label is its name and LED count, or its resolution for a screen. A box shows its label when the label fits inside it and nothing above covers it. Otherwise the label waits for hover or selection, and a box too small to hold it shows it in a callout beside the box.

## The properties panel

Select a single output and the panel below the canvas opens its properties. With nothing selected it reads **Click an output on the canvas to edit its properties**.

The panel heads with the output's name and its channel (**Ch**) as the zone holds them, read-only. Below them sit the controls a canvas edit saves:

| Control | What it does |
| --- | --- |
| Pos X / Y | Exact pixel position of the output's center, plus center-horizontally and center-vertically buttons. |
| Size W / H | Exact pixel size, with an aspect-ratio link toggle. |
| Rot | Rotation in degrees, slider plus numeric entry. |
| Scale | Uniform scale from 0.5× to 3×. |

The zone layout stores geometry only, so the panel offers no per-output stacking or brightness controls. The panel also carries quick actions: **identify** (flash the physical output so you can find it), **reset to defaults** (recenter, unrotate, restore default size), **Move to** (send the selected outputs to another zone or to Unassigned), and **remove** (take the output out of this zone). All pixel values convert against the canvas dimensions, so they read in real pixels even though the layout stores normalized coordinates.

## Selecting and compound selection

Click an output to select it. Shift-click adds or removes outputs from a multi-selection without starting a drag. Click empty canvas to clear the selection.

Compound selection lets you treat a whole device, or a wired component, as one unit:

- **Double-click** descends one level: from the whole canvas into a device, then into a component slot.
- **Escape** ascends one level back out.
- A breadcrumb in the corner shows your current depth.

When more than one output is selected, the properties panel switches to group controls for aligning, distributing, packing, and mirroring the set, plus its position, rotation, and scale.

## Live commit and undo

Canvas edits commit as you finish them: releasing a drag or a resize, releasing a slider, pressing Enter in a property field or leaving it, or clicking **Fit**. Typing in a field commits nothing until then. There is no Save and no Revert, and switching tabs or leaving Studio never loses an edit.

**Undo** and **Redo** in the Stage header step through one history that holds canvas edits, device moves, and zone changes in the order you made them. The shortcuts are `Ctrl+Z` to undo and `Ctrl+Shift+Z` or `Ctrl+Y` to redo. They are ignored while you type in a text field or while a drag is in flight. Undoing an edit made in another zone opens that zone's tab first, so you see what changed. The history belongs to the current Studio visit and resets when you switch scenes or leave Studio.

If someone changes the same scene from another client or the CLI while you edit, Studio writes only the fields you changed over the zone as it stands, so their edit to another output or another field survives. When the write still cannot land, a toast says the change was not saved, the edit stays on your canvas, and your next edit retries it.

## Why an effect can run with dark LEDs

To make the dark-LED failure concrete, here is the full chain from effect to photons:

{% <mermaid> %}
graph LR
    FX["Effect paints canvas"] --> SAMP["Sampler reads one pixel per LED"]
    SAMP --> OUT["Output's canvas position"]
    OUT --> HW["LEDs light up"]
{% </mermaid> %}

If the output is not on the canvas, the sampler has no position to read for it, so it sends nothing, so the LED stays dark. The effect renders perfectly the whole time; the break is the missing placement. Bring the device into the zone, check that its boxes sit under the effect, and the chain completes.

## Where to go next

- [Zones](@/studio/zones.md): how zones partition a scene and why each one owns a canvas.
- [Device assignment](@/studio/device-assignment.md): bringing device outputs into a zone so they appear on its canvas.
- [Layers](@/studio/layers.md): stacking the effects, faces, and media that paint the canvas you are mapping onto.
- [Multi-zone walkthrough](@/studio/multi-zone-walkthrough.md): building a multi-zone scene step by step.
