+++
title = "Effects & controls"
description = "Apply effects to a zone and tune the live control panel inside a layer: debounced patching, media playback, and screen-reactive controls."
weight = 80
template = "page.html"
+++

To light a zone, add an effect layer to it and tune that effect from the layer's own control panel. Every slider, color, and toggle edits live: your change applies optimistically the instant you move it, then a debounced patch lands the final value on the daemon without restarting the effect or tearing down the layer between frames.

This page covers picking an effect for a zone, the live control panel, media playback settings, and the extra controls that appear when an effect reacts to your screen.

{{< img path="img/ui/studio.webp" alt="The Studio composition panel with an effect layer's live controls open" />}}

## Put an effect on a zone

There are two ways to light a zone with an effect, and they differ in what happens to the zone's existing layers.

**From Studio, add a layer.** Open the zone's tab, click the now-playing chip on the Stage to open the composition panel, and use **Add layer**. The picker opens with two tabs:

- **Effect**: the runnable effect catalog. On an LED zone it leaves out the display faces meant for screens.
- **Media**: uploaded images and video from your Media library.

Search filters as you type. Click an effect to add it on top of the zone's stack; the layers already there stay.

{% <callout type="info" title="LED zones and screens"> %}
On the Screens tab the **Effect** tab relabels to **Face** and floats the display-category effects (clocks, faces, ambient screen art) to the top of the list. The picker always leads with what makes sense for the tab you are composing.
{% </callout> %}

**From the Effects page, apply.** Click an effect card and it applies to the target zone, replacing that zone's layer stack with one layer running the effect. A favorite or a command palette pick applies the same way.

The catalog spans the native built-in effects compiled into the daemon plus the SDK HTML effects. The exact count grows with every release, so browse the full library with parameters and previews in the [Effects](@/effects/_index.md) section.

### Where an applied effect lands

An apply lands in the app's **target zone**, which is always one LED zone of the active scene:

- The zone whose tab you last had open in Studio.
- Or the zone you pick with the compact zone tabs on the Effects page or Dashboard preview. They appear only in a multi-zone scene, and they move the preview, the controls, the presets, and the target together.

In a multi-zone scene the command palette names the target zone next to its search field, so an apply never lands blind. With one zone, every apply lands in Main. A screen is never a target, and a target zone that has left the scene falls back to Main. For how zones and scenes compose, read [Zones](@/studio/zones.md) and [Scenes](@/studio/scenes.md).

## The live control panel

Once an effect layer exists, its parameter controls render inline under the layer in the composition panel. The panel reads the effect's own control schema from the daemon, so you get exactly the sliders, color pickers, dropdowns, and toggles that effect author defined. No two effects share a fixed control set.

Every edit follows the same path:

1. **Optimistic apply.** The moment you move a control, the local value updates and the canvas reflects it. The UI never waits on a network round-trip to feel responsive.
2. **Coalesce.** The raw edit is queued into a pending batch keyed by control id. Last write per key wins, so dragging a slider sends only its final resting position, not every intermediate frame.
3. **Debounced patch.** After 120 ms of quiet, the coalesced batch is sent as a single partial patch through the dedicated `patch_layer_controls` route.

The 120 ms debounce is a Studio product contract. It is fast enough to feel live while still collapsing a rapid drag into one clean write.

{% <callout type="tip" title="Why the layer never flickers"> %}
Effect-control edits go through a partial, *debounced* patch that updates only the changed control values. They never rewrite the whole layer or restructure the stack, so dragging a slider does not tear the layer row down and rebuild it between frames. The effect keeps rendering the entire time.
{% </callout> %}

### Concurrent edits stay consistent

Control patches are unguarded and apply in commit order against the real layer
id. A whole-layer replacement retires that id, so a late control patch cannot
land on the replacement. The control session keeps local edits coalesced while
requests are in flight, and newer values always win.

This is the same shared control-patch session that powers display-face controls and the standalone effect panel, so the behavior is identical everywhere you tune live controls.

{{< img path="img/ui/ui-effect-controls.webp" alt="Effect control panel in the Hypercolor Studio" />}}

## Media playback controls

A media layer (image or video from your library) carries playback settings instead of effect parameters. Three controls appear under the layer:

- **Speed**: a `0.1×` to `4×` multiplier on playback rate, in `0.05` steps.
- **Loop mode**: **Loop** (repeat from the start), **Ping-pong** (play forward then backward), or **Play once** (stop at the end).
- **Auto-play**: whether the media starts playing as soon as the layer becomes active.

Unlike effect controls, media playback edits rewrite the layer through the standard layer update rather than the debounced control patch. Each change is a discrete, deliberate setting, so there is no slider-drag stream to coalesce.

To add media to a zone, upload it first from the Media library, then pick it from the **Media** tab in the add-layer picker. For how layers stack, blend, and reorder, see [Layers](@/studio/layers.md).

## Screen-reactive controls

When an effect is tagged `screen-reactive`, the control panel grows a **Capture · Shared** section below the effect's own controls. It holds four knobs that tune the captured pixels before they reach your lights: **Saturation**, **Brightness**, **Gamma**, and **Smoothing** (low smoothing is cinematic, high is twitchy).

These knobs are deliberately global. One screen-capture pipeline feeds every screen-reactive effect and layer, so they write the daemon's shared capture config rather than this one layer's controls. Adjust them here and every screen-reactive effect on your rig responds.

The section only appears for effects that actually consume screen input, so a non-reactive effect never shows capture controls it cannot use. For setting up screen capture as an input source and the ambilight workflow, see [audio setup](@/guide/audio-setup.md) for the parallel reactive-input model and the [Effects](@/effects/_index.md) section for screen-reactive effects to try.

{{< img path="img/ui/ui-screen-controls.webp" alt="Screen-reactive capture controls in the Hypercolor Studio" />}}

## Where to go next

- [Layers](@/studio/layers.md): stack effects and media, blend modes, opacity, reorder, per-layer health.
- [Zones](@/studio/zones.md): partition your hardware into zones, each with its own canvas and its own looks.
- [Now playing & transport](@/studio/now-playing-transport.md): the per-zone now-playing rows, the preview's zone tabs, and pause and resume.
- [Multi-zone walkthrough](@/studio/multi-zone-walkthrough.md): build a multi-zone scene and give each zone its own look.
- [Effect controls reference](@/effects/controls.md): how effect authors define the control schema this panel renders.
