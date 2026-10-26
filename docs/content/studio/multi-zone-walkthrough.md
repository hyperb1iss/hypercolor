+++
title = "Multi-zone walkthrough"
description = "Build a multi-zone scene step by step: start a scene, add zones, bring devices in, let the rig place them, give every zone its own look, and undo anything."
weight = 90
+++

By the end of this walkthrough one scene runs three looks at once: the keyboard does its own thing, the desk strips do another, and everything else keeps the main look. You will start a scene, add zones from the tab strip, bring devices into them, let Studio place them from your rig, give each zone an effect, and tidy the zones up. Every step commits as you take it, and anything you change can be put back.

{{< img path="img/ui/studio.webp" alt="Studio with three zone tabs, the Keyboard tab open, its device fitted to the canvas, and an effect rendering under it" />}}

This is the end-to-end version of the focused pages. For the details behind each step, see [Zones](@/studio/zones.md), [Device assignment](@/studio/device-assignment.md), [Layouts](@/studio/layouts.md), and [Layers](@/studio/layers.md).

{% <callout type="info"> %}
A **scene** is the whole-rig configuration. A **zone** is a partition of that scene with its own canvas and its own layers. One scene holds many zones, each driving its own set of device outputs. Switching scenes swaps every zone together; this walkthrough builds one scene.
{% </callout> %}

## Before you start

Open Studio in the web UI and check two things:

- More than one LED device (or one device with several channels) is connected, so there is something to split. See [Finding devices](@/guide/finding-devices.md) if your rig is empty.
- Your devices sit roughly where they physically are on the **Main** tab's canvas. Zones place arriving devices from that arrangement, so a minute spent here pays off in every zone you build.

{% <callout type="tip"> %}
In a scene with a single zone, the Main canvas is your rig's layout, shared with every single-zone scene. Arrange it there once and every zone you make later starts from it. See [Layouts](@/studio/layouts.md#where-a-canvas-edit-lands).
{% </callout> %}

## 1. Start a scene for it

Build the split in its own scene, so the scene you have now stays as it is.

1. Open the **scene picker** at the right of the Studio header.
2. Choose **New scene**.
3. Type a name, such as `Desk Session`.
4. Leave **Duplicate current** selected. The new scene starts as a copy of the one running now: the same Main zone, the same device placement, the same look.
5. Click **Create scene**.

Studio switches to the new scene. The header now shows `Desk Session`, and the tab strip shows one **Main** tab with a **+** after it.

Choose **Blank** instead to start with one Main zone holding every device in your rig, and no effects yet. See [Scenes](@/studio/scenes.md) for both.

## 2. Add a zone

Click **+** after the tabs. A tab named **New zone** opens with its name selected. Type `Keyboard` and press **Enter**.

The new zone starts empty:

- Its canvas reads **Drag devices here, or add them from Unassigned**.
- Its tab carries a small warning triangle that means "no devices in this zone yet".
- The device pane expands every other zone's row under **Other zones**, so the devices you can bring in are right there.

Your **Main** zone still holds everything. Making a zone never moves devices on its own; that is the next step.

## 3. Bring devices in

The keyboard is still in Main. Pick whichever route suits you; each one is a single move.

- **Drag it.** Under **Other zones**, find the keyboard in the Main row and drag it onto the Keyboard canvas. It lands centered where you drop it.
- **Use the menu.** Click the keyboard's row under **Other zones** to open **Move to**, then choose **Keyboard**.
- **From the Main tab.** Open the keyboard's card, click **⋯**, then **Move to** and **Keyboard**. You can also drag the card straight onto the **Keyboard** tab. Hold it over the tab for about half a second and the tab springs open, so you can keep dragging onto its canvas.
- **From Unassigned.** A device no zone claims has a **+** on its card that adds it to the open tab's zone.

{% <callout type="tip" title="Steps 2 and 3 in one move"> %}
On the Main tab, open the keyboard card's **⋯** menu and choose **Move to**, then **A new zone**. Studio creates a zone named after the keyboard, moves the keyboard in, and opens the tab with the name selected so you can type over it. One `Ctrl+Z` takes the whole thing back.
{% </callout> %}

Screen readers hear the move announced, for example "Moved Razer Huntsman V2 to Keyboard from Main", and a toast confirms it saved.

## 4. Check where it landed

The keyboard arrives fitted to the Keyboard canvas: a zone's first devices scale up to fill it, keeping their physical arrangement, so the zone's effect covers them completely.

