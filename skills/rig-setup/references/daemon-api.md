# Daemon surfaces the setup touches

Everything runs against a live daemon on `http://localhost:9420/api/v1` (override with
`--base` on the generator). Three surfaces exist; use whichever the session has:

- **MCP tools** (`mcp__hypercolor__*`) for inspection and activation: `get_devices`,
  `get_layout`, `list_effects`, `activate_scene`, `set_brightness`, `diagnose`.
- **REST** for everything the generator does (attachments, layouts, scenes, identify).
  `curl` works; the generator uses plain `urllib`.
- **`hypercolor` CLI** for the same reads when MCP is not connected: `hypercolor devices`,
  `hypercolor layouts`, `hypercolor scenes`, `hypercolor status`, `hypercolor diagnose`.
  Run `hypercolor --help` for the exact subcommand tree of the installed version.

Responses are enveloped: `{ "data": ..., "meta": {...} }`.

## Inventory

| call | gives you |
|---|---|
| `GET /devices` | every known device: `id`, `name`, `state`, `led_count`, `segments` count |
| `GET /devices/{id}` | `layout_device_id` (the id layouts use), `segments[]` with `name`, `led_count`, `topology_hint` (strip, ring, matrix rows/cols, display w×h) |
| `GET /devices/{id}/attachments` | the controller's `slots[]` (`id`, `led_start`, `led_count`, `suggested_categories`), current `bindings`, `suggested_zones` |
| `GET /attachments/templates?limit=200` | the component catalog: fans, strips, strimers, AIO caps, case strips. `limit` caps at 200 |

Only devices with attachment slots (hubs, controllers, strimer bridges) take bindings.
Everything else (keyboards, LCD receivers, RAM, onboard accents) is placed straight from
its segments as a raw zone.

## Attachments

`PUT /devices/{id}/attachments` with `{ "bindings": [...], "validate_only": bool }`
replaces the controller's profile wholesale and answers with `suggested_zones` (one per
template instance, with `led_start`, `led_count`, `topology`). The generator turns those
into layout outputs, so the daemon stays the authority on template geometry.

Facts that bite:

- `validate_only: true` still needs every referenced template to exist, so custom
  templates are created before a dry run. `POST /attachments/templates` takes a bare
  template body; the daemon forces `origin: user`. There is no delete route today.
- Slot `led_start` values can depend on the profile itself. The Nollie 32 puts its GPU
  strimer slot right after the main channels when no ATX binding is enabled and 120 LEDs
  later when one is. Zones that use a relative `led_start` (0) are immune to this.
- A cable-type change (dual vs triple GPU strimer) is stored immediately but reaches the
  USB protocol only when the device reconnects. Until then the driver keeps driving the
  old row count.
- `POST /devices/{id}/attachments/{slot}/identify` with `{"color": "#0000FF",
  "duration_ms": 6000}` flashes one slot through the device path and bypasses the layout
  entirely. Blue coming out blue clears channel order and routing in one shot. Two
  optional selectors narrow the flash: `binding_index` (zero-based position in the slot's
  binding list, default 0, so set it whenever a slot carries several bindings such as two
  strips on one channel) and `instance` (zero-based template instance within that binding,
  default all instances).

## Layouts

- `POST /layouts` `{name, description, canvas_width, canvas_height}` creates an empty
  layout; `PUT /layouts/{id}` `{zones: [...]}` replaces its outputs wholesale and returns
  a **summary** (id, name, canvas, zone_count, is_active), not the layout.
- Editing the active layout with `PUT` does not re-apply the live copy. Follow with
  `POST /layouts/{id}/apply`. The generator does this automatically.
- A layout carries its own canvas size; non-4:3 sizes are fine (the render loop retunes).
- Zone routing: `zone_name` selects the device segment by alias (`channel-3` matches
  `Channel 3`, `atx-strimer` matches `ATX Strimer`); `attachment.led_start` is read as a
  device-global index when it falls inside that segment, otherwise as an offset from the
  segment start when it fits.
- After a layout is applied, the daemon auto-places every connected device the layout
  does not mention (keyboards, desk strips) at default positions. That is expected; the
  case layout only has to cover the case.

## Scenes

- `POST /scenes` `{name, description}` creates a scene with one default zone and returns
  a summary; `GET /scenes/{id}` for the full document.
- `PUT /scenes/{id}` is a whole-document replace. Each zone needs `members` (one per
  layout output: `{id, device_id, segment, name}`) **and** `layout.placements` naming
  exactly those members once each; the daemon rejects the request otherwise. Set
  `layout_id` so activating the scene applies the layout.
- `POST /scenes/{id}/activate` (or the `activate_scene` MCP tool) swaps the rig; reversible
  by activating the previous scene.

## Effects and colour surprises

Effects paint the whole canvas, including corners nothing else occupies. Color Wave, for
one, fades its background toward the palette accent at the bottom-right corner, so an
output that lives only in that corner shows a warm tint while the rest sweeps purple.
When a single output looks the wrong colour, flash it with identify before blaming the
mapping; if the flash is right, the answer is in the effect or the zone's position.
