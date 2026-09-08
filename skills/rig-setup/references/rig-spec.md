# Rig spec format

A rig spec is one person's build: which controller slot drives what, where each thing
sits on the case, and the wiring facts (chain order, LED direction) that only the
hardware can tell you. It references a case spec by id and is the file you edit during
the dial-in loop. `references/rigs/o11d-evo-rgb-reversed-example.json` is a complete
worked example (three controllers, four LCDs, two AIO rings, DRAM, strips, strimers).

## Top level

```json
{
  "schema": "hypercolor-rig-spec/1",
  "name": "My Case Layout",             // layout AND scene name (idempotency key)
  "case": "lian-li-o11d-evo-rgb",        // must match the case spec id
  "view": "reversed",                    // or "standard"
  "canvas": { "width": 640, "height": 630 },   // match the case aspect
  "description": "...",
  "effect_id": "<effect uuid>",          // optional: first scene layer, Color Wave by default
  "custom_templates": [ ... ],           // optional: simple strips the catalog lacks
  "controllers": { ... },
  "raw_zones": [ ... ],
  "bridge_rows": [ ... ],                // optional: strimers through an OpenRGB-bridged hub
  "bridge": { "zone_sizes": { ... } },   // optional: hub zone sizes apply pushes into config
  "verified": [ ... ], "unverified": [ ... ]   // free text, keep them honest
}
```

`references/rigs/o11d-evo-rgb-reversed-bridge-example.json` is an older test snapshot with
the Nollie, DRAM, and motherboard driven through the OpenRGB bridge. Its Corsair
rings and radiator ordering predate the native example's geometry corrections.

## Controllers and bindings

```json
"controllers": {
  "hub": {
    "device": "<daemon device uuid>",
    "layout_device_id": "<layout_device_id from GET /devices/{id}>",
    "bindings": [
      { "slot": "channel-3", "template": "lian-li-sl-infinity-fan", "instances": 3,
        "place": "top", "chain": "rear_to_front", "rotate": 3.14159 }
    ]
  }
}
```

Each binding is one `PUT /devices/{id}/attachments` entry plus placement. Fields:

| field | meaning |
|---|---|
| `slot`, `template`, `instances`, `led_offset` | the attachment binding itself; several bindings may share a slot with different `led_offset` (two strips on one channel) |
| `place` | a case-spec mount name, or `motherboard` for a full-board part like a backplate |
| `chain` | how instances map onto a row/column: `rear_to_front` (default) or `front_to_rear` for rows, `top_to_bottom` (default) or `bottom_to_top` for columns |
| `index` | pin one instance to one mount position explicitly (single-fan slots on a column) |
| `board` | `{u, v, w, hgt, rot}` board-frame rectangle instead of a mount |
| `anchor` + `offset` + `size_mm` + `rot` | board feature by name, nudged by `{du, dv}` mm |
| `case` | `{d, h, w, hgt, rot}` raw case-frame rectangle |
| `rotate` | extra rotation (radians) applied after placement; the whole-zone flip |
| `mirror` | reverse LED travel along the zone's long axis (ring winding, strip direction, matrix columns) |
| `mirror_y` | reverse the cross axis (matrix rows, vertical strips) |
| `led_start_override` | force the zone's LED window start; `0` means "relative to the segment", the robust choice for strimer slots |
| `label` | name fragment for the generated zone |

Board-frame placements (`board`, `anchor`, `motherboard`) flip with the board in a
reversed view; `rot` is specified in the board frame and the generator adds π. Mount
placements take their rotation from the case spec; use `rotate` to flip a template
whose natural orientation disagrees with the mount.

## Raw zones

Devices without an attachment profile (LCDs, AIO rings, RAM, onboard accents) are placed
directly from their segment:

```json
{ "layout_device_id": "corsair:1b1c:0c4e:...", "segment": "Display", "name": "Pump LCD",
  "kind": "display", "px": [480, 480], "circular": true, "size_mm": [55, 55], "anchor": "cpu_socket" }
```

`kind` is one of `display` (matrix sized to the panel, `lcd-display` preset), `ring`
(`count`, optional `start_angle`, `direction`), `vstrip` / `hstrip` (`count`, optional
`direction`), `custom` (explicit LED positions), or `point`. `segment` must be the device segment name exactly as
`GET /devices/{id}` reports it. `mirror`, `mirror_y`, and `rot` work here too.

Custom zones require a positive integer `count` and exactly that many `positions`
(`{"x": 0.0, "y": 1.0}`), in device LED order. Each coordinate must be a finite
number in `[0, 1]`. Use `circular: true` for a ring-shaped footprint; the default
footprint is rectangular. The generator preserves position order and applies
`mirror` and `mirror_y` before the placement rotation. Raw-zone `rot` is the final
canvas rotation in radians, including for board anchors; it does not inherit the
board flip. Keep the map's source URL with the rig when using measured coordinates.

## Bridged hubs