Bring in a second device, a mouse or a headset stand, and it lands where it sits relative to the keyboard in your rig, carried onto the canvas at the same scale. Nothing piles up in a corner. (A later device your rig does not place lands at its own built-in footprint, or at the daemon's default spot; drag it where it belongs.)

To tidy up by hand:

- Drag a box to move it, or grab a corner handle to resize it. The change commits when you release.
- Click **Fit** in the Stage header to scale and center everything in the zone to fill the canvas again.

Every zone keeps its own canvas, so arranging the Keyboard zone never moves anything on the Main canvas. See [Layouts](@/studio/layouts.md) for the full editor.

## 5. Give the zone its look

With the **Keyboard** tab open, use either route:

- **From the Stage chip.** Click the now-playing chip at the left of the Stage header. It reads **No layers** for a new zone. The composition panel slides in; click **Add layer**, stay on the **Effect** tab, and pick an effect, such as Borealis. It renders under the keyboard's box right away.
- **From the Effects page.** The Keyboard zone is now the app's target zone, and the preview's zone tabs show it selected. Click an effect card and it replaces the Keyboard zone's layers with that effect.

Tune the effect from its controls in the composition panel. See [Effects and controls](@/studio/effects-and-controls.md) for live tuning and [Layers](@/studio/layers.md) for blend modes and stacking.

## 6. Give every zone its own look

Now repeat for the rest of the rig.

1. Click **+** again and name the zone `Desk strips`.
2. Bring the strips in. A controller can split, so you can move just the channel that drives them. Open the **Main** tab, find the controller's card (a Nollie, say), and drag its strip channel's row onto the **Desk strips** tab. Or open **Move to** from the card, pick that channel under **Move**, and choose **Desk strips**. The rest of the controller stays in Main, and its card there now shows a line such as `1 more output in Desk strips`.
3. Open the **Desk strips** tab and give it an effect from its Stage chip.
4. Open the **Main** tab and change its look too.

All three zones render at once, each from its own canvas. Use **[** and **]** to step between the tabs, or the arrow keys with a tab focused.

{% <callout type="info"> %}
Every zone shares the daemon's global audio, screen-capture, and sensor inputs. An audio-reactive effect in one zone and a static effect in another both read the same audio; the static one ignores it. Per-zone input routing (a zone capturing its own monitor) is future work.
{% </callout> %}

## 7. Pause, rename, recolor, and reorder

Every zone setting lives on its tab. Right-click a tab, or click the **⋯** on the open tab:

- **Rename** turns the tab into a text field. Double-clicking the tab does the same.
- **Color** gives the zone an identity dot. Zone colors label the zone in Studio and elsewhere; they never change what your LEDs show.
- **Pause zone** stops the zone driving its devices while keeping its layers and canvas. The tab shows a pause glyph until you choose **Resume zone**. The sidebar's Now Playing rows pause and resume zones too.
- **Move left** and **Move right** reorder the tabs. Dragging a tab along the strip works as well.

## 8. Undo anything

Studio commits every step as you take it, so undo is how you back out.

Press `Ctrl+Z`, or use the **Undo** arrow button in the Stage header, to take back the last step. `Ctrl+Shift+Z` or **Redo** puts it back. One history holds every step in the order you made it:

- **Device moves:** each move, drop, Unassigned **+**, and **Remove from this zone**. Undoing a move puts the device back where it was, in its old zone.
- **Canvas edits:** each drag, resize, field change, and **Fit**.
- **Zone changes:** each new zone, rename, recolor, pause or resume, and reorder. A **Move to** ▸ **A new zone** is one step, zone and all, so one undo puts the device back and removes the zone. If a zone you made has gained devices or layers since, undo keeps it and says so, then carries on.
- **Deleted zones:** a delete is a step like any other, and the **Deleted** bar's **Undo** button offers the same restore for eight seconds. The zone comes back with its name, color, layers, brightness, paused state, place among the tabs, and devices.

Screen readers hear what each undo and redo did, such as "Undid: moved Razer Huntsman V2 back to Main" or "Redid: paused Main".

The undo history lasts while you stay in Studio on this scene. Switching scenes or leaving Studio starts it fresh.

## 9. Decide what unassigned hardware does

Anything you leave in no zone shows under **Unassigned** at the bottom of the device pane, for example after you delete a zone or choose **Remove from this zone** on a card. The section's header sets what that hardware shows:

- **Off**: unassigned outputs go dark. This is the default.
- **Hold last colors**: they keep whatever they last showed.
- **Follow** a zone: they mirror that zone's look.

The policy is editable when the daemon advertises the `scene-unassigned-behavior-write` capability; otherwise it shows read-only. A device split across zones is handled per output: its claimed channels render from their zones, and its unclaimed channels follow the policy.

## What you built

One scene, three zones, three looks, and a defined fate for everything in between:

{% <mermaid> %}
graph TD
    SCENE[Desk Session scene] --> Z1[Main<br/>case fans, RAM, the rest]
    SCENE --> Z2[Keyboard zone]
    SCENE --> Z3[Desk strips zone]
    SCENE --> UN[Unassigned outputs]
    Z1 --> E1[Main look]
    Z2 --> E2[Keyboard look]
    Z3 --> E3[Desk strips look]
    UN --> POL[Unassigned policy:<br/>off / hold / follow a zone]
{% </mermaid> %}

All three zones render together and switch together when the scene changes. Changing one zone's look never disturbs the others. Switch back to the scene you started from with the scene picker any time; `Desk Session` waits in the list, exactly as you left it.

## Where to go next

- [Zones](@/studio/zones.md): the full zone lifecycle: add, rename, color, pause, reorder, and delete.
- [Device assignment](@/studio/device-assignment.md): every way to move devices and channels, and where arrivals land.
- [Scenes](@/studio/scenes.md): creating, duplicating, and switching whole-rig configurations.
- [Zone API and concurrency](@/studio/zone-api-and-concurrency.md): the REST routes and the single scene `revision` contract behind every move you just made.
