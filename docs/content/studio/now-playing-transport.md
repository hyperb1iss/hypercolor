+++
title = "Now playing & transport"
description = "Select a zone, preview its effect, and control playback from the sidebar or Studio."
weight = 100
+++

The now-playing controls show what each zone is rendering and let you steer it. They live in three places: the **Now Playing panel** at the bottom of the sidebar, the **now-playing chip** in the Studio Stage header, and the **zone tabs** on the Dashboard and Effects preview. The sidebar selector and preview tabs share one selected zone.

{{< img path="img/ui/studio.webp" alt="Studio with the Now Playing panel in the sidebar and the now-playing chip on the Stage" />}}

## The sidebar Now Playing panel

The Now Playing panel sits below the nav and above the collapse toggle. It appears whenever something is rendering, and it carries a live thumbnail of the running effect, an extracted color palette, transport controls, and a global brightness slider.

The panel's glow picks up the running effect's colors. In a single-zone scene, the header names the effect and its category. A multi-zone scene shows a compact zone selector with the selected zone's name, color swatch, and effect label.

### Transport controls

The control row carries five actions:

- **Previous** and **Next** step through the effect catalog. They walk the runnable effects in order and wrap around, so Next from the last effect returns to the first.
- **Pause / Resume** is the center button. It is global: it pauses or resumes all device output at once, in every scene shape.
- **Shuffle** jumps to a random runnable effect, skipping the one already playing when more than one exists.
- The **brightness slider** sets global brightness from 0 to 100 percent. It pushes updates as you drag, throttled so a fast drag does not flood the daemon.

Previous, Next, and Shuffle act on the zone shown in the preview. Changing the selected zone updates both the preview and these controls on every page.

{% <callout type="info"> %}
Previous, Next, and Shuffle only consider effects flagged `runnable`. The full catalog is browsable from the [Effects](@/effects/_index.md) section; the transport is the quick way to cycle through it without leaving your current page.
{% </callout> %}

### The audio toggle

A small audio button rides in the metadata row of a single-zone scene. Its behavior depends on the running effect:

- **Audio on:** a waveform icon that glows coral and pulses purple on the beat. Click to disable audio.
- **Audio off, effect is audio-reactive:** a muted icon, dimmed. Click to enable audio.
- **Audio off, effect is not reactive:** hidden entirely, since there is nothing to react to.

For the full audio pipeline and device setup, see [Audio setup](@/guide/audio-setup.md).

## Selecting a zone in a multi-zone scene

When the active scene has more than one LED zone, the sidebar shows one selected-zone card. The card stays the same height as you add zones. Click its name to open a dropdown containing every LED zone; the list scrolls within a bounded panel rather than growing the sidebar.

Selecting a zone moves the preview, transport, effect controls, and apply target together. The card shows that zone's color swatch, name, and current effect or layer label. An empty zone reads **No effect**. Paused zones are marked **(paused)** in the dropdown.

Use the arrow keys to move through the open list, Home or End to jump, and Enter to select. Escape closes the dropdown.

### Pausing one zone

The button beside the selector pauses or resumes the selected zone. Its tooltip names the target, such as **Pause Keyboard** or **Resume Desk**. Other zones keep rendering.

The center transport button pauses or resumes all device output. Its tooltip reads **Pause all output** or **Resume all output**. Global brightness also applies to every zone.

Under the hood, a per-zone pause flips that zone's `enabled` flag through a guarded zone PATCH, not the global stop. While Studio is open, the pause goes through Studio's history, so `Ctrl+Z` takes it back like a pause from the tab menu; on other pages it writes directly. If the scene changed underneath you between read and write, the toast reads **"Scene changed, try again"** and the panel reloads rather than clobbering someone else's edit. That optimistic-concurrency contract is shared across Studio; see [Zone API and concurrency](@/studio/zone-api-and-concurrency.md) for how the revision guard works.

## The Studio Stage now-playing chip

The Stage header carries its own now-playing chip, and it does a different job from the sidebar's. It names the **top layer** of the open zone or screen: the layer's own name, else the name of the effect it runs, else its source kind. It reads **No layers** when the stack is empty, and a screen showing its display's stored default face adds a **Default** badge. Clicking the chip toggles the composition slide-over, the way layer editing opens in Studio, so the chip is your handle on the layer stack.

The chip appears for every LED zone and every screen, because both carry a layer stack and both need a way to open the composition panel. For how the layer stack itself works, see [Layers](@/studio/layers.md).

{{< img path="img/ui/ui-studio-zones.webp" alt="The Studio Stage header with the now-playing chip naming the open zone's top layer" />}}

## The preview's zone tabs

In a multi-zone scene, the preview on the Dashboard and the Effects page shows a row of compact zone tabs in its info overlay, one per LED zone: the zone's identity dot, its name, and one status glyph, the same ones the Studio tabs use (a warning for a failed layer or a zone with no devices, a pause glyph for a paused zone). Below the tabs, a caption names what the selected zone is showing. With a single zone the tabs are hidden and the preview shows the effect's own title instead.

The tabs choose the app's **target zone**. Selecting one moves the preview, the controls, the presets, and the target for the next effect apply together, and Studio opens on that zone next time. The arrow keys move between the tabs and select as they go. Hovering a tab reads **Show and edit** and the zone's name.

The tabs, preview caption, and sidebar selector share the same zone selection and scene data. Selecting a zone in either place updates the other.

## Where the panel runs, and in what mode

The sidebar canvas thumbnail behaves differently depending on the page you are on. On pages that already have their own large preview, the sidebar drops the redundant thumbnail and instead drives the ambient palette. On `/`, `/effects`, and `/studio`, the Now Playing canvas runs in **Palette** mode: it extracts colors from the live frame to tint the chrome rather than drawing a second copy of the effect. Every other page (`/media`, `/devices`, `/settings`) shows the live thumbnail instead.

Both preview modes follow the selected zone. Navigating to another page keeps that selection.

## How it all connects

{% <mermaid> %}
graph TD
    Scene[Active scene] --> ZE[Per-zone effect state]
    ZE --> Selector[Sidebar zone selector]
    ZE --> Tabs[Preview zone tabs]
    Tabs -->|select| Target[Target zone]
    Selector -->|select| Target
    Target --> Preview[Preview and transport]
    Selector -->|pause / resume selected zone| PATCH[Guarded zone PATCH]
    PATCH -->|enabled flag| Scene
    Chip[Stage now-playing chip] -->|click| Comp[Composition slide-over]
{% </mermaid> %}

## Related pages

- [Studio overview](@/studio/overview.md): the Scene, Zone, Layers, and Layout model.
- [Zones](@/studio/zones.md): add, color, pause, and reorder the zones that partition your rig.
- [Effects and controls](@/studio/effects-and-controls.md): applying effects to a zone and tuning them live.
- [Multi-zone walkthrough](@/studio/multi-zone-walkthrough.md): build a multi-zone scene step by step.
