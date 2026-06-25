+++
title = "Multi-zone walkthrough"
description = "Build a second zone, split devices, run different effects per zone, switch between zones, and set the unassigned-lights policy."
weight = 90
+++

By the end of this walkthrough your rig runs two effects at once: one effect on
the keyboard and case, a different effect on the desk strips, switched together
when you change scenes and tunable independently while they play. You will build
a second zone, move device outputs into it, give each zone its own effect, hop
between zones, and decide what happens to any output you leave behind.

![Hypercolor Studio with the zone tree, the live Stage canvas, and the composition panel](/img/ui/studio.webp)

This is the end-to-end version of the focused pages. If you want the model
first, read [Zones](@/studio/zones.md), [Device grouping](@/studio/device-grouping.md),
and [Layers](@/studio/layers.md), then come back here to put it together.

{% callout(type="info") %}
A **scene** is the whole-rig configuration. A **zone** is a flexible partition
of that scene's canvas. One scene holds many zones, each driving a disjoint set
of device outputs. Switching scenes swaps every zone together; this walkthrough
lives entirely inside one scene.
{% end %}

## Before you start

Open Studio in the web UI and confirm two things:

- More than one LED device (or one device with multiple addressable channels)
  is connected, so there is something to split. See [Finding devices](@/guide/finding-devices.md)
  if your rig is empty.
- A scene is active. A fresh install gives you a default scene with a single
  **Default zone** that already owns every device. That single zone is your
  starting point.

The left column is the **zone tree**. The center is the **Stage**. For a Light
the Stage is the always-on spatial layout editor with the live effect rendering
under the device boxes. There is no Preview/Layout toggle to hunt for.

## 1. Create a second zone

In the zone tree, use **New zone** under the Zones section. It opens an inline
name field; type a name you will recognize later ("Desk strips", "Ambient") and
press Enter to create the zone. Give it an identity color afterward from the
zone's controls in the tree (see [Zones](@/studio/zones.md)). That color is the
zone's swatch throughout Studio, including the assignment panel you are about to
use.

The new zone starts empty: no device outputs, no layers. Your original **Default
zone** still owns everything. Creating a zone never moves devices on its own;
that is the next step, and it is deliberate.

{% callout(type="tip") %}
The moment a scene has more than one LED zone, Studio switches on the multi-zone
affordances: the zone-assignment strip docks under the Stage canvas, and a
synthetic **Unassigned** entry appears in the tree. In a single-zone scene
neither shows, because there is nothing to partition.
{% end %}

## 2. Split devices across the two zones

Every device output starts in the Default zone. You move outputs out of it and
into your new zone. The unit of assignment is an **output** (one device output
or addressable segment), never the whole physical device. A multi-channel
controller can have one channel in one zone and another channel elsewhere.

You have two ways to move outputs.

### Add a whole device to a zone

On the target zone, use the **Add device** control. It opens a picker of every
device that is not already entirely in this zone. Each entry shows where the
device currently lives, for example `Desk Strip (in Default zone)`, so you can
see what the move pulls from.

Picking a device brings every output it has into this zone. A device already
placed in another zone is **moved** (its outputs are reassigned out of their old
owner). A device the scene has never placed is **minted** fresh, one output per
channel. Either way the daemon resets each output's canvas placement on assign,
so you re-place it in the target zone's layout editor afterward.

### Move individual outputs

For finer control, use the **Zone assignment** strip docked below the Stage
canvas. It lists every output grouped by its owning zone, and within a zone by
physical device. Click output chips to multi-select them across zones, then pick
a destination from the **Assign to** dropdown in the strip's toolbar. The
toolbar shows your selection count and clears it after a successful move.

This is the path for partially assigning a multi-channel device: select just the
channels you want, send them to the new zone, and leave the rest where they are.

{% callout(type="warning") %}
Each device output belongs to exactly one zone at a time. Assigning an output to
a new zone always removes it from its previous owner. There is no "copy an output
into two zones" — exclusivity is the invariant that keeps each zone's output
correct.
{% end %}

## 3. Re-place outputs on each zone's canvas

Each zone owns its **own** spatial canvas. Moving an output between zones keeps
its topology and LED mapping but resets its position to the target zone's
default, because a position is only meaningful against a specific canvas.

