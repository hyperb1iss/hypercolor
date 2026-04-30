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

| Method   | Path                             | Purpose                                  |
| -------- | -------------------------------- | ---------------------------------------- |
| `GET`    | `/devices`                       | List devices (with filters + pagination) |
| `GET`    | `/devices/{id_or_name}`          | Fetch one device                         |
| `PUT`    | `/devices/{id_or_name}`          | Update user settings (`name`, `enabled`) |
| `DELETE` | `/devices/{id_or_name}`          | Remove a tracked device                  |
| `POST`   | `/devices/discover`              | Start discovery scan                     |
| `POST`   | `/devices/{id_or_name}/identify` | Trigger identify pattern                 |
| `GET`    | `/devices/debug/queues`          | Inspect backend output queue diagnostics |

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

`PUT /devices/{id_or_name}`

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

`POST /devices/{id_or_name}/identify`

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

`{id_or_name}` accepts UUID or case-insensitive name.

- No match -> `404 not_found`
- Multiple name matches -> `409 conflict` (ambiguous name)
- Success responses always return canonical resolved `device_id`.

## Layouts

### Endpoint map

| Method   | Path                          | Purpose                                         |
| -------- | ----------------------------- | ----------------------------------------------- |
| `GET`    | `/layouts`                    | List saved layouts                              |
| `POST`   | `/layouts`                    | Create layout                                   |
| `GET`    | `/layouts/active`             | Get currently active layout from spatial engine |
| `GET`    | `/layouts/{id_or_name}`       | Fetch one layout (full `SpatialLayout`)         |
| `PUT`    | `/layouts/{id_or_name}`       | Update layout metadata/canvas size              |
| `POST`   | `/layouts/{id_or_name}/apply` | Apply saved layout to spatial engine            |
| `DELETE` | `/layouts/{id_or_name}`       | Delete saved layout                             |

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

`PUT /layouts/{id_or_name}`

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

`POST /layouts/{id_or_name}/apply`

- Loads saved layout from the store.
- Calls `spatial_engine.update_layout(...)`.
- Returns `{ layout, applied: true }`.

Layout authoring note:

- Each `SpatialLayout.zones[].device_id` should reference a logical device ID
  from `/logical-devices` (or `/devices/{id}/logical-devices`), not a raw
  physical controller identifier.

### Delete behavior

`DELETE /layouts/{id_or_name}`

- Fails with `409 conflict` when trying to delete the active layout.
- Returns `{ id, deleted: true }` on success.

### Name resolution rules

Same as devices:

- UUID or case-insensitive name accepted.
- Ambiguous name -> `409 conflict`.

## Logical Devices (User-Defined Segments)

Logical devices are user-authored virtual units mapped onto a physical device
LED range. Layout zones should target these logical IDs.

### Endpoint map

| Method   | Path                                    | Purpose                                           |
| -------- | --------------------------------------- | ------------------------------------------------- |
| `GET`    | `/logical-devices`                      | List all logical devices                          |
| `GET`    | `/logical-devices/{id}`                 | Fetch one logical device                          |
| `PUT`    | `/logical-devices/{id}`                 | Update logical device fields                      |
| `DELETE` | `/logical-devices/{id}`                 | Delete a user-defined segment                     |
| `GET`    | `/devices/{id_or_name}/logical-devices` | List logical devices for one physical device      |
| `POST`   | `/devices/{id_or_name}/logical-devices` | Create a new logical segment on a physical device |

### Create payload

`POST /devices/{id_or_name}/logical-devices`

```json
{
  "name": "Desk Left",
  "led_start": 0,
  "led_count": 120,
  "enabled": true
}
```

Validation:

- `name` must not be empty after trim.
- `led_count` must be greater than `0`.
- `led_start + led_count` must fit within physical LED count.
- Enabled segment ranges for a physical device cannot overlap.

Behavior:

- A default full-range logical device exists per physical device.
- When one or more enabled segment logical devices exist, the default logical
  device is auto-disabled.
- Only user-defined segment logical devices are persisted.
- Logical-device responses include the physical device `origin` object rather
  than a flat backend field. Use `origin.backend_id` only when routing/debugging,
  and `origin.driver_id` when grouping by driver ownership.

### Update payload

`PUT /logical-devices/{id}`

```json
{
  "name": "Desk Left Updated",
  "led_start": 10,
  "led_count": 100,
  "enabled": true
}
```

All fields are optional. Default logical devices cannot change
`led_start`/`led_count`.

## Effect -> Layout Associations

Bind effects to saved layouts so activating an effect can also activate a
specific spatial layout.

### Endpoint map

| Method   | Path                           | Purpose                                 |
| -------- | ------------------------------ | --------------------------------------- |
| `GET`    | `/effects/{id_or_name}/layout` | Get the associated layout for an effect |
| `PUT`    | `/effects/{id_or_name}/layout` | Associate an effect with a layout       |
| `DELETE` | `/effects/{id_or_name}/layout` | Remove an effect/layout association     |

### Set payload

`PUT /effects/{id_or_name}/layout`

```json
{
  "layout_id": "layout_1234-or-layout-name"
}
```

Notes:

- `layout_id` accepts either canonical layout ID or case-insensitive layout
  name.
- Ambiguous layout names return `409 conflict`.
- Associations are user-defined and persisted to `effect-layouts.json`.

### Apply behavior

`POST /effects/{id_or_name}/apply`

- If the effect has an associated layout and that layout still exists, the
  API applies it automatically via `spatial_engine.update_layout(...)`.
- The effect apply response includes a `layout` object indicating whether the
  association resolved and was applied.
