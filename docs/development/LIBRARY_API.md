# Library API Quick Reference

This is the agent-facing API guide for saved effect library features:

- Favorites
- Presets
- Playlists (including runtime activation)

These routes are implemented in `hypercolor-daemon` at `/api/v1/library/*`.

## Base URL and Envelopes

Base URL:

```text
http://127.0.0.1:9420/api/v1
```

All responses use the standard envelope:

```json
{
  "data": {},
  "meta": {
    "api_version": "1.0",
    "request_id": "req_...",
    "timestamp": "2026-03-04T06:00:00.000Z"
  }
}
```

Errors use:

```json
{
  "error": {
    "code": "not_found",
    "message": "Preset not found: foo",
    "details": null
  },
  "meta": {}
}
```

## Endpoint Map

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/library/favorites` | List favorites |
| `POST` | `/library/favorites` | Add or refresh a favorite |
| `DELETE` | `/library/favorites/{effect}` | Remove favorite by effect id or name |
| `GET` | `/library/presets` | List presets |
| `POST` | `/library/presets` | Create preset |
| `GET` | `/library/presets/{id_or_name}` | Fetch preset |
| `PUT` | `/library/presets/{id_or_name}` | Update preset |
| `DELETE` | `/library/presets/{id_or_name}` | Delete preset |
| `POST` | `/library/presets/{id_or_name}/apply` | Activate preset effect + controls |
| `GET` | `/library/playlists` | List playlists |
| `POST` | `/library/playlists` | Create playlist |
| `GET` | `/library/playlists/{id_or_name}` | Fetch playlist |
| `PUT` | `/library/playlists/{id_or_name}` | Update playlist |
| `DELETE` | `/library/playlists/{id_or_name}` | Delete playlist |
| `POST` | `/library/playlists/{id_or_name}/activate` | Start playlist runtime |
| `GET` | `/library/playlists/active` | Inspect active playlist runtime |
| `POST` | `/library/playlists/stop` | Stop active playlist runtime |

## ID vs Name Resolution

`{id_or_name}` endpoints accept either:

- UUID v7 string, or
- case-insensitive resource name.

This currently applies to:

- Presets: `GET/PUT/DELETE /library/presets/{id_or_name}` and `POST /library/presets/{id_or_name}/apply`
- Playlists: `GET/PUT/DELETE /library/playlists/{id_or_name}` and `POST /library/playlists/{id_or_name}/activate`

## Favorites

### Create / Upsert

```http
POST /api/v1/library/favorites
Content-Type: application/json
```

```json
{
  "effect": "solid_color"
}
```

Response:

```json
{
  "data": {
    "favorite": {
      "effect_id": "2f79...",
      "effect_name": "solid_color",
      "added_at_ms": 1762266895000
    },
    "created": true
  }
}
```

`created` is `false` if the favorite already existed and was refreshed.

### List

`GET /api/v1/library/favorites`

Returns `data.items` plus pagination metadata. Current implementation is not paged on the server and returns all items with `offset=0`, `limit=50`, `has_more=false`.

### Delete

`DELETE /api/v1/library/favorites/{effect}`

`{effect}` resolves by effect id or effect name.

## Presets

### Create / Update Payload

```json
{
  "name": "Warm Sweep",
  "description": "Optional",
  "effect": "solid_color",
  "controls": {
    "speed": 7.25,
    "enabled": true,
    "accent": [1.0, 0.4, 0.0, 1.0],
    "label": "studio"
  },
  "tags": [" cozy ", "night"]
}
```

Behavior:

- `name` is required and trimmed; empty names return `422`.
- `effect` resolves by id or name.
- `controls` is optional and must be a JSON object when present.
- Controls are validated against the effect control schema.
- Tags are trimmed and empty tags are dropped.

Accepted `controls` input JSON types:

- integer -> `ControlValue::Integer`
- float -> `ControlValue::Float`
- boolean -> `ControlValue::Boolean`
- string -> `ControlValue::Text`
- RGBA array of 4 numbers -> `ControlValue::Color`

### Stored / Returned Controls Shape

Preset responses are strongly typed enums, for example:

```json
{
  "controls": {
    "speed": { "float": 7.5 },
    "enabled": { "boolean": true },
    "accent": { "color": [1.0, 0.4, 0.0, 1.0] }
  }
}
```

### Apply Preset

`POST /api/v1/library/presets/{id_or_name}/apply`

Response includes:

- applied preset summary
- resolved effect summary
- `applied_controls`
- `rejected_controls`

This activates the effect immediately through the same engine path as direct effect activation.

## Playlists

### Create / Update Payload

```json
{
  "name": "Night Rotation",
  "description": "Optional",
  "loop_enabled": true,
  "items": [
    {
      "target": { "type": "effect", "effect": "solid_color" },
      "duration_ms": 2000,
      "transition_ms": 250
    },
    {
      "target": { "type": "preset", "preset_id": "Warm Sweep" },
      "duration_ms": 3000
    }
  ]
}
```

Target types:

- Effect target: `{ "type": "effect", "effect": "<effect id or name>" }`
- Preset target: `{ "type": "preset", "preset_id": "<preset id or name>" }`

Notes:

- `loop_enabled` defaults to `true` when omitted on create.
- `items` may be empty at create/update time, but activation will fail with `422`.
- `duration_ms` defaults to `30000` at runtime when omitted.
- `transition_ms` is persisted but not yet applied by the runtime scheduler.

### Activate

`POST /api/v1/library/playlists/{id_or_name}/activate`

Behavior:

- Any existing active playlist runtime is stopped first.
- The first playlist item is applied immediately.
- A background runtime task then advances items by `duration_ms`.

### Active Runtime

`GET /api/v1/library/playlists/active`

Returns:

- `playlist.id`
- `playlist.name`
- `playlist.loop_enabled`
- `playlist.item_count`
- `playlist.started_at_ms`
- `state` (`running`)

### Stop Runtime

`POST /api/v1/library/playlists/stop`

Stops only the playlist scheduler runtime. The last activated effect remains active until changed or stopped through effect endpoints.

## Ordering and Lifecycle Guarantees

- Favorites are listed newest first by `added_at_ms`.
- Presets and playlists are listed by `updated_at_ms` descending, then name.
- Activating playlist B while playlist A is running replaces A immediately.
- Updating or deleting an active playlist stops its runtime.
- Non-looping playlists clear active runtime state after the last item completes.

## Storage Status (Current)

When the daemon builds `AppState` from live startup state, library data is
persisted to:

- Linux default: `~/.local/share/hypercolor/library.json` (or `$XDG_DATA_HOME/hypercolor/library.json`)

Behavior:

- Snapshot is written after each library mutation.
- On load failure (missing/corrupt file), daemon logs a warning and falls back
  to in-memory storage for that run.

The API contract stays stable because storage is abstracted behind
`LibraryStore`, enabling future Turso/libsql migration without endpoint changes.

## Minimal cURL Flows

Create a preset:

```bash
curl -sS -X POST http://127.0.0.1:9420/api/v1/library/presets \
  -H 'content-type: application/json' \
  -d '{
    "name":"Warm Sweep",
    "effect":"solid_color",
    "controls":{"speed":7.25}
  }'
```

Apply by name:

```bash
curl -sS -X POST \
  http://127.0.0.1:9420/api/v1/library/presets/Warm%20Sweep/apply
```

Create and activate a playlist:

```bash
curl -sS -X POST http://127.0.0.1:9420/api/v1/library/playlists \
  -H 'content-type: application/json' \
  -d '{
    "name":"Runtime Playlist",
    "items":[
      {"target":{"type":"effect","effect":"solid_color"},"duration_ms":5000}
    ]
  }'

curl -sS -X POST \
  http://127.0.0.1:9420/api/v1/library/playlists/Runtime%20Playlist/activate
```
