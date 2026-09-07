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
  "verified": [ ... ], "unverified": [ ... ]   // free text, keep them honest
}
```

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
`direction`), or `point`. `segment` must be the device segment name exactly as
`GET /devices/{id}` reports it. `mirror`, `mirror_y`, and `rot` work here too.

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
