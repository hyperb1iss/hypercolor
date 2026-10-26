+++
title = "Device assignment"
description = "Bring devices into Studio zones and move them between zones by drag or Move to, read the device card, split single channels, and undo any move."
weight = 50
+++

Devices join zones from the Studio device pane. The pane is about the open tab: the open zone's devices first, then the scene's other zones, then the hardware no zone claims. Every device is a card you can drag, and every drag has a menu twin, so a move is always one gesture or one menu choice away. This page walks the whole flow.

The unit that actually moves between zones is an **output**, not a whole device. An output is one device output or addressable segment. A single-segment strip has one output; a multi-channel controller contributes several. An output lives in exactly one zone at a time, so moving a device into a zone takes its outputs out of wherever they were.

{% <callout type="info"> %}
New to the Scene, Zone, and Layout model? Read [Zones](@/studio/zones.md) first. Scenes are whole-rig configs; zones are partitions of a scene, each with its own canvas.
{% </callout> %}

{{< img path="img/ui/studio.webp" alt="The Studio device pane: the open zone's device cards, the Other zones rows, and the Unassigned section" />}}

## The device card

Every physical device in the open zone renders as a card. The card carries the device's brand identity and its live state:

- A **duotone accent strip** down the left edge, tinted to the vendor's brand colors.
- The **vendor mark** (or a device-class icon when there is no mark to carry the identity).
- The **device name**, wrapping to a second line before it truncates, then a meta line with the **LED count**, the transport (USB, Network, SMBus, Bridge, MIDI, or Serial), and a driver label when there is no vendor mark.
- For a screen device, the card shows the real **resolution** (for example `2560 × 1440`) instead of an LED count, because a screen is a pixel grid rather than addressable LEDs.

Clicking the card body selects the device's outputs on the canvas, and hovering it previews them. The link runs the other way too: resting the pointer on a box on the canvas softly lights the card that owns it. A screen device whose face has no boxes in this zone opens its screen on the Screens tab instead.

{{< img path="img/ui/ui-devices.webp" alt="Device discovery in the Hypercolor web UI" />}}

### Channels and outputs

A device with more than one channel lists a **per-channel breakdown** beneath the card body. Each channel row shows:

- The channel's **topology shape** (strip, ring, matrix) and its name.
- A live **component badge** when something is wired to that channel, naming the attached component or a count like `3 components`. Hover the badge for the full per-component LED breakdown.
- The channel's **LED count**.
- An **eye** toggle that hides that one output on the canvas.

The card shows each channel's effective name (your rename if you've set one, otherwise the device's own channel name), never a raw slot id. Clicking a channel row selects its box on the canvas.

A device whose outputs are split across zones lists only the channels the open zone claims, followed by a line such as `2 more outputs in Push 2`. Clicking that line expands the other zone's row under **Other zones** and scrolls it into view.

## Card actions

A card in the open zone carries exactly two trailing icons:

| Icon | What it does |
| --- | --- |
| Eye | Hides every output of the device on this zone's canvas, or shows them again |
| **⋯** (Device options) | Opens the card's menu: **Identify**, **Move to**, and **Remove from zone** |

**Identify** flashes the device's LEDs through the daemon so you can match the card to a physical light, and a toast confirms the flash. **Move to** opens the move menu described below. **Remove from zone** sends the device's outputs in this zone to Unassigned.

**Hide** is a Studio view convenience, not a render change. A hidden output's box fades to a faint gray ghost that ignores clicks, which declutters a busy canvas; the output keeps rendering on your hardware. Hidden state is client-side, kept per scene and per zone in your browser, and it is not the daemon's discovery memory. Use a channel row's eye to hide one output, or the card's eye to hide them all.

Cards in the **Unassigned** section look different. They carry a lightning **Identify** button and a **+** that adds the device to the open tab's zone; hover the **+** to see which zone that is. Their channels fold behind a `N channels` toggle until the device is in a zone.

### Offline devices

A device that is placed in the zone but not connected gathers in the **Offline** subsection at the bottom of the zone's devices, as a dimmed, dashed card. It shows a friendly vendor word (Razer, Corsair, Philips Hue) rather than its raw backend id, an **Offline** tag, and its LED count. Its **⋯** menu offers **Remove from this zone** and **Forget controller**, which removes the saved controller's attachments and outputs from every layout and scene after a confirmation. A device that is only temporarily unplugged keeps its placements, so forget a controller only when it is gone for good.

## Bringing a device into a zone

The quickest route depends on where the device is now.

- **From Unassigned:** click the **+** on its card. The device joins the open tab's zone.
- **From another zone:** open its card's **⋯** menu and choose **Move to**, or expand that zone's row under **Other zones** and click the device's compact row to open the same menu.
- **Anywhere:** drag the card to where it should go.

### Dragging

Drag a device card, a single channel row, or a compact row under **Other zones**, and drop it on one of these targets. Only the targets that accept the drop light up as you move over them, a badge counts the outputs when more than one moves, and a hint under the dragged card names what releasing will do.

