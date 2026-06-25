+++
title = "Profiles & scenes"
description = "Profiles save your full lighting state; scenes hold automated configurations. Learn how each works and how start_profile restores your setup on boot."
weight = 130
+++

Profiles and scenes are both ways to store your lighting configuration, but they do
different jobs. Profiles are snapshots you recall on demand. Scenes are named
configurations the engine can switch between, with priority and mutation controls
that let you build automated or context-driven lighting behavior. Understanding the
distinction saves a lot of confusion.

![The scene switcher in the Hypercolor web UI](/img/ui/ui-scenes.webp)

## Profiles

A profile is a named snapshot of the current full lighting state: the active effect,
its control values, the active preset (if any), display-face assignments, spatial
layout, and global brightness. When you apply a profile, all of those are restored
at once. Profiles are persisted in `profiles.json` inside the Hypercolor data directory.

Applying a profile rewrites the active scene's primary state. If the active scene
is a named scene in `snapshot` mutation mode, the daemon refuses the apply and
returns a conflict error, because snapshot scenes are locked against runtime
edits. Return to the Default scene or deactivate the snapshot scene first, then
apply the profile. A named scene in the default `live` mode accepts the apply.

### Save a profile

```bash
hypercolor profiles create "Gaming Night"
hypercolor profiles create "Ambient Work" --description "Low brightness desk setup"
```

The `--force` flag overwrites an existing profile with the same name instead of
returning a conflict error:

```bash
hypercolor profiles create "Gaming Night" --force
```

### Apply a profile

```bash
hypercolor profiles apply "Gaming Night"
```

Profile names are fuzzy-matched, so short unambiguous substrings also work. The
apply is immediate. The `--transition` flag is accepted but transitions are not yet
implemented, so only `0` is valid; any other value is rejected with an error.

### List and inspect profiles

```bash
hypercolor profiles list
hypercolor profiles list --format table
hypercolor profiles info "Gaming Night"
```

The table format shows profile ID, name, saved brightness, and description at a glance.

### Delete a profile

```bash
hypercolor profiles delete "Gaming Night" --yes
```

The `--yes` flag is required. Without it the command warns and exits, so you can not
accidentally wipe a profile.

### Profile fields

When you save a profile, the daemon captures:

- The active effect and all of its current control values (including resolved preset values)
- Any display-face assignments (for devices with LCD screens)
- The active spatial layout ID
- Global brightness as a 0–100 integer

On apply, the daemon restores each field. If an effect referenced in a profile is no
longer installed, the apply fails with an error naming the missing effect; if the
saved layout ID no longer exists, the apply also fails. Control values that no
longer fit the effect's schema are dropped and logged as a warning, and the rest
of the profile still applies.

## Boot restore: `start_profile`

By default the daemon restores your last-active state on startup. This is controlled
by the `start_profile` key in `~/.config/hypercolor/hypercolor.toml`:

```toml
[daemon]
start_profile = "last"
```

The value `"last"` (the default) means the daemon's runtime session is persisted and
replayed on the next boot — your lights come back to where they were without any
action from you. Set this to a specific profile name to always boot into a known
state:

```toml
[daemon]
start_profile = "Gaming Night"
```

Or set it to an empty string to boot with no profile applied:

```toml
[daemon]
start_profile = ""
```

{% callout(type="tip") %}
The connection `--profile` flag on the `hypercolor` CLI binary is completely separate
from lighting profiles. `--profile` selects a daemon connection profile from
`~/.config/hypercolor/cli.toml` (host, port, API key). It does not apply a lighting
profile. The two concepts share the name but are unrelated.
{% end %}

## Scenes

A scene is a named, persistent whole-rig configuration managed by the daemon's scene
manager. Where a profile is something you apply once and forget, a scene is something
the engine holds as an active slot, with a priority, an enabled flag, and a mutation
mode that controls whether live interactions can rewrite it.

Every Hypercolor session has a built-in **Default** scene that is always present. The
Default scene is what you work with in `hypercolor effects activate`, brightness
changes, and layout edits when no named scene is active. The Default scene cannot be
deleted.

Named scenes let you define multiple distinct whole-rig configurations and switch
between them explicitly, or activate them from automation (MCP tools, the REST API,
scripts).

