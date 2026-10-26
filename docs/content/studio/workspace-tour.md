+++
title = "Workspace tour"
description = "Guided tour of every Studio surface: the scene picker, the zone tab strip, the device pane, the Stage, the composition slide-over, and the narrow-viewport drawer."
weight = 20
template = "page.html"
+++

Studio shows one zone at a time. The page header carries the scene picker and a tab for every zone; below it, the device pane sits on the left and the Stage fills the center, and the composition panel slides in over the Stage when you summon it. There is no Save button anywhere: every edit commits as you make it. This page walks every surface in order, so you know what each control does before you start composing.

{{< img path="img/ui/studio.webp" alt="The Studio workspace: the scene picker and zone tabs in the header, the device pane on the left, the live Stage in the center" />}}

If you want the conceptual model first (what a scene, zone, layer, and layout are), read the [Studio overview](@/studio/overview.md). This page is the literal UI tour.

## The header

The page header has two rows. The title row carries **Studio** and, at its trailing edge, the scene picker. The second row is the zone tab strip.

### The scene picker

The scene picker names the active scene: a **Scene** label, the scene's name, and a chevron. A lock glyph marks a snapshot-locked scene, and a spinner turns while a switch is in flight.

Open it to see **Default** first, then every saved scene with a check on the active one, then the actions: **New scene**, **Rename**, and **Delete**. New scene duplicates the scene running now unless you pick **Blank**. Rename and Delete act on the active saved scene and do not appear while Default runs. The same picker sits in the same place on the Dashboard and the Effects page. See [Scenes](@/studio/scenes.md) for the full lifecycle.

The active scene is shared app-wide and kept fresh over the WebSocket, so a scene change made from another page, another client, or the command line lands in Studio without a manual refresh.

### The zone tab strip

The second row holds one tab per LED zone, in the scene's zone order. Each tab shows the zone's identity dot (its color, or a neutral dot when it has none), its name, and at most one status glyph:

- A **warning triangle** when a layer on the zone failed to render, or when the zone has no devices yet.
- A **pause glyph** when the zone is paused.

Hover a tab with a glyph for the reason. The selected tab carries the purple underline and the workspace's surface color, so it reads as joined to the pane and Stage below. Zone colors stay on the dots and never tint the tabs themselves.

After the zone tabs come, in order:

- A divider and a single **Screens** tab with the number of screens, whenever the scene drives display screens.
- The **+** button, which makes a new zone and opens its tab with the name ready to type over. It appears when the daemon supports zone creation.

The selected tab also carries a **⋯** button that opens the zone's menu: **Rename**, **Color**, **Pause zone** or **Resume zone**, **Move left**, **Move right**, and **Delete zone**. Right-clicking any tab opens the same menu, and double-clicking a tab renames it in place. Dragging a tab along the strip reorders the zones. See [Zones](@/studio/zones.md) for every zone action.

When the strip runs out of room, tabs shrink to a minimum width and then the tail folds into a **N more** menu. The open tab always stays visible.

{% <callout type="tip" title="Keyboard"> %}
The strip is a standard tab list. With a tab focused, the arrow keys move between tabs and open them as they go, and **Home** and **End** jump to the ends. **F2** renames the focused zone, and **Shift+F10** or the context-menu key opens its menu. Anywhere on the page outside a text field, **[** and **]** step to the previous and next tab.
{% </callout> %}

## The device pane (left column)

The device pane answers one question about the open tab: what is in this zone, and what could be? It has a search field and up to three sections.

### Search

