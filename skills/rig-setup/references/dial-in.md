# The dial-in loop

Generating the layout gets the geometry right. The wiring facts (which fan is first on
a chain, which way a ring winds, which strip of a strimer is row 0) are invisible from
software and have to be read off the lit hardware with the user. Expect three to five
rounds. Each round is: one observation, one flag, one regenerate, look again.

## Order of operations

1. **Activate the scene** so the case is lit by a directional effect. Color Wave in its
   default horizontal mode is a good probe: a band sweeping front to rear makes chain
   order and per-fan direction obvious.
2. **Walk the mounts in physical order**: top row, side column, bottom row, rear, then
   board-mounted parts (strimers, RAM, pump, bar). Ask about one group at a time.
3. **Ask for observations, not diagnoses.** "Does the sweep cross the top fans in the
   same direction as the bottom fans?" beats "is the chain reversed?". Translate what
   they say with the table in `rig-spec.md`.
4. **Change one flag per group per round**, run `gen_layout.py apply`, and confirm the
   printed `re-applied active layout: applied=True` line before asking again.
5. **When two fixes are possible, pick the single-axis one first.** A 180° rotation
   reverses both axes; if the user says "still wrong" after it, the fix was one axis.
6. **Record every confirmed fact** in the rig spec's `verified` list with a date, and
   keep the honest `unverified` list for what you had to assume.

## Discriminators

Use these before changing anything, in this order:

| symptom | test | reads |
|---|---|---|
| an output is dark | `POST /devices/{id}/attachments/{slot}/identify` with a colour | flash seen → mapping problem (zone off-canvas, wrong segment); no flash → device path or wiring |
| an output shows the wrong colour | identify with `#0000FF` | blue → the effect or the zone's canvas position; another colour → channel order in the driver segment |
| a fan lights but in the wrong place | identify the slot with `instance: n` | tells you which physical fan is chain position n |
| a strimer is dark but its sibling on the same controller works | check the ribbon seating | narrow ribbons in wide sockets only work at the keyed end |

Identify goes through the device path and skips the layout, which is exactly why it
separates "we mapped it wrong" from "the LEDs are not getting our frames".

## Hardware realities worth saying out loud

- Fans on the top and bottom rows usually face the same way, so once the bottom row is
  right, copy its chain order and rotation to the top row rather than re-deriving.
- Reversed (inverted) cases flip the motherboard 180°: I/O and CPU power at the bottom
  rear, GPU up top, DIMMs still on the front side of the socket. Board-frame placement
  handles this; do not hand-mirror coordinates.
- Strimer ribbons: the Nollie 32 has separate `LIAN LI MB` and `LIAN LI GPU` sockets,
  and a dual 8-pin ribbon is narrower than the socket. After the driver switches from
  triple to dual row counts (next reconnect), only rows 25 to 22 are driven, so a ribbon
  seated at the wrong end goes dark. Say this before it happens.
- Ring winding depends on which face of the fan points at the glass (reverse-blade
  fans, intake vs exhaust), so it is a rig fact carried by `mirror`, never a template edit.
- LCD fans are two devices: the fan ring on the hub and a display receiver. Place the
  display zone centred on the ring with a size matching the visible screen (about 60 mm
  for a 1.6-inch panel); the receiver order in the device list is an assumption until the
  screens show something distinguishable.

## Wrapping a round

When the user says it looks right, do three things: dump the final rig spec next to the
case spec (they own it), write the date-stamped facts into `verified`, and if the case
spec was new, offer to contribute it to `references/cases/` so the next owner of that
case starts from a finished template.
