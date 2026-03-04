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

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/devices` | List devices (with filters + pagination) |
| `GET` | `/devices/{id_or_name}` | Fetch one device |
| `PUT` | `/devices/{id_or_name}` | Update user settings (`name`, `enabled`) |
| `DELETE` | `/devices/{id_or_name}` | Remove a tracked device |
| `POST` | `/devices/discover` | Start discovery scan |
| `POST` | `/devices/{id_or_name}/identify` | Trigger identify pattern |
| `GET` | `/devices/debug/queues` | Inspect backend output queue diagnostics |

### List query params

`GET /devices?offset=0&limit=50&status=active&backend=wled&q=desk`

Supported:

- `offset` (default `0`)
- `limit` (default `50`, range `1..=200`)
- `status` (`known`, `connected`, `active`, `reconnecting`, `disabled`)
- `backend` (case-insensitive family display name match, for example `wled`)
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

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/layouts` | List saved layouts |
| `POST` | `/layouts` | Create layout |
| `GET` | `/layouts/active` | Get currently active layout from spatial engine |
| `GET` | `/layouts/{id_or_name}` | Fetch one layout (full `SpatialLayout`) |
| `PUT` | `/layouts/{id_or_name}` | Update layout metadata/canvas size |
| `POST` | `/layouts/{id_or_name}/apply` | Apply saved layout to spatial engine |
| `DELETE` | `/layouts/{id_or_name}` | Delete saved layout |

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

### Delete behavior

`DELETE /layouts/{id_or_name}`

- Fails with `409 conflict` when trying to delete the active layout.
- Returns `{ id, deleted: true }` on success.

### Name resolution rules

Same as devices:

- UUID or case-insensitive name accepted.
- Ambiguous name -> `409 conflict`.
