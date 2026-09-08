---
name: rig-setup
description: >-
  Set up a Hypercolor spatial layout and scene for a PC build from scratch: research
  the case's real dimensions, inventory the connected controllers and devices, interview
  the owner about what hangs on each port and where it sits, generate the attachment
  bindings, layout, and scene with the bundled generator, then dial the result in on the
  lit hardware until every fan, strip, strimer, LCD, and RAM stick animates in the right
  place and direction. Use this whenever someone wants their case, rig, PC, or build
  "mapped", "laid out", "set up", "onboarded", or "auto-configured" in Hypercolor, asks
  why a device animates backwards or upside down, wants to add a new controller or case
  to an existing layout, or mentions a case model (O11D, Hyte Y70, NZXT H9, Lancool,
  Torrent), a hub (Nollie, Uni Hub, iCUE LINK, L-Connect), or Strimer cables together
  with layout or positioning. Also use it when a device is missing from Hypercolor:
  "unsupported device", "device not found", "not detected", "nothing drives my X",
  anything about OpenRGB (install it, bridge a device through it, "OpenRGB sees it but
  Hypercolor doesn't"), or a request to "request support" for hardware. The coverage
  phase decides native driver, OpenRGB bridge, or a prefilled support request per device.
  Works with the REST API, the MCP tools, and the `hypercolor` CLI; it never needs the
  source tree.
---

# Rig setup

You are about to turn a physical PC into a Hypercolor layout the owner can trust: every
LED sits where the eye sees it through the glass, animations sweep across the case in
one direction, and displays sit on the fans they belong to. Two files carry the work and
one script turns them into daemon state:

- a **case spec** (`references/cases/<case>.json`) describes the chassis in millimetres,
  once per case model, reusable by anyone with that case;
- a **rig spec** (the owner's file) says which controller slot drives what, where each
  part sits, and the wiring facts learned on the hardware;
- `scripts/gen_layout.py plan|apply --case … --rig …` validates bindings against the
  running daemon, writes a preview, and on `apply` creates or updates the attachment
  profiles, the layout, and a scene. It is idempotent, so the whole dial-in loop is
  "edit rig, apply, look".

`scripts/coverage.py` (who drives each device) and `scripts/request_support.py` (an
unclaimed device becomes a prefilled support request) serve the coverage phase.

The daemon stays the authority on template geometry: the generator asks it for the
suggested zones of each binding and only places them. Read `references/daemon-api.md`
for the surfaces and their sharp edges before the first write.

## Phase 0: preconditions

A daemon on `localhost:9420` with the devices connected. Check with the `get_status` and
`get_devices` MCP tools, `hypercolor status` / `hypercolor devices list`, or
`curl localhost:9420/api/v1/devices`. A native device in state `known` rather than
`connected` cannot be placed meaningfully; sort that out first (pairing, power, USB) or
leave it out and say so. Devices reached through the OpenRGB bridge are the exception:
they sit at `known` until the active layout targets them, by design, and identify still
flashes them. Python 3 is the only tool the scripts need; `rsvg-convert` makes a PNG of
the preview when present.

## Phase 1: inventory

Build the picture before asking the owner anything, so your questions are specific:

1. `GET /devices`, then `GET /devices/{id}` for each connected device: note
   `layout_device_id`, and every segment's name, LED count, and topology hint.
2. `GET /devices/{id}/attachments` for each hub or controller: its slots, their LED
   windows, current bindings. Slots are the things the owner will describe ("channel 3
   is the top fans").
3. `GET /attachments/templates?limit=200`: the component catalog. Match the owner's
   hardware to template ids (fan model and LED count, strimer variant, case strip).
4. `GET /layouts` and `GET /scenes`: what already exists, what is active, and whether an
   older layout carries clues (a previous mapping of the same controller tells you fan
   templates and chain orders the owner already validated).

Keep a scratch table: device → segments → slot → what the owner says is attached.

## Phase 1b: coverage

Inventory shows what the daemon drives. Coverage shows what it does not, and that gap
alone decides whether OpenRGB enters the picture. Run `python3 scripts/coverage.py`
(or `hypercolor devices coverage` and `hypercolor devices unclaimed`, or
`GET /devices/coverage` and `GET /devices/unclaimed`; the script falls back to the host's
USB list against `GET /drivers` on daemons without those routes). Every physical device
lands in one group: native, bridge, unclaimed, or conflict.

The rule is native first. Hypercolor drives everything it has a driver for, the bridge
drives only what native cannot, and OpenRGB gets installed only when an unclaimed device
needs it. Never treat OpenRGB as a prerequisite, and never run its detection while a
native SMBus driver is live (two probes on one bus corrupt DRAM LED counts).

For each unclaimed device, take the first row that fits:

1. **A native driver knows it but is disabled** (`claimable_by` names it). Offer to
   enable: `hypercolor config set drivers.<id>.enabled true`, then
   `hypercolor devices discover`. Native output, no OpenRGB.
2. **OpenRGB covers it.** Climb the guided ladder in `references/coverage.md`:
   `hypercolor openrgb hints` → install → permissions → `hypercolor openrgb partition` →
   `hypercolor config set drivers.openrgb.enabled true` plus the ownership mode →
   `hypercolor openrgb start` → `hypercolor devices discover --target openrgb` → size hub
   zones with `hypercolor openrgb resize` and copy the sizes into the rig's
   `bridge.zone_sizes`. Each rung has a check; stop at the first one that fails.
3. **Neither.** `python3 scripts/request_support.py --vid-pid VVVV:PPPP` builds the
   prefilled device-support issue (or files it with `gh`) and points at an existing
   issue instead of filing twice. Tell the owner, then continue: a layout that covers
   what is drivable today is a finished layout.

Conflict rows mean both stacks reached the same silicon. Native keeps driving and the
bridge route is output-disabled with a `disabled_reason` naming the owner. Leave it,
unless the owner wants OpenRGB for that one device, in which case they hand it over by
disabling it natively (`PUT /devices/{id}` with `{"enabled": false}`); native never
yields on its own. Bridged devices join the scratch table like any other controller,
with their `openrgb:` layout id.

## Phase 2: interview

Ask in batches grouped by physical location, offer the default you would pick, and
accept "I don't know" as an answer that becomes an `unverified` note rather than a
blocker. The owner is looking at the case; you are not.

**Case and orientation**

- Exact case model and variant (RGB edition, XL, Mini). Check `references/cases/` first;
  say whether a template exists.
- Standard or reversed (inverted) build? Reversed puts the glass on the right and the
  motherboard upside down (I/O at the bottom rear, GPU up top). The owner usually knows
  this as "inverted mode".
- Motherboard form factor. ATX defaults are fine unless they say otherwise.

**Per controller slot** (walk the slots you inventoried)

- What is plugged in: fan model and how many, strip and its LED count, strimer variant,
  backplate, light bar. Map each to a template; when the count is unknown, pick the
  template a previous layout used or the vendor default, and flag it.
- Where it sits: top row, bottom row, side column, rear, on the board (which feature),
  along a case edge.
- Chain order if they know it (which fan is first from the controller). Default to
  rear-to-front for rows and top-to-bottom for columns and let the dial-in fix it.

**Everything without slots**

- LCD fans: which receiver is which fan (usually unknown; assume list order).
- AIO pump cap and its LCD: sit them on the CPU socket.
- RAM: which DIMM slots are populated.
- Onboard accents, GPU bars, motherboard strimers: which feature they attach to.

**Taste**

- Canvas: default to the case's own aspect (`depth:height`, e.g. 640×630 for 478×471 mm)
  so one canvas pixel is roughly a millimetre. Say why.
- Starting effect for the scene: something directional (Color Wave horizontal) makes the
  dial-in loop legible; they can change it afterwards.

## Phase 3: case research

If no case spec exists, build one from the manufacturer's spec page (fetch it; the URL
goes into `sources`). You need the envelope, fan positions and sizes per mount, radiator
clearances, and any built-in strips. `references/case-spec.md` explains the two
coordinate frames and how to derive mount centres from those numbers. Hedge every
guessed dimension in the spec's notes, not in the chat: a spec with honest notes gets
corrected by the next user; a spec that looks authoritative does not.

Never encode the viewing side or the board flip in a case spec. The rig's `view`
handles both.

## Phase 4: generate and preview

Write the rig spec (`references/rig-spec.md`; copy the worked example and edit), then:

```bash
python3 scripts/gen_layout.py plan --case references/cases/<case>.json --rig <rig>.json --out <dir>
```

`plan` creates any custom templates the rig declares (additive), validates every binding
against the daemon, and writes `layout.json` plus `preview.svg`/`preview.png`. Look at
the preview before applying: the side column should be full rings, rows and the rear fan
should be edge-on bars, the board outline should span the right region with the pump,
DIMMs, and headers where the owner described them, strips should hug the frame. Fix
geometry in the specs, not by hand-editing `layout.json`.

Sanity checks worth running on `layout.json`: every position and size inside `[0, 1]`,
no two attachment zones on the same slot with overlapping LED windows, LED totals per
slot at or under the slot capacity.

Bridged hubs bind fans through their generic per-channel slots exactly like native ones.
Strimers through a hub are one zone per row, and the strimer templates cannot bind to a
20-LED zone, so they go in a `bridge_rows` block (`references/rig-spec.md`) that expands
to one raw strip per row; `references/rigs/o11d-evo-rgb-reversed-bridge-example.json` is
the shipped example's bridged twin. `plan` warns about any bridged zone still at 0 LEDs,
because frames into an unsized zone succeed and light nothing. `plan --offline` (with a
saved template catalog) checks specs and geometry when no daemon is reachable.

## Phase 5: apply and activate

```bash
python3 scripts/gen_layout.py apply --case … --rig … --out <dir>
```

This writes the attachment profiles, creates or updates the layout and the scene (both
keyed by the rig's `name`), and re-applies the layout if it is already live. Activation
swaps the whole rig's lighting, so do it when the owner is ready to look: the
`activate_scene` MCP tool, `hypercolor scenes activate <name>`, or
`POST /scenes/{id}/activate`. The previous scene stays available to switch back to.

Expect the daemon to auto-place devices the layout does not mention (keyboards, desk
strips) at default positions once the layout applies. Tell the owner; it is not a bug.

## Phase 6: dial in

Now the hardware teaches you the wiring facts. Follow `references/dial-in.md`: walk the
mounts in physical order, ask for observations rather than diagnoses, change one flag per
group per round, `apply`, look again. The flag table in `references/rig-spec.md` maps
"the top fans run backwards" or "the strimer is upside down" onto `chain`, `rotate`,
`mirror`, or `mirror_y`.

Before blaming the mapping for a dark or wrong-coloured output, flash it through
`POST /devices/{id}/attachments/{slot}/identify` with an explicit colour. Identify skips
the layout, so a blue flash that comes out blue proves the device path and points you at
the zone position or the effect; no flash points at seating or the driver.

Rows of fans that face the same way share chain order and rotation; once the bottom row
is right, copy it to the top row. A 180° rotation reverses both axes, so if "upside
down" survives a `rotate: π`, revert it and flip one axis instead.

## Phase 7: hand off

When the owner says it looks right:

- Write dated entries into the rig spec's `verified` list for each fact the hardware
  confirmed, and leave the honest `unverified` list for what you assumed.
- Give them the rig spec path and the exact `apply` command so they can re-run it after
  a controller swap or a daemon reinstall. With hubs on the bridge, `apply` also restores
  the hub's zone sizes from `bridge.zone_sizes`, which OpenRGB itself never saves.
- If a device went the support-request route, leave them the issue link and the words
  "re-run coverage after the next Hypercolor update".
- If the case spec was new, offer to add it to `references/cases/` so the next owner of
  that case starts from a finished template.
- Mention the follow-ups that live outside the layout: the effect palette if one output
  sits in a corner the effect tints differently, output brightness if the flash looked
  dim, and any hardware seating rule you discovered (narrow ribbons in wide sockets).

## Why the loop is shaped this way

This skill is the setup experience Spec 70 (`docs/specs/70-agent-rig-setup.md`) describes,
delivered as data and a script ahead of the daemon-side rig template registry.

Geometry is knowable from a spec sheet; wiring is not. Fan chain order, ring winding,
which strip of a strimer is row 0, and which LCD receiver is which fan can only be read
off lit hardware. Separating the case spec (knowable, reusable) from the rig spec
(personal, empirical) means the expensive research is done once per case model and the
dial-in is a handful of one-flag edits. The identify flash matters because a wrong colour
or a dark output has three unrelated causes (effect, mapping, hardware) that look
identical from the chair; identify collapses them to one in six seconds.

Coverage sits between inventory and interview because ownership has to be settled before
anyone is asked where a device lives: a device nobody drives has no slot to describe, and
a device two stacks fight over lights unpredictably wherever it is placed.