Select a zone in the tree to switch the Stage to that zone's layout. Drag,
resize, and rotate the device boxes to match the physical arrangement, then use
**Save** in the Stage header. **Revert** discards unsaved edits; both light up
only when the canvas is dirty. Undo and redo are `Ctrl+Z` and `Ctrl+Shift+Z`.
The full editor is covered in [Layouts](@/studio/layouts.md).

Positions are normalized to `[0.0, 1.0]`, so a layout stays correct no matter
what resolution the canvas runs at. The canvas defaults to 640×480 and is
configurable.

## 4. Give each zone its own effect

Select a zone, then open its composition by clicking the **now-playing chip** in
the Stage header. That slide-over hosts the layer stack for the selected zone.
Add an effect layer, browse the catalog, and pick what you want. Each zone gets
an independent effect, controls, and brightness.

Repeat for the second zone. Now the keyboard and case can run one effect while
the desk strips run another, both live at once. The catalog and per-layer
controls work the same as on any single-zone scene; see
[Effects and controls](@/studio/effects-and-controls.md) and [Layers](@/studio/layers.md)
for blend modes, opacity, and live tuning.

{% callout(type="info") %}
Every zone shares the daemon's global audio, screen-capture, and sensor inputs.
An audio-reactive effect in one zone and a static effect in another both read
the same audio; the static one simply ignores it. Per-zone input routing (a
zone capturing its own monitor) is future work.
{% end %}

## 5. Switch between zones

Selecting a zone in the tree is how you move between them. The selected LED zone
is also Studio's app-wide **apply target**, so a quick-apply from the dashboard,
the sidebar, or the command palette lands in the zone you are composing rather
than spraying every device.

Per-zone now-playing rows in the sidebar let you pause and resume a single zone
without touching the others. The sidebar shows up to three of them and folds the
rest behind a "more zones" link back into Studio. See
[Now playing and transport](@/studio/now-playing-transport.md) for the transport
surfaces.

Switching the active **scene** is different: it swaps all zones together. Your
two-zone arrangement is one configuration inside one scene. [Scenes](@/studio/scenes.md)
covers creating, renaming, and switching whole-rig configurations.

## 6. Set the unassigned-lights policy

If you split devices and leave an output in no zone, it becomes **unassigned**.
That happens deliberately (you moved everything out of the Default zone) or as a
side effect (you deleted a zone, and its outputs fell out). The scene decides
what those outputs do.

In a multi-zone scene the tree shows a synthetic **Unassigned** entry. It is not
a surface — no layer stack, no canvas — so selecting it shows the scene-level
policy instead. Pick one of:

- **Turn off** — unassigned outputs are cleared to black. This is the default.
- **Hold last colors** — unassigned outputs keep whatever they last rendered.
- **Follow &lt;zone&gt;** — unassigned outputs are routed through a named zone,
  so they mirror that zone's effect.

The policy is editable only when the daemon advertises the
`scene-unassigned-behavior-write` capability; otherwise the current setting
shows read-only. The Unassigned Stage also points you back to the zone-assignment
strip so you can pull those outputs into a real zone at any time.

{% callout(type="tip") %}
A partially assigned multi-channel device is handled per output. Its assigned
channels render from their zone; its unassigned channels follow the
unassigned-lights policy. You never have to assign a whole device just to satisfy
one channel.
{% end %}

## What you built

One scene, two zones, two effects, a defined fate for everything in between:

{% mermaid() %}
graph TD
    SCENE[Active scene] --> Z1[Default zone<br/>keyboard + case]
    SCENE --> Z2[Desk strips zone]
    SCENE --> UN[Unassigned outputs]
    Z1 --> E1[Effect A]
    Z2 --> E2[Effect B]
    UN --> POL[Unassigned-lights policy:<br/>off / hold / follow a zone]
{% end %}

Both zones render concurrently and switch together when the scene changes.
Per-zone control edits change one zone without disturbing the others. To make
the same split your everyday look, save it as part of a scene and switch to it
whenever you want it back.

## Where to go next

- [Zones](@/studio/zones.md) — the full zone lifecycle: rename, color, enable,
  make-default, delete.
- [Device grouping](@/studio/device-grouping.md) — the device card, channels,
  hide, identify, and remove.
- [Scenes](@/studio/scenes.md) — saving and switching whole-rig configurations.
- [Zone API and concurrency](@/studio/zone-api-and-concurrency.md) — the REST
  routes and the `groups_revision` optimistic-concurrency contract behind every
  move you just made.
