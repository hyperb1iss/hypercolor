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
    "message": "preset not found: foo"
  },
  "meta": {}
}
```

## Endpoint Map

| Method   | Path                                       | Purpose                              |
| -------- | ------------------------------------------ | ------------------------------------ |
| `GET`    | `/library/favorites`                       | List favorites                       |
| `POST`   | `/library/favorites`                       | Add or refresh a favorite            |
| `DELETE` | `/library/favorites/{effect}`              | Remove favorite by effect id or name |
| `GET`    | `/library/presets`                         | List presets                         |
| `POST`   | `/library/presets`                         | Create preset                        |
| `GET`    | `/library/presets/{id_or_name}`            | Fetch preset                         |
| `PUT`    | `/library/presets/{id_or_name}`            | Update preset                        |
| `DELETE` | `/library/presets/{id_or_name}`            | Delete preset                        |
| `POST`   | `/effects/{effect}/presets/{preset}/apply` | Apply preset through its effect      |
| `GET`    | `/library/playlists`                       | List playlists                       |
| `POST`   | `/library/playlists`                       | Create playlist                      |
| `GET`    | `/library/playlists/{id_or_name}`          | Fetch playlist                       |
| `PUT`    | `/library/playlists/{id_or_name}`          | Update playlist                      |
| `DELETE` | `/library/playlists/{id_or_name}`          | Delete playlist                      |
| `POST`   | `/library/playlists/{id_or_name}/activate` | Start playlist runtime               |
| `GET`    | `/library/playlists/active`                | Inspect active playlist runtime      |
| `POST`   | `/library/playlists/deactivate`            | Deactivate playlist runtime          |

## ID vs Name Resolution

`{id_or_name}` endpoints accept either:

- UUID v7 string, or
- case-insensitive resource name.

This currently applies to:

- Presets: `GET/PUT/DELETE /library/presets/{id_or_name}`
- Playlists: `GET/PUT/DELETE /library/playlists/{id_or_name}` and `POST /library/playlists/{id_or_name}/activate`

Effect-scoped preset apply takes an effect id or name and a canonical preset id.

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

- integer -> `ControlValue::Int`
- float -> `ControlValue::Float`
- boolean -> `ControlValue::Bool`
- string -> `ControlValue::Text`
- RGBA array of 4 numbers -> `ControlValue::ColorLinear`

### Stored / Returned Controls Shape

Preset responses are strongly typed enums, for example:

```json
{
  "controls": {
    "speed": { "kind": "float", "value": 7.5 },
    "enabled": { "kind": "bool", "value": true },
    "accent": {
      "kind": "color_linear",
      "value": { "r": 1.0, "g": 0.4, "b": 0.0, "a": 1.0 }
    }
  }
}
```

### Apply Preset

`POST /api/v1/effects/{effect}/presets/{preset}/apply`

The preset id comes from `GET /effects/{effect}/presets` or from the saved
preset resource. The optional request body may name a target `zone`. Applying a
preset delegates to the canonical effect sugar: it replaces the target zone's
stack, mints a fresh layer id, and returns the updated zone resource plus the
output-wake outcome.

The library owns preset storage and CRUD. It does not expose a parallel apply
implementation.

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

`POST /api/v1/library/playlists/{id}/activate`

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

### Deactivate Runtime

`POST /api/v1/library/playlists/deactivate`

Deactivates only the playlist scheduler runtime. The last activated effect
remains active until another effect replaces it or the scene is cleared.

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

Create a preset and retain its canonical id:

```bash
preset_id=$(curl -sS -X POST http://127.0.0.1:9420/api/v1/library/presets \
  -H 'content-type: application/json' \
  -d '{
    "name":"Warm Sweep",
    "effect":"solid_color",
    "controls":{"speed":7.25}
  }' | jq -r '.data.id')
```

Apply through the effect-scoped route:

```bash
curl -sS -X POST \
  "http://127.0.0.1:9420/api/v1/effects/solid_color/presets/$preset_id/apply"
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
