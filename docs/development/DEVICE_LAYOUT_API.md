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

| Method   | Path                     | Purpose                                  |
| -------- | ------------------------ | ---------------------------------------- |
| `GET`    | `/devices`               | List devices (with filters + pagination) |
| `GET`    | `/devices/{id}`          | Fetch one device                         |
| `PUT`    | `/devices/{id}`          | Update user settings (`name`, `enabled`) |
| `DELETE` | `/devices/{id}`          | Remove a tracked device                  |
| `POST`   | `/devices/discover`      | Start discovery scan                     |
| `POST`   | `/devices/{id}/identify` | Trigger identify pattern                 |

### List query params

`GET /devices?offset=0&limit=50&status=active&backend_id=wled&driver=wled&q=desk`

Supported:

- `offset` (default `0`)
- `limit` (default `50`, range `1..=200`)
- `status` (`known`, `connected`, `active`, `reconnecting`, `disabled`)
- `backend_id` (case-insensitive output route match, for example `wled`)
- `driver` (case-insensitive owning driver match, for example `wled`)
- `q` (case-insensitive substring match on name/vendor)

### Update payload

`PUT /devices/{id}`

```json
{
  "name": "Desk Strip",
  "enabled": false
}
```

Notes:

- At least one of `name` or `enabled` is required.
- `name` is trimmed and must not be empty.
- `enabled=false` maps runtime state to `disabled`.
- `enabled=true` transitions `disabled` back to `known`.

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

| Method   | Path                  | Purpose                                         |
| -------- | --------------------- | ----------------------------------------------- |
| `GET`    | `/layouts`            | List saved layouts                              |
| `POST`   | `/layouts`            | Create layout                                   |
| `GET`    | `/layouts/active`     | Get currently active layout from spatial engine |
| `GET`    | `/layouts/{id}`       | Fetch one layout (full `SpatialLayout`)         |
| `PUT`    | `/layouts/{id}`       | Update layout metadata/canvas size              |
| `POST`   | `/layouts/{id}/apply` | Apply saved layout to spatial engine            |
| `DELETE` | `/layouts/{id}`       | Delete saved layout                             |

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
  "canvas_height": 200
}
```

All fields are optional.

### Apply behavior

`POST /layouts/{id}/apply`

- Loads saved layout from the store.
- Calls `spatial_engine.update_layout(...)`.
- Returns `{ layout, applied: true }`.

Layout authoring note:

- Each `SpatialLayout.zones[].device_id` should use the device's
  `layout_device_id` from `GET /devices`, not its physical `id`.

### Delete behavior

`DELETE /layouts/{id}`

- Fails with `409 conflict` when trying to delete the active layout.
- Returns `{ id, deleted: true }` on success.

### Name resolution rules

Same as devices:

- UUID or case-insensitive name accepted.
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
