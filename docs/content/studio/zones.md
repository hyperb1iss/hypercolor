+++
title = "Zones"
description = "Zones are partitions of a scene, one tab each: add, rename, color, pause, reorder, and delete them, plus the Main zone and the Unassigned section."
weight = 40
+++

A **zone** is a partition of a scene. Each zone claims its own device outputs and owns its own layer stack and its own canvas, so it lights its slice of the rig independently. One scene can hold a single zone covering everything, or several zones that each drive a different slice of your hardware with a different look.

Every LED zone is a tab in the Studio header, in the scene's zone order. The tab you open is the zone the device pane, the Stage, and the composition panel describe. This page covers the zone lifecycle (add, rename, recolor, pause, reorder, delete), the permanent **Main** zone, and the **Unassigned** section for hardware no zone has claimed.

{{< img path="img/ui/studio.webp" alt="The Studio zone tab strip with the Main tab open and a + after the tabs" />}}

{% <callout type="info"> %}
A scene is a whole-rig config; a zone is a part of that scene. Switching scenes swaps your entire setup, while zones split one scene into independently lit parts.
{% </callout> %}

## Single-zone and multi-zone scenes

Every scene starts with one zone, **Main**, which covers your LED hardware. That is a complete, useful setup on its own: pick an effect, and it plays across everything.

A single-zone scene keeps zones out of the way. Studio shows one **Main** tab with a **+** after it, and no other page shows a zone selector at all. The **Unassigned** section appears in the device pane only when some hardware is actually unclaimed.

A scene becomes **multi-zone** the moment it holds more than one LED zone. Then:

- The tab strip shows a tab per zone, and the device pane lists the other zones under the open one.
- The **Unassigned** section stays in the pane even when empty, as a place to move devices out to.
- The Dashboard and Effects previews grow compact zone tabs, the sidebar's Now Playing panel shows a row per zone, and the command palette names the zone an apply lands in.

This progressive split keeps the simple case simple. You meet the multi-zone surfaces once you have more than one zone to compose.

## The Main zone

The Main zone is the scene's **primary** zone, and it is permanent. You can rename it, recolor it, pause it, reorder it, and move devices in and out of it, but you cannot delete it: its **Delete zone** menu item is disabled. Every scene keeps one Main zone so there is always a home for your lights.

New scenes name it **Main**. A primary zone with a blank name, or with a name older versions seeded ("Primary" or "Default zone"), shows as **Main** too. Rename it and your name takes over immediately.

{% <callout type="tip"> %}
In a single-zone scene, the Main zone's canvas is your rig's layout: moving a device there updates the active named layout that every single-zone scene reads. Once a scene has a second LED zone, every zone's canvas belongs to that scene alone. See [Layouts](@/studio/layouts.md#where-a-canvas-edit-lands).
{% </callout> %}

## Adding a zone

Click **+** after the tabs. Studio creates a zone named **New zone** (or **New zone 2** and up when that name is taken), opens its tab, and selects the name so you can type over it. Type a name and press **Enter**. Press **Escape** to keep the default name.

```text
● Main   ● Desk strips   ┊  ▢ Screens 2   +
                                          └─ click: a "New zone" tab opens, name selected
```

A new zone starts empty: no devices, no layers. Its canvas reads **Drag devices here, or add them from Unassigned**, and the device pane expands every other zone's row so the devices you might bring in are in view. Bring hardware in from the [device pane](@/studio/device-assignment.md); the first devices you add are placed from your rig and fitted to the canvas.

Two other routes make a zone and fill it in one move:

- A device card's **Move to** menu ends with **New zone**. It creates a zone named after the device, moves the device in, and opens the tab with the name ready to type over.
- Dragging a device from the pane onto **+** does the same thing.

The **+** button and the **New zone** item appear only once the daemon reports it can create zones, so on a freshly started daemon they may take a moment to show up.

## Zone settings: rename, color, pause, reorder

Every zone's settings live on its tab. Open the zone menu with the **⋯** button on the open tab, by right-clicking any tab, or with **Shift+F10** on a focused tab. The menu offers **Rename**, **Color**, **Pause zone** or **Resume zone**, **Move left**, **Move right**, and **Delete zone**. The Main zone has the same menu, so a single-zone scene can still rename or recolor its one zone.

**Rename.** Choose **Rename**, double-click the tab, or press **F2** on a focused tab, and the tab becomes a text field. Press **Enter** or click away to commit, **Escape** to cancel. A blank or unchanged name is ignored rather than saved.