The search field sits at the top of the pane, and **/** jumps to it from anywhere on the page. It filters every section at once. It matches device names, channel names (the hardware's and your own renames), and the names of components bound to channels. Other zones with hits expand, other zones and the Unassigned section hide when they have none, and each count reads like `3 of 19` while a search is active. **Escape** clears the search; a second **Escape** leaves the field.

### This zone

The first section belongs to the open zone. Its header shows the zone's dot, its name, and how many devices it holds, and it stays pinned while the pane scrolls. Device cards list connected devices first, by name. Devices that are placed in the zone but not connected gather in an **Offline** subsection at the bottom.

A device whose outputs are split across zones shows only the channels this zone claims, plus a quiet line such as `2 more outputs in Push 2`. Click that line to open the other zone's row in the next section.

See [Device assignment](@/studio/device-assignment.md) for the device card, its menu, and every way to move a device.

### Other zones

The **Other zones** section lists every other LED zone as one collapsed row: identity dot, name, and device count. Clicking a row expands it into compact device rows and never switches tabs; switching zones is the tab strip's job. A paused zone's row is dimmed.

### Unassigned

The **Unassigned** section lists hardware that no zone claims. Each card has an **Identify** button (the lightning bolt) that flashes the hardware, and a **+** that adds the device to the open zone.

The section header carries the scene's policy for unclaimed outputs as a compact select: **Off**, **Hold last colors**, or **Follow** one of your zones. The policy is editable when the daemon advertises the `scene-unassigned-behavior-write` capability; otherwise it shows as a read-only label.

In a multi-zone scene the section is always there, as a place to move devices out to. In a single-zone scene it appears only when something is actually unassigned.

### Resizing the pane

On wide viewports a drag handle sits between the pane and the Stage. Drag it to set the pane width, anywhere from 240 to 460 pixels. The width persists per browser, and the Stage takes the space that is left.

## The Stage (center column)

The Stage is the workspace for whatever tab is open.

### An LED zone: the live canvas

For an LED zone, the Stage *is* the spatial layout editor. The live effect renders under the device boxes, always on, and only the open zone's outputs appear. Boxes name themselves when their name fits; smaller boxes show their name on hover or selection. A press on overlapping boxes picks the smallest one under the pointer, and **Alt**-click reaches the boxes stacked beneath it. Below the canvas, a properties panel edits the selected output. The full editor is documented in [Layouts](@/studio/layouts.md).

A zone with no devices shows **Drag devices here, or add them from Unassigned** over the canvas.

The Stage header carries the now-playing chip on the left and three canvas actions on the right:

- **Undo** (`Ctrl+Z`) and **Redo** (`Ctrl+Shift+Z`, or `Ctrl+Y`) step through one history of device moves, canvas edits, and zone changes.
- **Fit** scales and centers the zone's devices to fill the canvas, keeping their arrangement.

There is no Save or Revert. A canvas edit commits when you release the drag or the slider, or when you press Enter in a property field or leave it. A device move commits when you make it.

### The Screens tab: the live face

The Screens tab switches the pane to a list of the scene's screens, each with a live thumbnail of its face and its resolution. Screens with identical device names take the per-device label the canvas uses, such as `TL LCD 1` and `TL LCD 2`. Selecting a screen shows its live face on the Stage, with its resolution as `width×height` below the preview.

The Stage header for a screen carries the now-playing chip, a **mounting** select that corrects a panel installed sideways or upside down, a **Preview** label, and a button that opens the face in a full-screen preview tab. Returning to the Screens tab reopens the screen you last had selected.

### The degraded banner

When the open zone or screen has a layer that failed to render or is missing its asset, a red **Degraded** banner appears at the top of the Stage. It is the surface-level alarm; open the composition panel to see which layer is unhappy.

## The now-playing chip

The now-playing chip lives at the left of the Stage header for every zone and screen. It names the top layer of the stack (the effect's own name when the layer has no name of its own), or **No layers** when the stack is empty. A screen showing its display's stored default face adds a **Default** badge.

Clicking the chip toggles the composition slide-over. It is the way layer editing opens, which is what keeps the Stage uncluttered.

## The composition slide-over

The composition panel slides in over the Stage from the right edge when you click the now-playing chip. It hosts the open zone's layer stack (the list of effects, faces, media, and other inputs), and it covers only the Stage, never the device pane or the tabs.

Dismiss it three ways: click the scrim behind it, click the close button in its top-right corner, or press `Escape`.

The panel is the same layer editor used everywhere layers are edited, so what you learn here transfers directly. Layer mechanics (adding a layer, blend modes, opacity, transforms, reordering, and per-layer health) are covered in [Layers](@/studio/layers.md), and live effect controls in [Effects and controls](@/studio/effects-and-controls.md).

## The undo bar

Deleting a zone shows a bar at the bottom of the Stage that reads **Deleted** and the zone's name, with an **Undo** button. It stays for eight seconds. Undo rebuilds the zone with its name, color, layers, brightness, paused state, position, and the devices it held. The bar and `Ctrl+Z` share one record, so a zone comes back once whichever you use. See [Zones](@/studio/zones.md#deleting-a-zone).

## The narrow-viewport drawer

On narrow viewports the device pane collapses into a slide-over drawer to free the Stage. A **Devices** button appears in a strip below the header (it is hidden on wide viewports, where the pane sits beside the Stage).

Tapping **Devices** slides the pane in from the left over a dark scrim. Opening another tab closes the drawer and reveals the Stage behind it. Tapping the scrim dismisses the drawer, and focusing the search field opens it.

## Where to go next

- [Studio overview](@/studio/overview.md): the Scene to Zone to Layers plus Layout model.
- [Scenes](@/studio/scenes.md): switch, create, rename, and delete whole-rig configs.
- [Zones](@/studio/zones.md): add, rename, color, pause, reorder, and delete zones.
- [Device assignment](@/studio/device-assignment.md): the device card, moves, and Unassigned.
- [Layers](@/studio/layers.md): the layer stack, blend modes, and health.
- [Layouts](@/studio/layouts.md): the zone canvas, rig placement, Fit, and undo.
- [Multi-zone walkthrough](@/studio/multi-zone-walkthrough.md): build a multi-zone scene step by step.