A hub reached through the OpenRGB bridge has the layout id
`openrgb:<host>:<port>:<identity>` (`openrgb:127-0-0-1:6742:serial:0994fa72ab3cae43`; the
fingerprint used for config keys is a different string, see below) and one generic slot
per zone (`channel-1`,
`channel-atx-1`, ...) with cumulative `led_start`. Fan and strip bindings work as on the
native hub, so a `controllers` entry only needs the new `device` uuid and layout id.

Strimers are the exception. Through the bridge each strimer row is its own zone (six
20-LED "Channel ATX n", four or six 27-LED "Channel GPU n"), and a 120-LED strimer template
cannot bind to a 20-LED zone, so the rows are placed as raw strips by a `bridge_rows`
block that the generator expands into N `raw_zones` entries:

```json
"bridge_rows": [
  { "layout_device_id": "openrgb:127-0-0-1:6742:serial:0994fa72ab3cae43",
    "name": "24-pin strimer row",
    "segments": ["Channel ATX 1", "Channel ATX 2", "Channel ATX 3", "Channel ATX 4", "Channel ATX 5", "Channel ATX 6"],
    "kind": "hstrip", "count": 20,
    "anchor": "atx24_header", "offset": { "du": 45, "dv": -18.75 }, "pitch_mm": 7.5,
    "size_mm": [90, 7], "rot": 3.14159 }
]
```

| field | meaning |
|---|---|
| `segments` | the zone names in row order, exactly as `GET /devices/{id}` reports them; row i gets `segments[i]` and the name `"<name> <i+1>"` (or `names[i]`) |
| `kind`, `count` | `hstrip` or `vstrip`, LEDs per row (a list when rows differ) |
| `anchor` + `offset`, or `board` | where row 1 sits, same meaning as for bindings |
| `pitch_mm` | spacing between rows, applied along `stack` |
| `stack` | `dv` or `du`; defaults to the row's cross axis (`dv` for a horizontal row, `du` when `rot` makes it vertical) |
| `size_mm`, `rot`, `direction`, `mirror`, `mirror_y` | per-row geometry and flags, shared by every row |

The dial-in flags read the same way as for a strimer matrix, one level up: the whole block
upside down is a reversed `segments` list (`mirror_y` on a matrix); every row running the
wrong way along the cable is `direction` or `mirror` (columns).

## Bridge zone sizes

Hubs arrive through the bridge with every zone at 0 LEDs, and OpenRGB does not persist the
sizes you set. `bridge.zone_sizes` is fingerprint → zone name → LED count, the same shape
as `drivers.openrgb.zone_sizes` in the daemon config; `gen_layout.py apply` merges it into
that key (`--skip-zone-sizes` to opt out) so a reinstall or a server restart restores the
hub. The key is the full fingerprint string as the driver mints it, the same form
`controller_fps` uses: `bridge:openrgb:<host>:<port>:serial:<SERIAL>` with the serial in
the case OpenRGB reports (upper for the Nollie), or `...:location:<path>` for devices
without a serial. It is not the layout id: `GET /devices/{id}` carries it as
`bridge.fingerprint`, `hypercolor devices info <id>` prints it on daemons that ship the
coverage routes, and the bridge matches the key case-insensitively:

```json
"bridge": { "zone_sizes": { "bridge:openrgb:127.0.0.1:6742:serial:0994FA72AB3CAE43": { "Channel 1": 20, "Channel 2": 60, "Channel ATX 1": 20 } } }
```

`apply` prefers the `bridge.fingerprint` the daemon reports for the matching controller
over the spelling in the rig, so a key written as the device's layout id
(`openrgb:127-0-0-1:6742:serial:...`) is also resolved when the device is on the daemon;
the rig's own string is the fallback.

`plan` warns when a zone the layout targets is still at 0 LEDs on the daemon, because
frames into an unsized zone succeed and light nothing.

## Which flag fixes what

When the user reports what they see, translate it before touching the file:

| observation | cause | flag |
|---|---|---|
| a sweep hits fan 3 before fan 1 | chain order | `chain` |
| each fan animates the wrong way but the row order is right | template winding | `rotate: π` on edge-on templates, `mirror` on rings |
| a matrix (strimer) shows the sweep on the wrong strip first | rows reversed | `mirror_y` |
| a matrix runs the sweep the wrong way along the cable | columns reversed | `mirror` |
| a vertical thing is upside down | length reversed | `rotate: π` (matrices) or `mirror` (strips) |
| a ring spins the wrong way | winding | `mirror` |

A 180° rotation reverses both axes of a matrix. If the user says "still wrong" after a
π rotation, the real fix was a single axis; revert and use `mirror` or `mirror_y`.

## Generator regression checks

From the repository root, run `uv run python -m unittest discover -s skills/rig-setup/tests -v`.
The tests check generated geometry and SVG placement; they do not establish physical
fan identity or confirm the pending Corsair mount rotation.
