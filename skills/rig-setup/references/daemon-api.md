# Daemon surfaces the setup touches

Everything runs against a live daemon on `http://localhost:9420/api/v1` (override with
`--base` on the generator). Three surfaces exist; use whichever the session has:

- **MCP tools** (`mcp__hypercolor__*`) for inspection and activation: `get_devices`,
  `get_layout`, `list_effects`, `activate_scene`, `set_brightness`, `diagnose`.
- **REST** for everything the generator does (attachments, layouts, scenes, identify).
  `curl` works; the generator uses plain `urllib`.
- **`hypercolor` CLI** for the same reads when MCP is not connected: `hypercolor devices`,
  `hypercolor layouts`, `hypercolor scenes`, `hypercolor status`, `hypercolor diagnose`,
  and for the coverage phase `hypercolor devices coverage`, `hypercolor devices unclaimed`,
  `hypercolor devices discover --target openrgb`, `hypercolor config set <key> <value>`,
  and `hypercolor openrgb status | hints | partition | start | stop | resize`. Run
  `hypercolor --help` for the exact subcommand tree of the installed version; older
  daemons lack the `openrgb` verb and the two coverage reads, and `scripts/coverage.py`
  covers that gap.
- **MCP for the bridge**: the `openrgb_setup` prompt walks the whole ladder, the
  `openrgb_status` tool returns what `hypercolor openrgb status` prints, and `diagnose`
  carries the `openrgb` check (endpoint reachable, protocol version, controller count,
  output-disabled routes with reasons).

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

## Coverage and the OpenRGB bridge

| call | gives you |
|---|---|
| `GET /devices/coverage` | one row per physical device: `identity`, `native: {device_id, driver_id, state} \| null`, `bridge: {device_id, output_enabled, disabled_reason} \| null`, `unclaimed: bool`, `active: native \| bridge \| none \| conflict`. Rows join native devices, bridge routes, and the unclaimed store by serial, then SMBus bus plus address, then USB path |
| `GET /devices/unclaimed` | `ListResponse` of `UnclaimedDevice { vendor_id, product_id, manufacturer, product, serial, bus_path, interface_classes, claimable_by }`; `claimable_by` names a native driver that knows the device but is disabled. USB only; SMBus has no enumerate-then-filter step |
| `GET /devices` (bridged rows) | `origin.transport == "bridge"` with `origin.driver_id == "openrgb"`, layout id `openrgb:<host>:<port>:<fingerprint>`, and `bridge: { endpoint, controller_index, identity_confidence, detector_class, output_enabled, disabled_reason, protocol_version }` |
| `GET /drivers` | every driver with `enabled`, `config_key`, and its `protocols[]` (`vendor_id`, `product_id` as integers); the coverage script diffs the host's USB list against this on daemons without the two routes above |
| `GET /config/keys/drivers.openrgb.zone_sizes` | fingerprint → zone name → LED count, the only place hub zone sizes survive an OpenRGB restart |

Facts that bite:

- On a daemon that predates Spec 81, `/devices/coverage` and `/devices/unclaimed` answer
  404 with `code: device_not_found` (the path matched `/devices/{id}`), not
  `route_not_found`. Treat both codes as "route missing".
- Bridged devices sit at `status: known` until the active layout targets them and connect
  then. Identify works on them anyway through a temporary connect.
- The conflict guard output-disables a bridge route whenever a renderable native device
  shares its identity, with `disabled_reason = "native driver owns this device (<driver_id>)"`,
  and publishes `DeviceStateChanged`. Handing a device to the bridge is
  `PUT /devices/{id}` with `{"enabled": false}` on the native device.
- Config writes take the bare value as the body: `PUT /config/keys/drivers.openrgb.enabled`
  with `true`, `PUT /config/keys/drivers.openrgb.zone_sizes` with the whole map. The
  response says whether the change went `live` or `requires_restart`. Driver sections may
  read back as `{"redacted": true}`, so a read-merge-write of `zone_sizes` degrades to a
  write of what you hold; keep every bridged hub's sizes in one rig spec.
- `hypercolor devices discover --target openrgb` triggers a bridge-only discovery pass;
  the daemon quiesces it while a native SMBus scan runs, and vice versa.
- Events: `UnclaimedDevicesChanged { count }` on the default `events` topic when the
  unclaimed store changes, so a UI or a long-lived agent refetches instead of polling.

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
