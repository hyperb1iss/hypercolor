+++
title = "Device grouping"
description = "Add and move devices between Studio zones, read the device card, hide/identify/remove outputs, and assign at output level in multi-zone scenes."
weight = 50
+++

Devices land in zones through the Studio zone tree. Each device shows up as a card under the zone that owns it, with one-tap actions to add it, move it, hide its outputs, flash the hardware, or pull it back out. In a multi-zone scene you can go finer still and reassign individual outputs across zones. This page walks the whole flow.

The unit that actually moves between zones is an **output**, not a whole device. An output is one device output or addressable segment. A single-segment strip has one output; a multi-channel controller contributes several. A device output lives in exactly one zone's layout at a time, so adding a device to a new zone *moves* its outputs there rather than copying them.

{% callout(type="info") %}
New to the Scene → Zone → Layout model? Read [Zones](@/studio/zones.md) first. Scenes are whole-rig configs; zones are flexible partitions of a scene's canvas. There is no "rooms" concept here.
{% end %}

![The Studio workspace, with the zone tree on the left listing zones and their devices](/img/ui/studio.webp)

## The device card

Every physical device under a zone renders as a card in the zone tree. The card carries the device's brand identity and its live state:

- A **duotone accent strip** down the left edge, tinted to the vendor's brand colors.
- The **vendor mark** (or a device-class icon when there is no mark to carry the identity).
- The **device name**, then a meta line with the **LED count**, transport (USB, Network, SMBus, Bridge, MIDI, or Serial), and a driver label when there is no vendor mark.
- For a screen device, the card shows the real **resolution** (for example `2560 × 1440`) instead of an LED count, because a screen's layout is a pixel grid rather than addressable LEDs.

Clicking the card body selects that device's zone and highlights its outputs on the Stage canvas. Hovering previews those outputs.

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

### Channels and outputs

A device with more than one channel expands into a **per-channel breakdown** beneath the card body. Each channel row shows:

- The channel's **topology shape** (strip, ring, matrix) and its name.
- A live **component badge** when something is wired to that channel, naming the attached component or a count like `3 components`. Hover the badge for the full per-component LED breakdown.
- The channel's **LED count**.
- A per-output **hide toggle**, when that channel has a placed output in the current zone.

The card shows each channel's effective name — your rename if you've set one, otherwise the device's own channel name — never a raw slot id.

## Adding a device to a zone

Each zone in the tree ends with an **Add device** button. Click it to open a picker listing every device that is not already entirely in this zone. Each option shows where the device currently lives, for example `Corsair LS100 (in Desk)` or `WLED strip (unassigned)`, so you can see what the move pulls from.

Picking a device brings every output it has into this zone:

- A device already placed in **another zone** is **moved**: its existing outputs are reassigned to the target zone.
- A device the scene has **not placed at all** is **minted** fresh, one output per channel, or a single output for a device with no channels. The daemon resets position and size on assign, so the device drops onto the zone's canvas with sensible defaults.

In a **single-zone scene**, devices that sit in no zone fold under the sole LED zone as one-tap **Available** rows. Each Available card carries a green **add** (`+`) action that drops the device straight into the zone, no picker needed.

{% callout(type="tip") %}
Can't find a device in the picker? It may already be entirely in this zone, or it may not be connected yet. Check [Finding devices](@/guide/finding-devices.md) to confirm the daemon sees it.
{% end %}

## Hide, identify, remove

Each placed device card carries a trailing action cluster:

| Action | Icon | What it does |
| --- | --- | --- |
| Hide all | eye | Hides every output of this device in this zone, or shows them again if all are hidden |
| Identify | lightning | Flashes the hardware so you can spot it physically |
| Remove | trash | Unassigns every output this device has in this zone |

**Hide** is a Studio view convenience, not a render change. Hidden state is client-side UI state keyed per scene and per zone, persisted in your browser. It hides the output's box on the canvas so you can declutter a busy layout. It is **not** the daemon's discovery-reconciliation memory, and hiding an output does not stop it rendering. Use the per-channel eye toggle to hide a single output, or the card's hide-all toggle to move every output of the device in unison.

**Identify** flashes the device's LEDs through the daemon so you can match the on-screen card to a physical light. It is available whenever the device is online. A brief toast confirms the flash.

**Remove** pulls every one of the device's outputs out of this zone. For a multi-output controller the removals run in sequence as a single user action, so the whole device leaves the zone in one click. The device then becomes Unassigned (or Available, in a single-zone scene) and can be added to a different zone.

### Offline devices

A device that is placed in the layout but not currently connected renders as a muted, dashed row tagged **Offline**. It shows a friendly vendor word (Razer, Corsair, Philips Hue) rather than its raw backend id, which is never shown to you. An offline device can still be **removed** from the zone, but it cannot be identified, since there is no hardware to flash.

## Output-level assignment in multi-zone scenes

When a scene has more than one zone, a **Zone assignment** panel docks below the Stage canvas. This is the fine-grained tool: instead of moving a whole device, you select individual outputs and reassign them.

The panel lists every output grouped by its owning zone, and within a zone by physical device. A multi-channel controller can have its segments split across different zones: one fan ring in `Desk`, another in `Shelf`.

To reassign:

1. Click output chips to multi-select them. Selected chips highlight in the accent color.
2. The toolbar shows the selection count and an **Assign to** zone picker.
3. Pick a target zone. The selected outputs move there, and the selection clears.

Each output chip names its device, and a multi-channel device also names the segment (for example `Lian Li · Front fans`). Like the device picker, chips never show a raw id.

{% callout(type="info") %}
Every assignment carries the active scene's revision as a precondition. If another client or the CLI changed the scene while you were working, the move is rejected cleanly, the scene reloads, and a toast asks you to try again. Your edit is never silently clobbered. See [Zone API and concurrency](@/studio/zone-api-and-concurrency.md) for the mechanics.
{% end %}

## Where devices go from here

Once a device is grouped into a zone, position it on the zone's canvas in the [layout builder](@/studio/layouts.md), then stack effects and other inputs on the zone in the [layer stack](@/studio/layers.md). For the full end-to-end path of splitting a rig across multiple zones, see the [multi-zone walkthrough](@/studio/multi-zone-walkthrough.md).

## Related pages

- [Zones](@/studio/zones.md) — create, color, and manage the partitions devices live in
- [Scenes](@/studio/scenes.md) — the whole-rig configs that own your zones
- [Layouts](@/studio/layouts.md) — arrange a zone's device outputs spatially
- [Finding devices](@/guide/finding-devices.md) — get hardware discovered before grouping it
