# Device + Layout API Quick Reference

Agent-facing reference for device and spatial layout endpoints.

Base URL:

```text
http://127.0.0.1:9420/api/v1
```

All responses use the standard Hypercolor envelope (`data` + `meta`, or
`error` + `meta`).

## Devices

### Endpoint map

| Method           | Path                                        | Purpose                                                |
| ---------------- | ------------------------------------------- | ------------------------------------------------------ |
| `GET`            | `/devices`                                  | List devices (with filters + pagination)               |
| `GET`            | `/devices/{id}`                             | Fetch one device                                       |
| `PUT`            | `/devices/{id}`                             | Update user settings (`name`, `enabled`, `brightness`) |
| `DELETE`         | `/devices/{id}`                             | Remove a tracked device                                |
| `POST`           | `/devices/discover`                         | Start discovery scan                                   |
| `POST`           | `/devices/{id}/identify`                    | Trigger identify pattern                               |
| `GET`            | `/devices/{id}/controls`                    | Read the device's control surface                      |
| `GET/PUT/DELETE` | `/devices/{id}/attachments`                 | Read, replace, or clear attachment slots               |
| `POST`           | `/devices/{id}/attachments/{slot}/identify` | Identify one attachment slot                           |
| `POST`           | `/devices/{id}/segments/{segment}/identify` | Identify one hardware segment                          |
| `POST/DELETE`    | `/devices/{id}/pair`                        | Pair or unpair a device that needs credentials         |

### List query params

`GET /devices?offset=0&limit=50&status=active&backend_id=wled&driver=wled&q=desk`

Supported:

- `offset` (default `0`)
- `limit` (default `50`, range `1..=200`)
- `status` (`known`, `connected`, `active`, `reconnecting`, `disabled`)
- `backend_id` (case-insensitive output route match, for example `wled`)
- `driver` (case-insensitive owning driver match, for example `wled`)
- `q` (case-insensitive substring match on name/vendor)
- `include` (comma-separated summary expansions; `attachments` is the only
  supported value)

### Update payload

`PUT /devices/{id}`

```json
{
  "name": "Desk Strip",
  "enabled": false,
  "brightness": 80
}
```

Notes:

- At least one of `name`, `enabled`, or `brightness` is required.
- `name` is trimmed and must not be empty.
- `enabled=false` maps runtime state to `disabled`.
- `enabled=true` transitions `disabled` back to `known`.
- `brightness` is a percentage in `0..=100`.

### Identify payload

`POST /devices/{id}/identify`

```json
{
  "duration_ms": 1500,
  "color": "ff00aa"
}
```

Validation:

- `duration_ms` must be `1..=120000`.
- `color` must be 6-digit hex (`RRGGBB`, optional `#` prefix).

### Name resolution rules

`{id}` accepts a UUID or case-insensitive name.

- No match -> `404 not_found`
- Multiple name matches -> `409 conflict` (ambiguous name)
- Success responses always return canonical resolved `device_id`.

## Layouts

### Endpoint map

| Method   | Path                      | Purpose                                         |
| -------- | ------------------------- | ----------------------------------------------- |
| `GET`    | `/layouts`                | List saved layouts                              |
| `POST`   | `/layouts`                | Create layout                                   |
| `GET`    | `/layouts/active`         | Get currently active layout from spatial engine |
| `PUT`    | `/layouts/active/preview` | Preview a `SpatialLayout` without saving it     |
| `GET`    | `/layouts/{id}`           | Fetch one layout (full `SpatialLayout`)         |
| `PUT`    | `/layouts/{id}`           | Update layout metadata, canvas size, or zones   |
| `POST`   | `/layouts/{id}/apply`     | Apply saved layout to spatial engine            |
| `DELETE` | `/layouts/{id}`           | Delete saved layout                             |

`PUT /layouts/active/preview` takes a whole `SpatialLayout` as its body and
answers `{ previewing: true }`. It does not touch the layout store.

### List query params

`GET /layouts?offset=0&limit=50&active=true`

Supported:

- `offset` (default `0`)
- `limit` (default `50`, range `1..=200`)
- `active` (`true` filters list to active layout only)

List items include `is_active`.

### Create payload

`POST /layouts`

```json
{
  "name": "Studio Layout",
  "description": "Optional",
  "canvas_width": 640,
  "canvas_height": 360
}
```

Validation:

- `name` must not be empty after trim.
- layout names are case-insensitive unique.
- `canvas_width` and `canvas_height` must be greater than `0`.

### Update payload

`PUT /layouts/{id}`

```json
{
  "name": "Updated Studio Layout",
  "description": "Optional",
  "canvas_width": 320,
  "canvas_height": 200,
  "zones": []
}
```

All fields are optional. Omitted fields leave the stored layout untouched. A
present `zones` list is a wholesale replacement of the layout's outputs, not a
merge, so send the full set.

### Apply behavior

`POST /layouts/{id}/apply`

- Loads saved layout from the store.
- Calls `spatial_engine.update_layout(...)`.
- Returns `{ layout, applied: true, persistence_pending: false }`.

`persistence_pending` is required, not optional. It is `true` when the runtime
change is live but the store write has not settled yet, which is why the route
can answer `202` as well as `200`.

Layout authoring note:

- Each `SpatialLayout.zones[].device_id` should use the device's
  `layout_device_id` from `GET /devices`, not its physical `id`.

### Delete behavior

`DELETE /layouts/{id}`

- Fails with `409 conflict` when trying to delete the active layout.
- Returns `{ id, deleted: true, persistence_pending: false }` on success, with
  `persistence_pending` required here too.

### Name resolution rules

Layout ids are opaque strings, not UUIDs. A created layout gets
`layout_<uuid-v7>`, and the built-in one is `default`, so a bare UUID resolves
nothing.

- Exact layout id, or case-insensitive layout name.
- No match -> `404 not_found`.
- Ambiguous name -> `409 conflict`.

## Layout Target IDs and Hardware Segments

Device list and detail responses expose two different identities:

- `id` addresses the physical device through `/devices/{id}`.
- `layout_device_id` is the opaque target for
  `SpatialLayout.zones[].device_id`.

The daemon owns the mapping between those identities so rediscovery and
attachment changes do not strand spatial layouts. There is no public logical
device CRUD resource.

`DeviceSummary.segments` describes the hardware topology reported by the
driver. A segment has its own id, name, LED count, and topology hint. Segments
can be identified through
`POST /devices/{id}/segments/{segment}/identify`, but they are not independent
layout target resources.

## Scene Layout Selection

Layout selection belongs to scenes. The live scene document exposes an optional
`layout_id`, and stored scenes carry the same field. Activating that scene
deliberately selects its referenced layout.

Effects do not own layout associations. The old
`/effects/{id_or_name}/layout` routes and `effect-layouts.json` store are
removed. Any existing `effect-layouts.json` file is deliberately left orphaned
and is not migrated. Set the stored scene's `layout_id` explicitly when that
scene should use a named layout.
