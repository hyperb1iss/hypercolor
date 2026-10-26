# Studio Zone Tabs: One Scene Vocabulary, One Zone at a Time

**Reviewed:** 2026-10-25 against `main` at `697e78597`, with the live daemon
running the "Studio Session" scene (Default zone with 21 devices, ROLI and
Push 2 custom zones, five Screens, two unassigned devices).
**Method:** live walkthrough of Studio, Effects, and Dashboard with
`agent-browser`; a read-only audit of every scene picker, zone picker, search
field, and status surface in `hypercolor-ui`; a decisive read of where device
positions are stored; persona walkthroughs grounded in
[06-personas-workflows.md](06-personas-workflows.md); and two research passes
(RGB competitors, then pro lighting, creative tools, and tabbed-editor UX
guidance).
**Status:** Accepted 2026-10-25. The owner approved the whole design and chose
D1 option A, live commit (D2), and "Main" for new primary zones (D3); section 10
records those calls. Implementation follows the waves in section 11.
**Builds on:** [Spec 64](../specs/64-multi-zone-scenes.md) (multi-zone
scenes), [Spec 65](../specs/65-studio-composition-ui.md) (Studio composition
UI), [Spec 78](../specs/78-api-resource-model.md) (the `/scene` resource
model), and [Plan 55](../archive/55-studio-zone-unification-plan.md) (the
Scene/Zone vocabulary lock).

**Verdict: the foundation is right and the presentation is wrong.** The data
model already treats every zone as an independent canvas with its own layer
stack, and the Stage already renders one zone at a time. Studio then buries
that independence in a single accordion that stacks every zone's device list
on top of each other, so selecting the third zone means scrolling past
twenty-one devices from the first. Around Studio, the same concepts (the active
scene, the target zone) are picked through five different components that
disagree with each other. The fix is a tab per zone, a device pane that is
about the active tab, one scene picker, one zone selector, and a round of
deletions.

---

## Contents