{% callout(type="info") %}
Scenes are whole-rig configurations. They hold a set of zones across your full
device roster. They are not rooms, areas, or smart-home groupings. Zones are the
flexible partitions within a scene that let you run different effects on different
parts of your rig simultaneously. See the [Studio zones guide](@/studio/zones.md)
for zone authoring.
{% end %}

### Scene fields

The table output from `hypercolor scenes list` shows ID, name, mode, priority,
and enabled state:

| Field | Meaning |
|---|---|
| `mutation_mode` | `live` (default): the Studio and live controls can rewrite the scene. `snapshot`: the scene is locked; live actions do not mutate it. |
| `priority` | Higher number wins when the engine selects the active scene from overlapping candidates. |
| `enabled` | Disabled scenes are stored but never selected automatically. |

Ephemeral scenes are internal and filtered out of the list entirely. The `kind`
field (`named` vs `ephemeral`) is not in the list table, but `hypercolor scenes
active` surfaces it for whatever scene is currently running.

### Create a scene

```bash
hypercolor scenes create "Stream Mode"
hypercolor scenes create "Ambient" --description "Low-key background" --enabled true
hypercolor scenes create "Locked Config" --mutation-mode snapshot
```

A new scene always starts with a Default zone holding the current device layout, so
the Studio always has at least one zone to work with.

### Activate and deactivate scenes

```bash
hypercolor scenes activate "Stream Mode"
hypercolor scenes active
hypercolor scenes deactivate
```

`scenes deactivate` returns the engine to the Default scene. `scenes active` shows
what is currently running, including its kind, mutation mode, priority, and group
count.

The `--transition` flag is accepted by the CLI but the activate endpoint does not
read it yet, so scene activation is immediate regardless of the value you pass.

### List and inspect scenes

```bash
hypercolor scenes list
hypercolor scenes list --format table
hypercolor scenes info "Stream Mode"
```

### Delete a scene

```bash
hypercolor scenes delete "Stream Mode" --yes
```

The Default scene cannot be deleted. Deleting the active named scene switches the
engine back to Default.

## Profiles vs scenes: choosing the right tool

| | Profile | Scene |
|---|---|---|
| Stores effect + controls | yes | yes (via zones) |
| Stores brightness | yes | no |
| Stores spatial layout | yes | no |
| Stays active after apply | no (one-shot restore) | yes (remains the active slot) |
| Can be activated by automation | yes (REST `POST /profiles/:id/apply`) | yes (REST `POST /scenes/:id/activate`) |
| Can be locked against live edits | no | yes (`mutation_mode = snapshot`) |
| Survives reboots automatically | yes (via `start_profile = "last"`) | yes (scene store is persisted) |

Use a profile when you want a quick snapshot restore — "get me back to my Friday
night setup." Use a scene when you want to maintain a named slot that Studio and
automation can reference by name, or when you need the locking behavior of
`snapshot` mode.

## REST API

The daemon exposes full CRUD for both resources. All profile endpoints are under
`/api/v1/profiles` and scene endpoints are under `/api/v1/scenes`.

Profile endpoints: `GET /profiles`, `GET /profiles/:id`, `POST /profiles`,
`PUT /profiles/:id`, `DELETE /profiles/:id`, `POST /profiles/:id/apply`.

Scene endpoints: `GET /scenes`, `GET /scenes/:id`, `GET /scenes/active`,
`POST /scenes`, `PUT /scenes/:id`, `DELETE /scenes/:id`,
`POST /scenes/:id/activate`, `POST /scenes/deactivate`.

Names and IDs are interchangeable in path parameters — the daemon fuzzy-matches
by name when a UUID is not provided. See the [REST API reference](@/api/rest.md)
for the full envelope and error shapes.

## Further reading

- [Studio: scenes](@/studio/scenes.md) — build and edit scene zone layouts in the UI
- [Studio: zones](@/studio/zones.md) — zone partition authoring, zone roles, and multi-zone effects
- [Configuration](@/guide/configuration.md) — full `[daemon]` config reference including `start_profile` and `shutdown_behavior`
- [CLI reference](@/api/cli.md) — all `profiles` and `scenes` subcommand flags
