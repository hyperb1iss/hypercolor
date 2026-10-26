+++
title = "Scenes"
description = "Scenes are whole-rig configurations. Switch, create, duplicate, rename, and delete them with the scene picker, and learn how the Default scene and the target zone fit together."
weight = 30
+++

A scene is your whole rig captured as one configuration: every zone, every layer, every layout, every effect and control. Exactly one scene is active at a time, and switching scenes rewrites the entire rig in one move. The scene picker sits at the trailing edge of the page header in Studio, on the Dashboard, and on the Effects page: the same control, in the same place, behaving the same way.

{{< img path="img/ui/studio.webp" alt="The Studio header with the scene picker at its trailing edge" />}}

If you want the full mental model first, read the [Studio overview](@/studio/overview.md). This page covers the scene itself: the switch, create, rename, and delete cycle, the Default scene you start out in, and how the active scene connects to where effects land.

## What a scene owns

A scene is the top-level object. It owns everything below it:

- The **zones** that partition your hardware (see [Zones](@/studio/zones.md)).
- Each zone's **layer** stack of effects, faces, media, screen capture, and color (see [Layers](@/studio/layers.md)).
- Each zone's **layout**, where its device outputs sit on its canvas (see [Layouts](@/studio/layouts.md)).
- The **Unassigned** policy for hardware no zone claims.

Switching scenes is therefore a whole-rig change, not a per-light tweak. Because the active scene is shared across the entire app, a switch made in Studio, from the dashboard, from another browser tab, or from the CLI lands everywhere at once.

Edits land in the active scene as you make them. There is no separate save step: a device move, a canvas edit, or a new layer is part of the scene the moment Studio confirms it.

{% <callout type="info"> %}
Scenes are whole-rig configurations and zones are partitions of a scene. Hypercolor uses no smart-home vocabulary for either. See [Vocabulary and naming](@/studio/vocabulary-and-naming.md) for the locked terms.
{% </callout> %}

## The scene picker

The picker's trigger shows a **Scene** label, the active scene's name, and a chevron. Two glyphs can join them:

- A **lock** marks a snapshot-locked scene. It still activates, but it refuses structural edits until you replace its stored document.
- A **spinner** turns while a switch is in flight.

Click the trigger to open the panel:

1. **Default**, always first.
2. Every saved scene, with a check on the active one and a lock on snapshot-locked ones.
3. After a divider, the actions: **New scene**, **Rename**, and **Delete**.

{{< img path="img/ui/ui-scenes.webp" alt="The scene picker open: Default, the saved scenes, and the New scene, Rename, and Delete actions" />}}

The panel is a menu: scenes are radio items with the active one checked, and the actions sit in their own group. It works from the keyboard. The arrow keys on the trigger open it, and focus moves into the menu as it opens. The arrow keys, **Home**, and **End** move the highlight, **Enter** or **Space** chooses, and **Tab** or **Escape** closes it and returns focus to the trigger. Name entry and the delete confirmation happen inside the panel, so you never leave it mid-task.

## The Default scene

When you first run Hypercolor, you are in the **Default** scene. It is the live working state of your rig before you have saved anything, and the daemon never lists it as a saved scene. The picker still names it, as **Default**, so you always know what is on screen. **Rename** and **Delete** do not appear while it runs, because there is nothing persisted to rename or delete.

On a brand-new install the Default scene starts with Hyperbloom in its Main zone, so something is on screen before you pick an effect. From then on it holds whatever you leave in it, including nothing at all.

The **Default** row is always in the panel. Choose it to return to the Default scene from any saved scene; the daemon rebuilds the ephemeral working state, and the picker flips once it confirms the switch.

## Create a scene

Choose **New scene**, type a name, pick where it starts, and click **Create scene**:

- **Duplicate current** (the default) copies the zones, layers, and layout running now. A duplicate starts editable, so you can change it right away.
- **Blank** starts a fresh scene: one Main zone with every device in the rig, and no effects yet.

The new scene is created and activated immediately. A few details worth knowing:

- An empty name creates nothing.
- **Cancel** or **Escape** returns to the list without creating anything.
- If a duplicate is created but cannot be made editable, it still appears in the list, and a toast explains why Studio did not switch into it.

{% <callout type="tip" title="Duplicate to make variants"> %}
Duplicating keeps everything you built. Make your daily scene once, then duplicate it into Gaming, Movie, and Off: each copy starts with the same zones and the same device placement, and you change only the looks.
{% </callout> %}

## Switch scenes

Choose a different scene in the panel. The daemon activates it and the entire rig is rewritten to that configuration.

Activation is deliberately not optimistic. Because switching rewrites every zone wholesale and can fail validation on the daemon side, the picker does not flip to the new scene until the daemon confirms the switch. A spinner marks the row being switched to, and the panel closes once the switch settles. If activation fails, a toast says so and the picker stays on the scene that is genuinely active.

Switching scenes also resets Studio's undo history, because the steps it holds belong to the previous scene.

## Rename a scene

Choose **Rename**, edit the name, and click **Save name** or press Enter. **Cancel** or **Escape** backs out. Renaming to the same name, or to an empty name, changes nothing.

Rename acts on the **active** scene, so it appears only when the active scene is a saved one, never for Default. If another client switches scenes while the rename form is open, the form stands down rather than renaming whatever became active.

{% <callout type="warning"> %}
Renaming uses the same whole-document replacement contract as every stored-scene edit. The UI first reads the complete scene, changes only its name, then sends the document back with its revision as `If-Match`. If another client changes the scene before the rename lands, the daemon rejects the stale write instead of overwriting it.
{% </callout> %}

## Delete a scene

Choose **Delete**. Like rename, delete acts on the active saved scene and never appears for Default. The panel asks you to confirm, naming the scene and warning that its zones and layers go with it and that this cannot be undone. Choose **Delete scene** to remove it, or **Keep scene** to back out. A toast confirms what was deleted.

## Scenes and the target zone

The active scene decides where quick-applied effects land. One zone of it is the app's **target zone**:

- In Studio, it is the LED zone whose tab you have open.
- On the Dashboard and the Effects page, the preview's compact zone tabs choose it. They appear only in a multi-zone scene.

Applying an effect from the Effects page, a favorite, or the command palette lands in the target zone, and the command palette names that zone in a multi-zone scene. Choosing a zone anywhere moves the preview, the controls, the presets, and the target together, and Studio opens on the same zone next time.

The target is always one LED zone; there is no "every zone" target. A screen is never a target, and a target that points at a zone the active scene no longer has falls back to Main, so a quick-apply always has a defined destination. To give your whole rig one look, put it in one zone.

## Where scenes live in the bigger picture

{% <mermaid> %}
graph TD
    S[Scene, one active, whole-rig] --> Z1[Zone]
    S --> Z2[Zone]
    Z1 --> L1[Layers]
    Z1 --> LO1[Layout]
    Z2 --> L2[Layers]
    Z2 --> LO2[Layout]
{% </mermaid> %}

For the engine and concurrency story behind scene and zone mutations, see [Studio architecture](@/studio/architecture.md) and [Zone API and concurrency](@/studio/zone-api-and-concurrency.md). For the REST surface itself, see the [API reference](@/api/rest.md).
