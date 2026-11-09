+++
title = "Stability & versioning"
description = "What semantic versioning covers from Hypercolor 1.0 on: the frozen interfaces, what stays internal, how deprecation works, and how long old config files and backups keep loading."
weight = 90
template = "page.html"
+++

From 1.0.0 on, Hypercolor follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
for the interfaces on this page. A minor release (`1.x`) only adds to them, a
patch release (`1.x.y`) only fixes them, and anything that would break a working
integration waits for 2.0. A script, client, effect, or config file that works
through these interfaces on one 1.x release keeps working on every later 1.x
release.

Releases before 1.0 make no such promise. The [changelog](@/guide/changelog.md)
explains how pre-1.0 versions number breaking changes.

## What the promise covers 🎯

| Interface | What stays stable | Reference |
| --- | --- | --- |
| REST API | Every route under `/api/v1`: paths, methods, request bodies, response bodies, status codes, and error codes. The OpenAPI document served at `/api/v1/openapi.json` describes the same contract. | [REST reference](@/api/rest.md), [OpenAPI](@/api/openapi.md), [envelope and errors](@/api/rest-envelope-and-errors.md) |
| WebSocket binary frames | Tag bytes, header layouts, and payload encodings of every binary frame. The `ws` module of `hypercolor-leptos-ext` is the single definition: the daemon's encoders conform to it, and the web UI and TUI decode with it. | [Binary frame format](@/api/websocket-binary-frames.md) |
| WebSocket JSON messages | The `hypercolor-v1` subprotocol, the hello handshake, topic names, client and server message types, and event payloads. | [WebSocket protocol](@/api/websocket.md) |
| Config file | The `hypercolor.toml` schema: section names, key names, value types, and what each key means. See [config files](#config-files) for how long old files keep loading. | [Configuration](@/guide/configuration.md) |
| Portable backups | The `.hypercolor-backup` archive layout and its manifest schema. See [backups](#backups). | [REST reference](@/api/rest.md) |
| HTML effect SDK | The public API of the [`hypercolor`](https://www.npmjs.com/package/hypercolor) npm package, plus the effect metadata the daemon reads from a built effect: its `<meta>` tags for identity, controls, presets, and audio. | [SDK API reference](@/effects/sdk-api-reference.md), [raw HTML effects](@/effects/raw-html.md) |
| Python client | The names the [`hypercolor`](https://pypi.org/project/hypercolor/) package exports from `hypercolor` and `hypercolor.models`, and their signatures. The `hypercolor._generated` package is internal. | [PyPI](https://pypi.org/project/hypercolor/) |
| CLI for scripts | Output under `--format json` (or `-j`) and exit codes: `0` on success, `1` when a command fails, and `2` for a command-line usage error or for `status --watch` losing its daemon connection. | [CLI for agents](@/agents/cli-scripting.md) |
| MCP server | Tool names and the shape of each tool's arguments. | [Tool reference](@/agents/tools-reference.md) |

## What stays internal

These parts change in any release, minor or patch, without notice:

- **Rust crate APIs.** Every crate in the workspace is an implementation detail,
  including `hypercolor-driver-api`. Drivers that live in the tree move with it.
- **Human-readable CLI output.** Tables, colors, wording, and column order are
  for people. Scripts read `--format json`.
- **Diagnostics and metrics internals.** The fields inside system status
  internals, diagnose check details, render metrics, and logs describe how the
  daemon works today. Read them tolerantly and never branch on their exact shape.
- **Experimental drivers.** A driver marked experimental is off until you enable
  it, and its behavior, settings, and device coverage can change or disappear
  in any release.

## What a minor release may add

Additive changes ship in minor releases:

- new routes, WebSocket topics, message types, event types, MCP tools, and CLI
  commands or flags
- new optional fields in requests and MCP tool arguments
- new fields in responses, events, and `--format json` output
- new config keys, each with a default, so a file without them still loads
- new values in an existing enum, such as a new effect category or error code

A client stays compatible by ignoring fields it does not know and treating an
unknown event type or enum value as something to keep or skip rather than an
error.

## Which clients follow that rule

- **The Python client** does. It ignores fields it does not model, passes
  unknown WebSocket messages through as events, and decodes an enum value it
  predates as an unlisted member that keeps the wire string (a new effect
  category arrives as `EffectCategory("holographic")`, equal to
  `"holographic"` and not one of the listed members). An older 1.x Python
  client keeps working against a newer 1.x daemon.
- **Hypercolor's own Rust clients** (the CLI, the TUI, the web UI, and the
  desktop app) decode with the types of the release they ship in, and they
  refuse an enum value or envelope field they predate. They support a daemon
  from their own release or an earlier 1.x release. The web UI and the
  desktop app always run against the daemon they ship with. When the CLI or
  TUI talks to a remote daemon from a newer release, upgrade the CLI or TUI
  to match it.
- **Your own clients** should follow the rule above, so a daemon upgrade
  never requires a client upgrade.

## Deprecation

A deprecated route, field, message, config key, or tool keeps working for the
rest of 1.x. The release that deprecates it says so in its notes and names the
replacement, and the reference page for that surface marks it deprecated. Only a
major release removes it.

## Config files

Every 1.x build reads every config schema from **schema 4** (the floor) through
the schema it writes. An older file is upgraded in memory when it loads, and
the daemon writes the current schema on its next save. Settings you chose stay
as you set them; a value still at a retired default moves to the current default.

| Schema | Written by |
| --- | --- |
| 4 | v0.2.1 through v0.3.2 |
| 5 | v0.4.0 through v0.7.0 |
| 6 | v0.7.1 |
| 7 | v0.7.2 and later |

A file below the floor is refused with the exact hand edit that brings it up to
schema 4, and the build upgrades the edited file the rest of the way. A file
from a newer Hypercolor is refused with a message to upgrade, and the file is
left untouched either way. The floor only moves at a major release.

## Backups

A backup manifest declares its own schema. Schema 1 is the first, written since
v0.9.0. Every 1.x build reads every backup schema from 1 through the one it
writes, upgrading an older manifest in memory before restoring it, so a backup
taken on any 1.x release restores on every later one. A backup from a newer
Hypercolor is refused with a message to upgrade, and nothing is applied.