**Color.** **Color** opens a row of eight identity swatches (Purple, Cyan, Coral, Green, Yellow, Blue, Orange, and Red), a custom color picker, and **No color**. The color tints the zone's dot on its tab, in the device pane, in the preview's zone tabs, and on its Now Playing row in the sidebar. It is an identity cue for the zone, not a lighting color, so it never changes what your LEDs display.

**Pause and resume.** **Pause zone** stops the zone driving its devices while it keeps its layers, canvas, and color, so **Resume zone** brings it back exactly as it was. A paused zone's tab shows a pause glyph, and its row under **Other zones** is dimmed. The per-zone pause buttons in the sidebar's Now Playing panel flip the same switch.

**Reorder.** **Move left** and **Move right** step the zone one tab over; dragging a tab along the strip drops it anywhere. Tab order is the scene's zone order, which the preview's zone tabs and the sidebar rows follow too.

{{< img path="img/ui/ui-studio-zones.webp" alt="The zone menu open on a Studio tab: Rename, Color, Pause zone, Move left, Move right, Delete zone" />}}

**Per-zone brightness.** A zone carries its own `brightness` multiplier, clamped to `0.0`-`1.0`, on top of global output brightness. Studio has no control for it today; set it over the API with `PATCH /api/v1/scene/zones/{zone}` and a `brightness` field. See [zone API and concurrency](@/studio/zone-api-and-concurrency.md) for the route and its revision guard.

## Deleting a zone

Choose **Delete zone** from the zone menu. The Main zone and a scene's last LED zone cannot be deleted; for those the item is disabled and carries the reason, such as "The main zone can't be deleted".

An empty zone deletes immediately. A zone that holds devices asks once first: the dialog names the zone and how many devices become unassigned, and offers **Keep zone** and **Delete zone**. **Keep zone** has focus, and **Escape** or a click outside the dialog also keeps the zone.

Either way, a bar at the bottom of the Stage then reads **Deleted** and the zone's name, with an **Undo** button, for eight seconds. Undo rebuilds the zone: its name, color, paused state, its position among the tabs, and the devices it held, back where they sat on its canvas. A device you moved somewhere else in the meantime stays where you put it. When the deleted zone was open, the tab to its left (or, for the first tab, its right) opens in its place.

{% <callout type="warning"> %}
Deleting a zone releases its devices to Unassigned and throws away its layer stack. Undo puts the zone and its devices back, but not its layers, so add them again after an undo. The physical devices are never touched.
{% </callout> %}

## The Unassigned section

The **Unassigned** section at the bottom of the device pane collects every device that no zone has claimed. It is not a zone: it has no tab, no layer stack, and no canvas. It answers one question: what should hardware do when it belongs to no zone?

### Unassigned policy

The section header carries the scene's policy for unclaimed outputs as a compact select:

| Option | What it does |
| --- | --- |
| **Off** | Unclaimed outputs go dark. |
| **Hold last colors** | They keep whatever they were last showing. |
| **Follow** a zone | They mirror the zone you choose, so they share that zone's look. |

The select lists one **Follow** option per LED zone in the scene, Main included.

The policy is editable only when the daemon advertises the `scene-unassigned-behavior-write` capability. When it does not, the header shows the current policy in plain words as a read-only label.

{{< img path="img/ui/studio.webp" alt="The Unassigned section of the Studio device pane, with its policy select set to Off" />}}

The cleanest fix for unclaimed hardware is usually to assign it. Each Unassigned card has a **+** that adds the device to the open tab's zone, and you can also drag a card onto a canvas, a tab, or a zone row. See [device assignment](@/studio/device-assignment.md) for every route.

## How zone edits stay safe under concurrent changes

Studio shares one active scene across every client, the CLI, and the MCP server. Two people, or you and an agent, can be editing the same scene at once. To keep edits from clobbering each other, every zone change carries the scene's current revision as a precondition.

If the scene changed somewhere else between when you loaded it and when your edit went out, your edit does not silently overwrite the newer state. Studio reloads the scene, shows a "Scene changed elsewhere. Reloaded, try again." toast, and lets you reapply your change against the fresh state. This applies to every zone action on this page: add, rename, color, pause, reorder, delete, and the Unassigned policy.

The full revision model, REST routes, and stale-retry semantics live in [zone API and concurrency](@/studio/zone-api-and-concurrency.md).

## Where to go next

- [Device assignment](@/studio/device-assignment.md) brings hardware into your zones and splits outputs across them.
- [Layouts](@/studio/layouts.md) arranges each zone's devices on its own canvas.
- [Layers](@/studio/layers.md) stacks effects, media, and faces on a zone.
- [Multi-zone walkthrough](@/studio/multi-zone-walkthrough.md) builds a multi-zone scene step by step.
