# Case spec format

A case spec describes one PC case in millimetres, independent of any particular build,
controller, or viewing side. It is the reusable half of a rig setup: write it once per
case model, then every owner of that case reuses it with their own rig spec.

Existing specs live in `references/cases/`. Check there first. If the user's case is
missing, author one from the manufacturer's spec page and drop it in the same folder so
the next person gets it for free.

## Coordinate frame

Two frames, both in mm, both independent of which side the glass is on:

- **Case frame**: `d` = distance from the front outer face (0 at the front, `depth` at
  the rear), `h` = height above the bottom outer face (0 at the floor, `height` at the
  top). Every mount that belongs to the chassis (fan rows, columns, strips) lives here.
- **Board frame**: `u` = distance from the motherboard's I/O edge toward the front,
  `v` = distance from the board's CPU-end edge toward the PCIe end. Everything attached
  to the motherboard (socket, DIMMs, headers, GPU bay) lives here so it survives the
  board being flipped in reversed-mode cases.

The **view** is chosen in the rig spec, not the case spec. `standard` puts the glass on
the viewer's left (front of the case on the viewer's right); `reversed` puts the glass on
the right (front on the left) and flips the motherboard 180° in-plane. The generator
applies the view transform, so a case spec never encodes "left" or "right".

## Shape

```json
{
  "schema": "hypercolor-case-spec/1",
  "id": "vendor-model-slug",
  "name": "Vendor Model",
  "vendor": "Vendor",
  "sources": ["https://vendor.example/product/model/"],
  "envelope": { "depth": 478, "width": 290, "height": 471 },
  "mounts": { ... }
}
```

`envelope.depth` and `envelope.height` define the canvas aspect the rig should use
(e.g. 478:471 → a 640×630 canvas). Width is recorded for completeness.

## Mount kinds

Each entry in `mounts` is named (the rig refers to these names) and has a `kind`:

| kind | fields | notes |
|---|---|---|
| `fan_row` | `fan_mm`, `count`, `h_center`, `d_centers_rear_to_front[]`, `edge_thickness_mm`, `edge_rotation` | Top or bottom fans seen edge-on from the glass. `edge_rotation` is the zone rotation for the fan template's natural orientation (top row 0, bottom row π). |
| `fan_column` | `fan_mm`, `count`, `d_center`, `h_centers_top_to_bottom[]`, `faces_viewer` | The side mount in dual-chamber cases. `faces_viewer: true` renders full rings; false renders edge-on bars. |
| `fan_single` | `fan_mm`, `d_center`, `h_center` **or** `h_from_board: {v}`, `edge_thickness_mm`, `edge_rotation`, `rotate_with_view` | The rear exhaust. `h_from_board` pins it to the board's CPU end so it follows the board flip. |
| `l_strip` | `run: {h, d_from, d_to, leds}`, `leg: {d, h_from, h_to, leds}`, `chain_order` | L-shaped chassis strips (run along the top or bottom edge, leg down the front pillar). The generator synthesizes a custom template per strip. |
| `motherboard` | `form_factor`, `board_mm: {u, v}`, `d_io_edge`, `h_low`, `h_high`, `features` | One per case. `features` are named anchor rectangles in the board frame: `{u, v, w, hgt}`. |

Standard board features worth including (ATX numbers as a starting point, adjust for
the real board when the user knows them):

| feature | u | v | purpose |
|---|---|---|---|
| `cpu_socket` | 85 | 100 | pump caps, AIO rings, pump LCDs |
| `dimm_slot_1..4` | 133, 143, 153, 163 | 100 (length 133) | RAM sticks |
| `eps_header` | 20 | 12 | CPU power strimer |
| `atx24_header` | 238 | 130 | 24-pin strimer |
| `io_shroud_accent` | 30 | 40 | onboard accent LED |
| `gpu_bay` | `u_bracket`, `length`, `v_from`, `v_to` | ghost outline for the preview and GPU-mounted parts |

## Deriving numbers from a spec page

Manufacturer pages give the envelope, the fan mount sizes per position, and radiator
clearances. That is enough to place mounts to within a centimetre:

1. Fan rows sit just inside the panel: `h_center ≈ height - panel - fan_thickness/2`
   for the top, `panel + fan_thickness/2` for the bottom. Space the centres by the fan
   size plus a few mm of frame.
2. The side column sits just behind the front glass with its span between the two rows.
   Leave a few mm so rows and column do not overlap in the projection.
3. The motherboard I/O edge is a couple of cm inside the rear panel; the board's
   vertical span is what is left between the top radiator clearance and the bottom fans.
4. Strips follow the frame edges; count LEDs at 10 mm pitch as a sanity check against
   any template you find (47 LEDs ≈ 470 mm).

Overlaps in the 2D projection are normal (the rear fan sits over the board's I/O column,
strimers cross fans). The layout samples a canvas, not a physics model; what matters is
that each output sits where the eye sees it through the glass.

Record every assumption in the spec's `notes` or per-mount `chain_order` strings so a
later identify pass knows what to check.

## Relation to Spec 70 rig templates

`docs/specs/70-agent-rig-setup.md` plans daemon-side `RigTemplate`s: mounts in normalized
front-view canvas coordinates, loaded from `data/rigs/` and searchable over MCP. A case
spec is the research artifact one layer below that: millimetres plus a view flag, so a
single file serves both a standard and a reversed build of the same case. When the
daemon-side registry lands, the generator's `Geometry` class is the export path (case spec
plus view → normalized mounts); until then the skill carries the geometry itself.