1. [The Naming Question](#1-the-naming-question)
2. [What Is Wrong Today](#2-what-is-wrong-today)
3. [Personas and Jobs](#3-personas-and-jobs)
4. [What Other Tools Do](#4-what-other-tools-do)
5. [Design Principles](#5-design-principles)
6. [The Studio Redesign](#6-the-studio-redesign)
7. [One Scene Picker, One Zone Selector](#7-one-scene-picker-one-zone-selector)
8. [Dashboard Header](#8-dashboard-header)
9. [What Gets Deleted](#9-what-gets-deleted)
10. [Decisions for the Owner](#10-decisions-for-the-owner)
11. [Delivery Waves](#11-delivery-waves)
12. [Verification](#12-verification)

---

## 1. The Naming Question

**Keep "Scene". Do not rename it to "World".** The instinct behind "World" is
detecting something real, but it points at a different object than the scene.

### 1.1 Why "Scene" is the right word for the switchable whole

The object at the top is the thing a user switches between: "Movie Night",
"Studio Session", "Gaming". Every tool our users already know calls that a
scene:

- **OBS** switches scenes live inside a scene collection.
- **Ableton Live** calls a horizontal row a scene, and launching it fires every
  track at once. That is exactly "every zone changes together".
- **iCUE, MadMapper, Home Assistant, Stream Deck, and Hue** all expose scenes as
  the thing an automation activates.
- Our own wire surface already speaks it: `/api/v1/scenes`, the MCP tools
  `activate_scene`, `create_scene`, and `list_scenes`, the Home Assistant
  integration's scene entities, the Python client, and the CLI.

The switch test settles it. "Switch to the Movie Night scene" reads naturally.
"Switch to the Movie Night world" reads like loading a game save.

### 1.2 Why "World" points the wrong way

- **In grandMA3 a World is a filter, not a configuration.** "Fixtures and
  attributes not in the active world are removed in some windows and cannot be
  used in programmer actions"
  ([MA docs](https://help.malighting.com/grandMA3/2.3/HTML/worldfilter.html)).
  It hides everything outside a subset. The thing in Hypercolor that behaves
  like a grandMA World is a zone tab, which is the opposite end of the
  hierarchy.
- **In Unreal a World is the one container you load**, not a preset you flip
  between several times an evening.
- **In our own code "world" is already a coordinate space**: the layout
  geometry tests transform local coordinates to world coordinates.
- **Renaming costs five clients and buys no capability.** The REST routes, MCP
  tools, Home Assistant entities, Python client, and CLI would all break for a
  word that reads worse in the sentence users actually say.

### 1.3 What the "World" instinct is actually detecting

There is a layer underneath scenes, and today it leaks into them. Call it the
**rig**: the physical hardware that stays the same no matter which scene runs.

- **Devices and their components are rig facts.** A Strimer is plugged into
  channel 4 in every scene. Components are bound on the Devices page and are
  never scene state.
- **Physical positions are sometimes rig facts and sometimes scene facts,
  depending on a mode the user cannot see.** A single-zone scene's primary
  zone "follows the active layout": a Studio save writes through to the shared
  named layout (`set_zone_layout_workflow` mirrors primary saves when
  `primary_zone_follows_active_layout` holds, in
  `crates/hypercolor-daemon/src/domain/scene_tree.rs`). The moment the scene
  gains a custom zone, every zone keeps a private, scene-local layout
  (`upsert_primary_zone` in `crates/hypercolor-core/src/scene/mod.rs` stops
  taking the full-scope layout once custom LED zones exist). Moving a fan then
  moves it in this scene only.
- **Scene names already carry rig variants.** The live library holds "O11D EVO
  RGB (inverted)" and "O11D EVO RGB (OpenRGB bridge)", which describe a case
  orientation and a driver path, next to "Movie Night", which describes a look.
  These came from the profile fold (Spec 78 §3.2 gave scenes a `layout_id` so
  profiles could become scenes losslessly), and they make "scene" feel like it
  means the whole universe.

### 1.4 The hierarchy, stated honestly

The user's framing, "a scene has zones, each zone has devices, devices have
components", is close but nests one thing wrongly. **Zones do not own devices;
zones claim outputs.** A device is a rig object, and one device can split its
outputs across zones (the Push 2's pads run in the Push 2 zone while its white
buttons stay in the Default zone, live, today).

```
RIG (physical, one per machine, scene-independent)
  Device ──▶ outputs (channels, segments) ──▶ components (fans, strips)
  Screen devices (LCDs)
  Physical positions (the active named layout)

SCENE (a switchable look over the whole rig)
  Zone ──▶ claims some rig outputs
       ──▶ its own canvas: where those outputs sit on it
       ──▶ its own layer stack
  Screen surface ──▶ one LCD's layer stack
  Unassigned policy ──▶ what outputs no zone claims do (off, hold, follow a zone)
```

This needs **no new UI noun.** The Devices page already is the rig. What it
needs is for Studio to stop pretending device cards are children of zones (show
which outputs a zone claims, and say so when the rest of a device lives
elsewhere) and for position edits to behave predictably. Decision D1 in section
10 picks how predictable.

---

## 2. What Is Wrong Today

Each row was confirmed on the live UI or in the source.

| Area | What happens | Evidence |
| ---- | ------------ | -------- |
| Zone presentation | One accordion holds every zone, its device cards, their channel rows, Unassigned, New zone, and Screens. Selecting ROLI scrolls past 21 Default-zone devices and their expanded channels. | `pages/studio/zone_tree.rs:56-285`; live screenshot |
| Device search | A full-width page header search bar filters a 280px pane. It matches device names only and leaves empty zones and full device counts in place. | `pages/studio/mod.rs:410-414`, `zone_tree.rs:148-192` |
| Drag and drop | None between the pane, the canvas, and zones. The only drag is moving boxes inside the canvas. The legacy `LayoutPalette` sets drag data nothing accepts and is never mounted, yet the canvas empty state still says "Drag devices onto the canvas". | `layout_builder.rs:427`, `:447-453`; `layout_palette/devices.rs:196-207` |
| New zone members | A device added to a new zone lands at a default-sized cascade position. In the live ROLI zone the two ROLI devices sit stacked in the top-left corner of a canvas the effect fills, so the effect barely touches them. | live ROLI screenshot; `reset_device_zone_placement` |
| Canvas legibility | The Default zone canvas is a soup of overlapping boxes and labels; small devices under large strips are hard to click. | live screenshot |
| Commit model | Layout edits are staged behind Save and Revert. Membership, layers, and controls apply live. Two commit models on one screen. | `stage.rs:351-414` |
| Scene pickers | The Dashboard uses `SceneSwitcherMenu` over shared `ScenesContext` (Default row, lock icon, switching spinner, no create or delete). Studio uses a `SilkSelect` over a private `list_scenes` resource (no Default row once a saved scene is active, so Studio cannot return to Default; no lock; no spinner; separate "+ New" button and an actions menu that clips off the right edge). The switcher's doc comment claims both pages use it. | `components/scene_switcher.rs:1-2`, `:65-243`; `pages/studio/scene_selector.rs:36-100` |
| Zone pickers | The Effects page alone has three zone selectors: the "Apply to" dropdown (always rendered, even with one zone), the preview chips, and the controls tabs. The tabs write only `focused_zone`, so the "Apply to" label can name a different zone than the one being edited. Only the dropdown offers "All zones". | `pages/effects.rs:67-134`, `:538`; `effects/zone_controls.rs:151-222`; `zone_now_playing.rs:186-278` |
| Two target signals | `ZonesContext.focused_zone` and `EffectsContext.apply_target` mean the same thing and are kept in step by two call sites. | `zones.rs:41-65`; `apply_target.rs`; `app.rs:203-206` |
| Default zone naming | Four different rules decide when a zone is shown as "Default zone". | `effects.rs:87-93`, `zones/surface.rs:150-168`, `zone_add_device.rs:215-224`, `devices.rs:124` |
| "Per-zone favorites" | Favorites are global (keyed by effect id in the API, the store, and the UI). The Dashboard panel's "→ Default zone" chip only names where a click will apply, but it reads as scoping. | `dashboard/charts.rs:268-353`; `api/library.rs:38-60` |
| Dashboard header | Five status pills in five hues (green, cyan, coral, purple, cyan or yellow) decorate counters, against Luminary §4. The pill glow builds `var(--color-cyan)aa`, which is invalid CSS, so the intended glow never renders. The counters come from a one-shot `/status` fetch. "Effects: 60" is catalog size, not status. | `dashboard/header.rs:23-180`; `components/status_pill.rs:24` |
| Accent drift in Studio | "+ New zone" is a cyan button; the "+" add buttons are success green. | `zone_controls.rs:263-273`; `device_card.rs:566`, `:589` |

---

## 3. Personas and Jobs

The personas come from [06-personas-workflows.md](06-personas-workflows.md);
the sixth stands in for the network-heavy orchestrator without room
vocabulary.

### 3.1 Jake, the set-and-forget gamer

**Job:** see the rig's devices, pick a cool effect, leave. Ten minutes, total, ever.
Jake will never make a zone.

**Today:** the Effects page shows an "Apply to: Default zone" dropdown with one
option. Studio's header carries a scene selector, a New button, a kebab, and a
page-wide search for a list Jake never needs to search. Unassigned hardware is
called "Available" in a single-zone scene and "Unassigned" in a multi-zone one.

**The redesign gives Jake:** a single "Main" tab with a quiet "+" after it, the
same pattern as a spreadsheet's single sheet tab. No zone UI anywhere else in
the app until a second zone exists.

### 3.2 Robin, the Windows refugee

**Job:** recreate the case layout once, physically accurate, then flip between
four profiles (Daily Driver, Gaming, Movie, Off).

**Today:** where a position edit lands depends on whether the scene has custom
zones (section 1.3). Devices dropped into a new zone forget where they
physically are. Two scene pickers behave differently, and the one in Studio
cannot get back to Default.

**The redesign gives Robin:** new zone members placed at their physical position
and fitted to the zone; one scene picker that behaves like the profile list he
remembers; "Duplicate current scene" as the default way to make a new one, so
the Gaming scene starts from Daily Driver's layout instead of from zero.

### 3.3 Luna, the streamer

**Job:** six scenes driven by OBS automation (just chatting, gaming, BRB,
starting soon). The same three areas in every scene (camera wall, desk, PC)
with a different look per scene.

**Today:** every scene owns its zones, so the area split must be rebuilt per
scene unless Luna knows to snapshot and edit. The scene Luna sees in Studio and
the scene the Dashboard shows are picked through different components.

**The redesign gives Luna:** duplicate-to-create keeps the zone split,
tabs make each area's look editable in isolation, and the scene picker sits in
the same header slot with the same behavior on every page.

### 3.4 Sam, the audio producer

**Job:** Push 2 pads reactive in one zone, ROLI blocks in another, the Push 2's
white buttons following the main look. Output-level splits are the whole point.

**Today:** the Push 2 card appears under two zones with no cue that it is split.
The device menu moves the whole device; per-channel move icons were removed to
keep the card to two trailing icons.

**The redesign gives Sam:** channel rows that drag on their own, a per-channel
"Move to" inside the existing kebab (still two icons), and a "+2 outputs in
Push 2" hint on a split device.

### 3.5 Bliss and Yuki, the power users

**Job:** 30-plus devices, 600-plus LEDs on the Nollie alone, several zones,
precise canvases, fast iteration.

**Today:** the accordion, the label soup on the canvas, a search bar that spans
the page but filters a sidebar, no drag and drop, and a Save button that loses
work if forgotten.

**The redesign gives power users:** tabs as workspaces, a pane about the active zone,
keyboard tab switching, drag everywhere with menu equivalents, smallest-box hit
testing and decluttered labels, and live commit with undo.

### 3.6 The network orchestrator

**Job:** WLED, Hue, and Nanoleaf hardware appears and disappears; identify each
new device and put it where it belongs.

**Today:** Unassigned sits at the bottom of the accordion below every zone, and
its policy (off, hold, follow) is only reachable by selecting a pseudo-zone that
swaps the whole Stage for one select.

**The redesign gives the orchestrator:** an always-visible Unassigned section in the pane
with identify and one-tap add, and the policy as an inline control on that
section's header.

### 3.7 The critical first multi-zone moment

The most common reason anyone makes a zone is "I want my keyboard to do its own
thing". That journey decides whether multi-zone feels magic or fiddly.

| Step | Today | Redesign |
| ---- | ----- | -------- |
| 1 | Scroll to the bottom of the zone tree, click "+ New zone", name it | Open the keyboard card's menu, choose "Move to ▸ New zone" |
| 2 | Find the keyboard among 21 Default-zone cards, open its menu, "Move to" the new zone | A tab named after the keyboard opens with the keyboard fitted to the canvas and the name selected for renaming |
| 3 | Click the new zone (the accordion scrolls) | Pick an effect from the Stage chip |
| 4 | Find the keyboard at default size in the top-left corner, resize it to fill the canvas | (done) |
| 5 | Open the effect chip, add a layer, pick an effect | |

---

## 4. What Other Tools Do

**Nobody in the PC RGB space ships real multi-zone composition, and the ones
who come closest prove the shape we want.** SignalRGB, the product most of our
switchers come from, runs "a single canvas that runs one effect across all your
devices at once"; staff confirmed in November 2025 that simultaneous effects
are not possible with their renderer and in September 2026 that the change is
"much bigger" than it sounds
([forum](https://forum.signalrgb.com/t/why-cant-we-assign-specific-effects-to-individual-components-independent-from-a-canvas/10115)).
Multi-zone is Hypercolor's differentiator, so its UX has to be the best part of
the product, not the most fiddly.

### 4.1 RGB products

| Product | Switchable whole | Partitions | What we take | What we avoid |
| ------- | ---------------- | ---------- | ------------ | ------------- |
| SignalRGB | No preset noun; Layouts (positions) plus playlists and macros | None: one canvas | Components on controller channels (we have them) | Every device stacked at canvas center on first run, placed by a hide-all, unhide-one ritual |
| Corsair iCUE 5 | Profile (lighting plus fans plus actions) | Per-device layer stacks; Lighting Link buried inside one device's module; Murals canvas | Multiple named layouts | Non-lighting state in the preset (importing a lighting profile spins fans to full); Murals layouts that reset and are missing from exports |
| Razer Chroma | Chroma Profile in a dropdown | Effect layers scoped by LED selection on one canvas | Identify on right-click; a names toggle | Devices present in Synapse but missing from Studio; canvas positions forgotten between sessions |
| Artemis | Profiles in categories, composited by category order | Folders and layers with LED selections | Editing a profile suspends the others (a focus mode); Identify; device list ordered by z-index with bring-forward | "Overloaded and overly complicated" |
| OpenRGB | Profile (full snapshot) | Effects plugin: one tab per effect, a device checklist each; assigning a zone silently removes it from other effects | One tab per independent partition, with exclusive ownership (our model exactly) | Silent stealing; spatial layout in a separate plugin from effect assignment |
| Lian Li L-Connect 3 | Per-controller settings | Fan clusters with editable order for merged flow | Click a channel to flash its hardware | Motherboard sync that disables software control entirely |
| Philips Hue | Scenes owned by one room or zone | Rooms (exclusive), zones (overlapping), entertainment areas (positioned, max 10 lights) | Drag lights to positions with a "Test area" button | Presets owned by one group, so a whole-home look means saving the same scene in every room |
| WLED, LedFx | Presets and playlists; LedFx scenes with a per-virtual action | Segments; LedFx virtuals spanning devices | LedFx scenes greying out devices added later (our Unassigned policy) | Overlap with "undefined behaviour"; WLED editing several segments but showing only the first one's state |
| Home Assistant | Scenes | Areas, floors, labels | The only real device table: group-by, filter chips, opt-in multi-select | |

Three lessons drive section 6:

- **Exclusive ownership must be visible.** OpenRGB already has our partition
  semantics and gets them wrong by moving devices silently. Studio shows where
  every output lives and makes every move explicit, undoable, and announced.
- **Placement and assignment belong in one tool.** OpenRGB splits them across
  two plugins and iCUE buries cross-device sync inside one device's page. In
  Studio, the canvas, the device pane, and the zone tabs are one surface.
- **The rig must survive.** Every vendor that loses positions (Murals, Razer)
  draws complaints. Positions are the slowest thing to rebuild and the first
  thing users notice, which is why decision D1 exists.

**Considered and rejected:** ghosting other zones' devices on the canvas
(Artemis, iCUE). It is the closest thing to a combined view, and the owner ruled
combined views out. LedFx's per-zone "span or copy" mapping (one effect stretched
across all devices, or one copy per device) is a genuinely good idea and a
separate feature; it is not part of this redesign.

**A word collision to manage:** OpenRGB, iCUE, and Razer use "zone" for a
hardware region below the device, and Hue uses it for an overlapping group.
With the OpenRGB bridge in the product, Studio must never surface a vendor's raw
"zone"; device sub-units are always "outputs", "channels", or "segments", and
"zone" means only our exclusive partition.

### 4.2 Pro lighting and creative tools

| Tool | Whole switchable state | Partition | Lesson for us |
| ---- | ---------------------- | --------- | ------------- |
| grandMA3 | Show file; sequences of cues | Stages (physical), Groups, **Worlds** (visibility filters) | A World hides non-members everywhere. That is what a zone tab should feel like. |
| xLights | Show folder, sequences | Models, Model Groups, **Previews** with built-in Default, All Models, and **Unassigned** | The closest analog to zone canvases, including an explicit Unassigned bucket. |
| Resolume Arena | Composition, decks | Advanced Output screens and slices, saved "independent of compositions" | Output plumbing (the rig) lives apart from content (the scene). |
| OBS | Scene collection | Scenes with their own canvases and sources | Users still confuse profile and collection; one noun per level matters. |
| Ableton Live | Live Set | A **scene** is a row across every track | "Scene" as "everything changes together" is mainstream. |
| Excel, Blender workspaces | Workbook, file | Sheet tabs, workspace tabs with "+" at the end | Document tabs for user-owned, persistent partitions. A single tab with "+" teaches the feature without clutter. |

### 4.3 Tabbed-editor guidance that shapes section 6

- **Zones are document tabs, not static tabs.** Fluent separates user-created,
  rearrangeable document tabs from fixed navigation tabs
  ([TabView](https://learn.microsoft.com/en-us/windows/apps/design/controls/tab-view)).
  The "+" sits after the strip, there is always an active tab, and the last one
  cannot be deleted (the Blender workspace rule).
- **Avoid overflow carousels.** Shrink tabs to a minimum width, then add an
  overflow menu ([NN/g](https://www.nngroup.com/articles/tabs-used-right/)).
  Labels of one or two words; the selected tab visually joined to its panel
  with at least two selection cues.
- **Spring-loaded tabs at 500 ms.** Chromium's tab hover transition is 500 ms;
  Atlassian expands collapsed tree items after a 500 ms hover; Apple's HIG gives
  dragging an event onto a Calendar segment as the canonical spring-load case
  ([HIG](https://developer.apple.com/design/human-interface-guidelines/drag-and-drop)).
- **WAI-ARIA tabs:** `tablist`/`tab`/`tabpanel`, roving tabindex, arrow keys,
  automatic activation when the panel swaps instantly. The "+" button lives
  outside the tablist ([APG](https://www.w3.org/WAI/ARIA/apg/patterns/tabs/)).
- **WCAG 2.2 SC 2.5.7** requires a single-pointer alternative for every drag;
  keyboard support alone does not satisfy it. A "Move to" menu does
  ([Understanding 2.5.7](https://www.w3.org/WAI/WCAG22/Understanding/dragging-movements.html)).
  Announce moves by name and location ("moved Huntsman V2 to Keyboard from
  Main") and return focus to the trigger
  ([Atlassian](https://atlassian.design/components/pragmatic-drag-and-drop/accessibility-guidelines)).
- **Deletes prefer undo over confirmation.** Confirm only when the zone has
  devices, name the consequence, and label buttons by outcome ("Delete zone",
  "Keep zone") ([NN/g](https://www.nngroup.com/articles/confirmation-dialog/)).
- **Scoped search defaults broad.** Search across every group in the pane,
  expand the groups with hits, and rank the active zone's hits first
  ([NN/g](https://www.nngroup.com/articles/scoped-search/)).

---

## 5. Design Principles

1. **A tab is a zone, and a zone is a world of its own.** The canvas, the
   device list's first section, the layer stack, and the now-playing chip all
   describe the active tab and nothing else. There is no combined view of every
   device at once, anywhere in Studio.
2. **One concept, one control, one place.** The active scene is picked with one
   component in the same header slot on every page that depends on it. The
   target zone is one signal with one selector component in two densities.
3. **Things live next to what they act on.** Search sits in the pane it filters.
   Zone settings live on the zone's tab. The Unassigned policy lives on the
   Unassigned section.
4. **Every drag has a menu twin.** Drag is the fast path; "Move to" is the
   guaranteed path.
5. **Progressive simplicity holds.** A single-zone user sees one tab and a "+",
   and nothing zone-related on any other page.
6. **Subtract before adding.** Each wave deletes the component it supersedes in
   the same change.

---

## 6. The Studio Redesign

### 6.1 Layout

The page header keeps its title row and turns its second row, where the
page-wide search used to be, into the zone tab strip. The device pane, Stage,
and composition slide-over keep their current roles.

```
┌ Studio ──────────────────────────────────────────── Scene  Studio Session ▾ ┐
│ ● Main   ● Keyboard   ● ROLI   ● Push 2  ┊  ▢ Screens 5   +                 │
├──────────────────────────┬──────────────────────────────────────────────────┤
│ ⌕ Search devices       / │ ◉ Bubble Garden ›                ↶  ↷   ⤢ Fit    │
│                          │                                                  │
│ MAIN · 19                │                                                  │
│  ▣ Corsair K70 Core   ⋯  │            the Main zone's canvas,               │
│  ▣ Nollie 32          ⋯  │            the Main zone's devices,              │
│    ├ Channel 1  SL120    │            nothing else                          │
│    └ Channel 4  ATX…     │                                                  │
│  ▣ Ableton Push 2     ⋯  │                                                  │
│    2 more outputs in     │                                                  │
│    Push 2 ›              │                                                  │
│  OFFLINE · 2             │                                                  │
│                          ├──────────────────────────────────────────────────┤
│ OTHER ZONES              │ selected output properties                       │
│  ▸ ● Keyboard        1   │                                                  │
│  ▸ ● ROLI            2   │                                                  │
│  ▸ ● Push 2          1   │                                                  │
│                          │                                                  │
│ UNASSIGNED · 2   Off ▾   │                                                  │
│  ▣ Dygma Defy      ⚡  +  │                                                  │
│  ▣ WLED Theater    ⚡  +  │                                                  │
└──────────────────────────┴──────────────────────────────────────────────────┘
```

### 6.2 The zone tab strip

**Contents of a tab.** A zone identity dot (the zone's color, identity only per
Luminary §4), the name, and one optional status glyph (a warning when a layer is
degraded or the zone has no devices; a paused glyph when the zone is disabled).
Nothing else. Device counts live in the pane; live previews live on the Stage.

**Selection.** The active tab carries the electric purple bar, primary text,
and the panel surface color so it reads as joined to the workspace below. Zone
colors never become chrome.

**Adding.** "+" sits after the strip, outside the tablist. It creates a zone,
opens its tab, and selects the name field for typing. The new zone's canvas
shows an empty state ("Drag devices here, or add them from Unassigned") and the
pane expands Unassigned and Other zones so the next move is obvious. Dropping a
device onto "+" creates a zone named after the device and moves the device into
it.

**Zone settings.** Double-click renames in place. Right-click, or the kebab
that appears on the active tab, opens: Rename, Color, Pause or Resume, Move
left, Move right, and Delete. This replaces the inline `ZoneControls`
disclosure and the cyan "+ New zone" button.

**Deleting.** The Main (primary) zone and the last remaining zone cannot be
deleted. An empty zone deletes immediately with an undo toast. A zone with
devices asks once: "Delete 'Keyboard'? 1 device becomes unassigned." with
"Delete zone" and "Keep zone", then offers undo. There is no × on tabs; a zone
holds authored work and is not a cheap browser tab.

**Reordering.** Drag a tab along the strip, or use Move left and Move right.
Tab order is the scene's zone order.

**Overflow.** Tabs shrink to a minimum width, then the tail collapses into a
"⌄ 3 more" menu. No horizontal carousel.

**Screens.** LCD surfaces are not zones in the UI vocabulary (Spec 65 §4), and
five of them as tabs would drown the zones. After a divider at the end of the
strip, one "Screens" tab appears whenever the scene has Screen surfaces. Inside
it, the pane lists the screens as cards with live thumbnails, the Stage shows
the selected screen's face and mounting, and the composition slide-over edits
that screen's layers. Identical device names ("Lian Li Uni Fan TL Wireless LCD"
three times) display with the per-device label the canvas already uses ("TL LCD
1", "TL LCD 2", "TL LCD 3").

**Keyboard and ARIA.** The strip is a `tablist` with roving tabindex. Arrow keys
move between tabs, Home and End jump, and activation is automatic because the
scene document is already local and the canvas swap is instant. `[` and `]`
cycle zones when focus is not in a text field.

**Single zone.** One "Main" tab and the "+". The strip stays, because a lone
tab with a plus is the clearest possible invitation to make a second one, and
it costs one row the old search bar already occupied.

### 6.3 The device pane

The pane answers one question: what is in this zone, and what could be?

**Search** sits at the top of the pane and keeps the `/` shortcut. It filters
every section of the pane (broad by default, per NN/g), auto-expands sections
with hits, shows hit counts ("3 of 19"), ranks the active zone's section first,
and matches device names, channel names, and component names. Escape clears it.

**Section 1: this zone.** The header names the zone and its device count and
sticks while scrolling. It cannot collapse. Device cards keep today's anatomy
(identity strip, name, LED count, transport, the two trailing icons: visibility
and the kebab) and today's sort (connected first, name, id). Offline devices
gather in an "Offline" subsection at the bottom, dimmed, with remove and forget
inside the kebab rather than as two extra icons.

A device whose outputs are split across zones shows only the outputs this zone
claims, plus a quiet "2 more outputs in Push 2 ›" line that expands that zone
in section 2.

**Section 2: other zones.** One collapsed row per other LED zone: identity dot,
name, device count. Expanding shows compact device rows that can be dragged out.
Rows are drop targets. Switching zones stays the job of the tabs, so a row
click expands and never navigates.

**Section 3: unassigned.** Expanded whenever it is non-empty. Each row has
identify (flash the hardware) and "+" (add to the active zone). The section
header carries the scene's Unassigned policy as a compact select ("Off",
"Hold last colors", "Follow Main"). This replaces the Unassigned pseudo-zone
that swapped the whole Stage for one select. One word everywhere:
"Unassigned", never "Available".

**What leaves the pane:** the per-zone "Add device" picker (superseded by
Unassigned's "+" and by dragging), the Screens section (now its own tab), the
New zone button (now the "+"), and the zone settings disclosure (now on the
tab).

### 6.4 Drag and drop, with a menu twin for every gesture

| Drag | Drop on | Result | Menu twin |
| ---- | ------- | ------ | --------- |
| Device card or channel row from any section | The canvas | Claim it for the active zone, placed at the drop point | "+" on Unassigned rows; "Move to ▸ this zone" |
| Device or channel | A zone tab | Move to that zone without switching tabs; placed per section 6.5 | "Move to ▸ <zone>" |
| Device or channel, held over a tab for 500 ms | (spring load) | Switch to that tab; keep dragging onto its canvas | none needed |
| Device or channel | "+" | New zone named after the device, device moved in, tab opened | "Move to ▸ New zone" |
| Device or channel | An Other zones row | Move to that zone | "Move to ▸ <zone>" |
| Device or channel | The Unassigned section | Unassign | "Remove from zone" |
| Output box on the canvas | A zone tab | Move that output to that zone | "Move to ▸ <zone>" in the properties panel |
| A zone tab | Elsewhere on the strip | Reorder zones | "Move left", "Move right" |

**Mechanics.** One pointer-based drag controller drives all of it, built on
the existing `pointer_gesture` machinery rather than HTML5 drag and drop, so
the pane and the canvas share one gesture model and touch works. Only targets
that will accept the drop highlight. Dragging a multi-output device shows a
count badge. The spring-load shows a fill ring during the hold and cancels when
the pointer leaves. Every move goes through `StudioHistory` (assignment edits
already do), so Ctrl+Z undoes a misdrop. An `aria-live` region announces each
move by name and location, and focus returns to the source row.

**No manual device reordering.** The earlier "order wild" complaint was about
raw layout order, which the deterministic sort already fixed. Device order has
no meaning for sampling, and persisting a hand order per zone per scene is
upkeep with no payoff. The overlap problem that reordering would have addressed
on the canvas is solved by hit testing instead (section 6.5).

### 6.5 The canvas

- **Only the active zone's outputs.** Already true; it stays true. No ghosted
  context from other zones.
- **Placement on assignment.** A device claimed by a zone lands where it sits
  in the rig's physical layout, mapped into the zone's canvas, instead of at a
  default-sized cascade. When a zone's first devices arrive through a tab drop,
  a "+" drop, or a menu, the zone fits them to the canvas so an effect covers
  them fully. Later arrivals keep their physical relation to what is already
  there.
- **Fit.** A Stage toolbar action scales and centers every member to fill the
  canvas while keeping their relative arrangement. One click turns the live
  ROLI zone from "two boxes in a corner" into "the effect covers both blocks".
- **Hit testing prefers the smallest box under the pointer**, with Alt-click
  cycling through the stack beneath it. Small devices under long strips become
  clickable without anyone managing z-order.
- **Labels declutter.** Names show on hover, on selection, and on boxes large
  enough to hold them. The Default zone canvas stops being a wall of text.
- **The empty state tells the truth.** "Drag devices here, or add them from
  Unassigned" replaces the current promise of a drag that does not exist.

### 6.6 Live commit with undo

Layout edits commit when a gesture ends (drag release, resize release, a
property field losing focus), exactly like membership, layers, and controls
already do. Drag-time feedback keeps using the `zone_layout_preview` WebSocket
command. Undo and redo cover layout, membership, and zone structure through one
history. "Saved", "Unsaved layout", Revert, and Save leave the Stage header.

Staged saves and tabs do not mix: either switching tabs silently drops work, or
it interrupts with a prompt. Live commit removes the question.

### 6.7 What stays

The composition slide-over over the Stage (nothing permanent competes with the
canvas), the device card anatomy and its two-icon cap, the rail-to-canvas hover
and selection sync, the resizable pane width, the narrow-screen drawer, and the
per-scene hidden outputs.

---

## 7. One Scene Picker, One Zone Selector

### 7.1 `ScenePicker`

One component replaces `SceneSwitcherMenu` and Studio's `SceneSelector`.

- **Data:** `ScenesContext` only. Studio's private `list_scenes` resource goes.
- **Trigger:** the "Scene" label, the scene name, a chevron, a lock glyph for
  snapshot-locked scenes, and a spinner while a switch is in flight.
- **Panel:** portaled with `SilkSelect`'s mechanics (keyboard navigation,
  dismissal, no clipping). Default first, always; saved scenes after it, with a
  check on the active one; then a divider and the actions: **New scene**
  (duplicate the current scene by default, or start blank), **Rename**, and
  **Delete**, the last two acting on the active saved scene and absent for
  Default.
- **Placement:** the right side of the page header's title row on every
  scene-dependent page: Dashboard, Effects, and Studio. Same component, same
  slot, same behavior. On the Effects page it takes the slot the "Apply to"
  dropdown vacates.

Duplicate-by-default matters more than it looks. It is how Robin's Gaming
scene inherits Daily Driver's layout and how Luna's six scenes share one zone
split without rebuilding it.

The command palette keeps finding scenes; its placeholder becomes "Search
effects and scenes...".

### 7.2 One target zone

- **One signal.** `ZonesContext.focused_zone` is the only record of which zone
  quick actions target. `ApplyTarget`, `EffectsContext.apply_target`, and the
  "All zones" apply path are deleted. Applying the same effect to several zones
  gives each its own unsynchronized canvas, which is almost never the intent;
  a user who wants one look across everything wants one zone.
- **One selector, two densities.** A shared `ZoneTabs` component renders the
  full strip in Studio and compact chips on the preview cabinet (Dashboard and
  Effects). The compact chips replace the Effects "Apply to" dropdown and the
  controls panel's own zone tabs. Selecting a zone anywhere moves the preview,
  the controls, the presets, and the apply target together, and the Studio tab
  strip opens on the same zone. Hidden entirely with one zone.
- **One name rule.** The four "Primary" relabel helpers collapse into one
  `zone_display_name`.
- **Favorites stay global.** The "→ Default zone" chip leaves the Favorites
  panel header. A favorite applies to the focused zone, which the compact zone
  chips on the same page already show.

**Shipped as (2026-10-25).** There is no single `ZoneTabs` component. What the
compact chips and the Studio strip share is the tab anatomy and keyboard model
in `components/zone_tabs.rs`: `ZoneDot` (the identity dot, also used by the
device pane, the Move to menu, and the sidebar rows), `ZoneLabel` (dot, name,
and one `TabStatus` glyph, with the status exposed as the tab's description
rather than its name), `tab_status` and `surface_status`, and `next_tab` with
`focus_tab_soon` (arrows, Home, End, and the brackets, selection following
focus). `CompactZoneTabs` renders the chips from those parts. The Studio strip
(`pages/studio/zone_strip.rs`) renders the same parts and adds its editing
affordances around them: rename in place, the zone menu, drag reorder,
overflow, the Screens tab, and "+".

---

## 8. Dashboard Header

The overview should read as one calm line of status plus the scene.

- **Title row:** "Dashboard", then `ScenePicker` and the layout menu on the
  right.
- **Status line:** `● Running · up 11h 40m · 23 devices · 60 fps`, in secondary
  text. One status dot carries health (success, warning, or error per Luminary
  status tokens); every other value is monochrome. "23 devices" links to the
  Devices page. The values ride the WebSocket like everything else instead of
  a one-shot `/status` fetch.
- **Removed:** the Effects count (catalog size is not status), the per-counter
  colors, the separate Scene pill (now the picker), and the invalid glow in
  `StatusPill`.
- **Neighbors:** the Performance cards drop their decorative colored dots;
  where a dot stays, it reports budget health in status colors. The Favorites
  panel drops its coral border, tri-color strip, gradient title, and coral count
  pill for the standard panel card.

---

## 9. What Gets Deleted

Each item leaves in the wave that supersedes it.

| Delete | Superseded by |
| ------ | ------------- |
| `components/layout_palette/` and the non-compact palette branch in `layout_builder.rs` (never mounted; drag data nothing accepts) | The pointer drag controller (§6.4) |
| `components/scene_switcher.rs` and `pages/studio/scene_selector.rs` | `ScenePicker` (§7.1) |
| `apply_target.rs`, `EffectsContext.apply_target`, the "All zones" apply path in `app.rs`, the Effects "Apply to" dropdown, and the controls panel zone tabs in `effects/zone_controls.rs` | `focused_zone` plus `ZoneTabs` (§7.2) |
| The Unassigned Stage view in `stage.rs` | The policy select on the Unassigned section (§6.3) |
| `zone_add_device.rs` (the per-zone "Add device" picker) | Unassigned "+", drag, and "Move to" |
| `ZoneControls` disclosure and the cyan "+ New zone" button | Tab context menu and "+" (§6.2) |
| The Studio page-header search bar | Pane search (§6.3) |
| Add-layer scopes "Selected surfaces", "All zones", and "Whole scene" (the rail multi-select they depend on disappears with tabs) | Add to the active tab; the Screens tab keeps "All screens" |
| Save, Revert, and the saved-state label in the Stage header | Live commit with undo (§6.6), if D2 is accepted |
| Three of the four "Primary" relabel helpers | `zone_display_name` |
| The Effects count pill, per-counter colors, and the `StatusPill` glow string | The status line (§8) |
| Stale comments: the switcher's "used by both" doc, the "identify only" card mode doc, "make default" in `zone_controls.rs`, the "multi-zone only" claim at `effects.rs:432-436`, the "manual reordering" note in `device_assignment.rs`, the orphaned `reset_device_zone_placement` doc comment that sits above `OutputPlacement` in `scene/mod.rs` | Accurate comments or none |

---

## 10. Decisions for the Owner

**D1. How predictable should positions be?** *(decided: A)*

- **A. Independent zone canvases, seeded from the rig.** Each zone keeps its own
  placements, as today, but members arrive at their physical position and the
  zone can Fit. Moving a device in one zone's canvas changes that zone in that
  scene only. This matches "it's all independent", costs placement hints on the
  existing assignment request plus the Fit action, and needs no migration.
- **B. Rig-wide positions with per-zone framing.** Physical position becomes one
  truth across every scene and zone; a zone stores only a framing (which part
  of the rig fills its canvas) plus optional per-device overrides for
  deliberately non-physical arrangements. Moving a fan anywhere moves it
  everywhere. This deletes the primary-zone mirroring special case but changes
  the scene model and needs a migration of every stored zone layout.

A delivers the visible win (zones that just work when created) now and leaves B
open if cross-scene drift shows up in dogfooding.

**D2. Live commit for layout edits?** *(decided: yes)* Removes Save and Revert
in favor of commit-on-gesture-end with undo (§6.6).

**D3. Name the primary zone "Main" in new scenes?** *(decided: yes)*
"Default zone" is an implementation word on a tab label. Existing scenes keep
their stored names.

---

## 11. Delivery Waves

Each wave is one atomic, revertable change set that deletes what it supersedes.

**W1. Consistency foundation.** `ScenePicker` on Dashboard, Effects, and
Studio; `focused_zone` as the single target with compact `ZoneTabs` on the
preview cabinet; delete `ApplyTarget`, the "Apply to" dropdown, the controls
zone tabs, and the favorites chip; the Dashboard status line; delete the dead
`LayoutPalette` and the stale comments. No Studio layout change yet, so this
can ship and soak on its own.

**W2. The tab strip.** The header's second row becomes the zone tab strip:
add, rename, color, pause, reorder, delete with undo, overflow, the Screens
tab, keyboard and ARIA. Studio's selection state becomes the active tab. Search
moves into the pane.

**W3. The device pane.** The three sections, the sticky active-zone header,
split-device hints, the inline Unassigned policy, the Offline subsection; delete
the per-zone Add device picker and the Unassigned Stage view.

**W4. Drag and drop.** The pointer drag controller: pane to canvas, pane to tab
with spring loading, canvas to tab, pane to Other zones and Unassigned, drop on
"+"; "Move to" menu twins including "New zone"; `aria-live` announcements.

**W5. Placement and canvas.** Physical seeding on assignment (placement hints), Fit,
smallest-box hit testing with Alt-click cycling, label decluttering, the honest
empty state.

**W6. Live commit.** Gated on D2: commit on gesture end, unified undo, Save and
Revert removed.

W1 is independent of everything else. W2 precedes W3 and W4. W5's daemon change
can run in parallel with W2 through W4. It turns out to need no daemon change
at all: `POST /scene/members/edit` already accepts an `assignment` with
`placements: Vec<MemberPlacementHint>`, "optional seeded geometry for newly
minted outputs, keyed by segment" (`crates/hypercolor-types/src/api/scene.rs`).
The UI computes each output's physical position from the rig layout and sends
it as a hint.

---

## 12. Verification

- **Automated:** contract tests for `zone_display_name`, the pane's section
  partitioning (including split devices and offline devices), search ranking,
  the drag controller's drop resolution table (§6.4), and Fit's transform.
  Contract tests for the rig-to-zone placement hint mapping.
- **Visual, with `agent-browser`, both themes:** each persona journey in
  section 3, run against the live rig. The section 3.7 keyboard journey must
  finish in two steps.
- **Keyboard only:** create a zone, move a device into it through menus, rename
  it, reorder it, delete it, and undo, without a pointer.
- **Touch:** drag a device onto a tab and onto a canvas on a tablet-width
  viewport.
- **Consistency sweep:** one scene picker component mounted three times, one
  zone selector component in two densities, zero remaining references to
  `ApplyTarget`. Shipped as (2026-10-25): one shared tab anatomy and key map
  used by both selectors, per the note in section 7.2.
