+++
title = "Scenes & snapshots"
description = "Save, activate, and automate whole-rig lighting with scenes, snapshot mode, and start_scene boot restore."
weight = 60
+++

Scenes are Hypercolor's reusable whole-rig lighting resource. A scene contains
zones, layer stacks, effect controls, device assignments, display faces, an
optional named layout, and an optional activation brightness. Activating one
switches the entire rig in a single operation.

The scene's mutation mode decides whether normal runtime actions may rewrite it:

- `live` scenes accept effect, control, and display-face changes while active.
- `snapshot` scenes preserve a captured configuration and reject runtime
  actions that would rewrite it.

## Create or capture a scene

Create a live scene when you want an editable workspace:

```bash
hypercolor scenes create "Gaming Night" --description "Desk and displays"
```

Create seeds a Default zone. You can then activate the scene and build its
zones and layers through Studio, the REST API, or the normal effect commands.

Two more flags shape what you get. `--mutation-mode <live|snapshot>` sets the
mode described above, defaulting to `live`, so `--mutation-mode snapshot`
creates a scene that rejects runtime rewrites from the moment it exists.
`--enabled <true|false>` decides whether the scene starts enabled, defaulting to
`true`.

Capture the current runtime state when you already have the rig tuned:

```bash
hypercolor scenes snapshot "Ambient Work" \
  --description "Low, cool desk lighting"
```

A snapshot copies the complete live scene into a new snapshot-mode scene. It
includes zones, members, layers, controls, display faces, and the current named
layout reference. Global output brightness is not captured. Brightness remains
global state, although a scene may explicitly carry `activation_brightness` for
use when it is activated.

## List, inspect, and activate scenes

```bash
hypercolor scenes list
hypercolor scenes info "Ambient Work"
hypercolor scenes activate "Ambient Work"
hypercolor scenes active
hypercolor scenes deactivate
```

Names are fuzzy matched when activating or inspecting a scene. Deactivate
returns to the auto-managed Default scene.

Scene activation commits the scene switch first. It then applies the scene's
optional named layout and activation brightness. The REST response reports the
layout and brightness outcomes separately, so a missing layout or output error
is visible without pretending the committed scene switch rolled back.

## Delete a scene

```bash
hypercolor scenes delete "Ambient Work" --yes
```

The confirmation guard keeps fuzzy matching from deleting an unintended scene.

## Boot restore with `start_scene`

The daemon's `start_scene` setting controls which scene loads on startup:

```toml
[daemon]
start_scene = "last"
```

- `"last"` restores the scene active at shutdown.
- `"default"` starts in the auto-managed Default scene.
- Any other non-empty value selects a saved scene by name or id.
- An empty string starts without selecting a saved scene.

The CLI's global `--profile` flag is unrelated. It selects a daemon connection
profile from `cli.toml`, so scripts can switch between local and remote
Hypercolor instances. Lighting state is stored only as scenes.

## External automation

Hypercolor does not include a scheduler or trigger engine. An external system
owns time, solar, presence, or webhook conditions and activates a scene when
they match:

```bash
hypercolor scenes activate "Evening"
```

The same action is available through `POST /api/v1/scenes/{id}/activate` and the
MCP `activate_scene` tool. The MCP `setup_automation` prompt helps prepare a
reusable scene and explains this external activation boundary.

## REST routes

The scene collection exposes these operations:

```text
GET    /api/v1/scenes
POST   /api/v1/scenes
POST   /api/v1/scenes/snapshot
GET    /api/v1/scenes/{id}
PUT    /api/v1/scenes/{id}
DELETE /api/v1/scenes/{id}
POST   /api/v1/scenes/{id}/activate
```

`PUT` replaces a complete stored scene document. Fine-grained edits target the
live tree under `/api/v1/scene`.

## Legacy profile import

On the first startup after upgrading, Hypercolor imports legacy
`profiles.json` entries into scenes. The import preserves effect controls,
display faces, layout references, descriptions, and saved brightness behavior.
The source file is retired only after the converted scenes are durably written.

## See also

- [Configuration](@/guide/configuration.md) for the full `[daemon]` reference
- [CLI reference](@/api/cli.md) for every `scenes` flag
- [REST API](@/api/rest.md) for complete scene documents and revision handling
- [Agent workflows](@/agents/workflows.md) for MCP and CLI automation patterns