| Drop on | Result |
| --- | --- |
| The Stage canvas | The device joins the open zone, centered on the drop point. A device already in the open zone just moves to that point. |
| Another zone's tab | The device moves to that zone without switching tabs. |
| A zone's tab, held for about half a second | The tab springs open (a ring fills while you hold), and you keep dragging onto its canvas. |
| The **+** after the tabs | A new zone named after the device is created, the device moves in, and its tab opens with the name ready to type over. |
| Another zone's row under **Other zones** | The device moves to that zone. |
| The **Unassigned** section | The device leaves its zone. |

Boxes work the same way from the canvas: drag an output box off the canvas onto another zone's tab and that output moves there.

Press **Escape** to cancel a drag. On a touchscreen, press and hold a card briefly to pick it up, so a quick swipe still scrolls the pane. The pane scrolls on its own while you drag near its top or bottom edge.

### Move to: the menu twin

Every drag has a menu equivalent, which works from the keyboard, with one pointer, or with a screen reader. Open **Move to** from any of these:

- A device card's **⋯** menu, then **Move to**.
- Right-clicking a device card, or pressing **Shift+F10** or the context-menu key on it.
- Right-clicking a channel row, or pressing **Shift+F10** on it.
- Clicking a compact row under **Other zones**, or pressing **Enter** on it.
- The **Move to** button in the canvas properties panel, for the selected outputs.

The menu lists every other LED zone with its identity dot, then **Unassigned**, then **New zone**. **New zone** creates a zone named after the device, moves the device in, and opens the new tab with the name selected so you can type over it.

### What moves: a whole device or one channel

The **Move to** menu for a multi-channel device starts by asking what moves. Under **Move**, choose **Whole device** or any one of its channels by name; then pick the destination under **Move to**. Opening the menu from a channel row preselects that channel. Dragging a channel row moves just that channel.

This is how one controller splits across zones: one fan ring in `Desk`, another in `Shelf`, the Push 2's pads in their own zone while its white buttons stay in Main. The split shows up on the cards as the `more outputs in` line.

{% <callout type="warning"> %}
Each output belongs to exactly one zone at a time. Moving an output into a zone always takes it out of its previous zone. There is no "copy an output into two zones"; exclusivity is what keeps each zone's output correct.
{% </callout> %}

## Where an arriving device lands

Every zone keeps its own canvas, and an arriving device is placed from your rig, the active named layout that records where your hardware physically sits:

- **A zone's first devices** fit the canvas together, keeping their physical arrangement, so an effect covers them completely.
- **Later arrivals** keep their physical relation to the devices already in the zone: a fan that sits above the strip in your case lands above it on the canvas.
- **A drop on the canvas** centers the arrival on the drop point instead.
- **A device your rig does not place** still joins the fit when it is among a zone's first devices. Arriving later, it takes the device's own built-in footprint when it has one (a Push 2 does), and otherwise the daemon's default placement. One moved from another zone keeps the spot it had.

To rescale everything afterward, click **Fit** in the Stage header. See [Layouts](@/studio/layouts.md) for placement and the canvas editor.

## Undo and announcements

Every move is one step in Studio's undo history: a menu choice, a drop, an Unassigned card's **+**, or a **Remove from zone**. A **New zone** move is one step too, zone and all: undoing it puts the device back and removes the zone. Press `Ctrl+Z` to undo and `Ctrl+Shift+Z` to redo, or use the **Undo** and **Redo** arrow buttons in the Stage header. Undo puts the outputs back in their old zone at the placement they had, unless you have moved them on that canvas since. Device moves, canvas edits, and zone changes share the one history, in the order you made them. It belongs to the current Studio visit and resets when you switch scenes or leave Studio.

Moves save as you make them, and a toast confirms each one. While a move is saving, the canvas holds still for a moment so the next edit lands on the updated zone.

Screen readers hear every move announced by name and place, for example "Moved Razer Huntsman V2 to Keyboard from Main". A drag that ends without a move announces "Move cancelled", and a tab that springs open under a drag announces the zone it opened. After a move from a menu, focus returns to the control that opened it.

{% <callout type="info"> %}
Every assignment carries the active scene's revision as a precondition. If another client or the CLI changed the scene while you were working, the move is rejected cleanly, the scene reloads, and a toast asks you to try again. Your edit is never silently clobbered. See [Zone API and concurrency](@/studio/zone-api-and-concurrency.md) for the mechanics.
{% </callout> %}

## Where devices go from here

Once a device is in a zone, arrange it on the zone's canvas in the [layout editor](@/studio/layouts.md), then stack effects and other inputs on the zone in the [layer stack](@/studio/layers.md). For a full multi-zone build from scratch, follow the [multi-zone walkthrough](@/studio/multi-zone-walkthrough.md).

## Related pages

- [Zones](@/studio/zones.md): add, color, and manage the zones devices live in
- [Scenes](@/studio/scenes.md): the whole-rig configs that own your zones
- [Layouts](@/studio/layouts.md): arrange a zone's outputs on its canvas
- [Finding devices](@/guide/finding-devices.md): get hardware discovered before you assign it
