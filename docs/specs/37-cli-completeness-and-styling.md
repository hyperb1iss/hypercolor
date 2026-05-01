# Spec 37 — CLI Completeness, Styling Parity, and Remote Control

> Implementation-ready specification for completing the `hypercolor` CLI as a full
> control surface over the Hypercolor daemon, replacing hardcoded ANSI escapes
> with an opaline-themed `Painter` that matches the TUI, and turning remote
> operation from a flag-heavy chore into a first-class experience with named
> connection profiles and WebSocket streaming subscriptions.

**Status:** Draft
**Author:** Nova
**Date:** 2026-04-09
**Crates:** `hypercolor-cli`, `hypercolor-daemon` (minor)
**Related:** `docs/specs/10-rest-websocket-api.md`, `docs/specs/15-cli-commands.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Design Principles](#4-design-principles)
5. [Target Architecture](#5-target-architecture)
6. [Command Inventory](#6-command-inventory)
7. [Coverage Matrix](#7-coverage-matrix)
8. [Connection Profiles](#8-connection-profiles)
9. [Styling: The Painter Module](#9-styling-the-painter-module)
10. [WebSocket Streaming Client](#10-websocket-streaming-client)
11. [Dependency Changes](#11-dependency-changes)
12. [Migration Plan](#12-migration-plan)
13. [Verification Strategy](#13-verification-strategy)
14. [Open Questions](#14-open-questions)
15. [Recommendation](#15-recommendation)

---

## 1. Overview

Hypercolor's daemon is the most complete layer in the project. It exposes
roughly 95 REST endpoints across 12 route groups, a WebSocket with six
streaming channels, and a 14-tool MCP server. The `hypercolor` CLI, in contrast,
covers only the happy-path fraction of that surface, renders output with
hardcoded ANSI escape codes that bypass the opaline theme engine the TUI
already uses, and treats remote operation as an afterthought that requires
`--host` and `--port` on every invocation.

This spec closes all three gaps in a single coordinated initiative:

- **Coverage.** Every REST endpoint that makes sense as a scriptable operation
  gets a `hypercolor` subcommand. Attachments, logical devices, effect-layout
  associations, live control patching, brightness, audio devices, and the
  handful of smaller orphans all become first-class commands.
- **Styling.** The CLI adopts `opaline` with a dedicated `Painter` struct that
  mirrors the unifly pattern, resolves themes from the same order as the TUI,
  and exposes semantic helpers (`success`, `error`, `name`, `state`, `number`,
  `id`, `muted`, `keyword`) that every command handler uses instead of
  `\x1b[38;2;...` literals.
- **Remote polish.** Connection profiles live in a new TOML config file, so
  `hypercolor --profile home status` replaces `hypercolor --host 192.168.1.42 --port
9420 --api-key $SECRET status`. A WebSocket streaming client powers
  subscription commands like `hypercolor status --watch`, `hypercolor effects watch`,
  `hypercolor stream frames`, and `hypercolor stream events`.

The result is a CLI that can drive every aspect of a local or remote daemon,
looks and feels identical to the TUI, and works over the network without
ceremony.

---

## 2. Problem Statement

### 2.1 Current Coverage Gaps

An audit of `crates/hypercolor-cli/src/commands/` against
`crates/hypercolor-daemon/src/api/` found the following endpoint groups with
zero CLI exposure:

- **Attachments.** Full CRUD for `GET/POST/PUT/DELETE /attachments/templates`,
  plus `GET /attachments/categories` and `GET /attachments/vendors`. The
  per-device routes (`PUT/DELETE /devices/{id}/attachments`, `POST
/devices/{id}/attachments/preview`, `POST
/devices/{id}/attachments/{slot_id}/identify`) are also absent.
- **Logical devices.** Global CRUD
  (`GET/POST/PUT/DELETE /logical-devices{,/{id}}`) and per-device views (`GET
/devices/{id}/logical-devices`, `POST /devices/{id}/logical-devices`) have
  no CLI equivalents.
- **Effect layout associations.** `GET/PUT/DELETE /effects/{id}/layout` —
  linking a specific effect to a preferred layout — is entirely absent.
- **Live control patching.** `PATCH /effects/current/controls` and `POST
/effects/current/reset`. Today the only way to change controls is
  `hypercolor effects activate <name> --param ...`, which re-applies the whole
  effect rather than patching the running instance.
- **Effect rescan.** `POST /effects/rescan` — triggers a reload of the effect
  library without restarting the daemon.
- **Zone and slot identify.** `POST /devices/{id}/zones/{zone_id}/identify`
  and `POST /devices/{id}/attachments/{slot_id}/identify`.
- **Settings.** `GET/PUT /settings/brightness` and `GET /audio/devices`.
- **Server identity.** `GET /server` — returns daemon version, name, and
  capability flags.
- **Layout extras.** `GET /layouts/active` and `PUT /layouts/active/preview`.
- **Preset and playlist updates.** `PUT /library/presets/{id}` and
  `PUT /library/playlists/{id}` — the CLI can create and delete but not edit.

The audit also confirmed that `hypercolor status --watch` polls the REST `status`
endpoint on an interval, because the CLI has no WebSocket client at all. Every
watch command in the current CLI is a polling loop.

### 2.2 Current Styling Friction

`crates/hypercolor-cli/src/output.rs:62-93` contains three methods
(`success`, `error`, `warning`) that emit ANSI escape codes with hardcoded RGB
triplets: `\x1b[38;2;80;250;123m` for success, `\x1b[38;2;255;99;99m` for
error, `\x1b[38;2;241;250;140m` for warning. There is no dependency on
`opaline`, no theme resolution, and no way for a user to retheme the CLI.

Meanwhile `crates/hypercolor-tui/src/theme.rs:8-18` already initializes
opaline with the `silkcircuit-neon` theme and exposes accessor functions
(`accent_primary`, `text_muted`, `border_focused`) that read from
`opaline::current()`. The TUI looks exactly how the project wants it to look.
The CLI looks almost the same only because the RGB values in `output.rs`
happen to match the theme tokens at the moment the code was written — which
means any palette change in opaline will drift the two surfaces apart.

The two surfaces have the same design language, no shared code, and a
latent divergence problem.

### 2.3 Current Remote Friction

`DaemonClient::new(&cli.host, cli.port, cli.api_key.as_deref())` already
works over HTTP with bearer-token auth, so remote daemon control is
mechanically possible today. What is missing is ergonomics:

- Every invocation requires `--host`, `--port`, and `--api-key` (or matching
  env vars). There is no way to say "my living room daemon" once and reuse it.
- The only automated way to pick a daemon is `hypercolor servers discover`, which
  scans mDNS and prints a list. There is no "pick one and remember it."
- No WebSocket client exists, so watching state from a remote daemon means
  HTTP polling — which is wasteful over a network and introduces latency on
  top of the existing REST round-trip.

For a project whose core value proposition is reactive real-time lighting,
that is a surprisingly weak remote story.

---

## 3. Goals and Non-Goals

### Goals

- `hypercolor` exposes every daemon REST endpoint that has a meaningful CLI shape.
- `hypercolor` uses `opaline` for all colored output, via a `Painter` struct that
  centralizes theme access and provides semantic helpers.
- CLI and TUI share theme resolution order: `--theme` flag → `HYPERCOLOR_THEME`
  env → CLI config `defaults.theme` → `silkcircuit-neon` default.
- Connection profiles live in `~/.config/hypercolor/cli.toml`. Users invoke
  remote daemons by name: `hypercolor --profile home effects list`.
- A WebSocket streaming client drives `--watch` variants and new `stream`
  subcommands without polling.
- Exit codes are stable and documented so scripts can rely on them.
- Shell-completion generation remains functional and picks up all new
  subcommands automatically.

### Non-Goals

- No changes to the daemon's REST or WebSocket wire format. This is purely a
  client-side completion initiative.
- No redesign of the command taxonomy. Existing subcommands keep their names
  and arguments. New commands slot into the existing tree.
- No introduction of a persistent CLI daemon, shell, or REPL. Each invocation
  is still a one-shot process.
- No coupling between the CLI and `hypercolor-tui`. The two share opaline
  theme data but not code modules. Each depends directly on opaline.
- No MCP client in the CLI. The MCP server stays for AI tooling.
- No credential vault integration (keyring, gnome-keychain, etc.) in this
  spec. Profiles store API keys in plain TOML for v1 with appropriate file
  permissions; credential vaulting is deferred to a follow-up.

---

## 4. Design Principles

**One binary, many surfaces.** `hypercolor` is the scripting surface; `hypercolor-daemon`
is the execution surface; `hypercolor tui` is the interactive surface.
All three should feel like one product. Shared theming via opaline is the
mechanism that makes this real.

**Command handlers are thin.** A subcommand handler should parse its clap
args, call one or two `DaemonClient` methods, and render the result through
an `OutputContext` that knows about the `Painter`. Any logic more complex
than that belongs in the daemon, not the CLI.

**Machine-readable first.** Every command must support `--json`. Human
styling is the default but never the only path. This is already the contract
for existing commands and is reaffirmed here for new ones.

**Semantic color tokens, not hex.** Command handlers never reach into
`opaline::Theme` directly, never hold a `Color`, never call `owo_fg`. They
call `painter.name(&device.name)`, `painter.state("online")`,
`painter.number(&format!("{port}"))`. The meaning of "name" or "state" can
change centrally without touching every command.

**Streaming is first-class.** The moment a command needs to react to daemon
state, it opens a WebSocket subscription rather than polling. Polling is a
fallback, not the default.

**Profiles over environment variables.** Environment variables are fine for
CI but miserable for interactive use. Named profiles in a TOML file give
users a stable vocabulary for their daemons.

---

## 5. Target Architecture

### 5.1 Crate Layout

```
crates/hypercolor-cli/
├── Cargo.toml
└── src/
    ├── main.rs                  # Clap root, dispatch, tracing init
    ├── client/
    │   ├── mod.rs               # Re-exports
    │   ├── http.rs              # Current DaemonClient, renamed HttpClient
    │   └── ws.rs                # NEW: WebSocket streaming client
    ├── config/
    │   ├── mod.rs               # Config loading and profile resolution
    │   └── profiles.rs          # NEW: Profile struct + TOML schema
    ├── output/
    │   ├── mod.rs               # OutputContext, format dispatch
    │   ├── painter.rs           # NEW: Opaline-backed Painter
    │   └── table.rs             # Table renderer (extracted from output.rs)
    └── commands/
        ├── mod.rs
        ├── status.rs
        ├── devices.rs
        ├── effects.rs
        ├── scenes.rs
        ├── profiles.rs          # Daemon profiles, not CLI connection profiles
        ├── library.rs
        ├── layouts.rs
        ├── config.rs
        ├── service.rs
        ├── diagnose.rs
        ├── servers.rs
        ├── completions.rs
        ├── attachments.rs       # NEW
        ├── logical.rs           # NEW: logical-devices subcommand tree
        ├── brightness.rs        # NEW
        ├── audio.rs             # NEW
        ├── server.rs            # NEW: `hypercolor server info`
        └── stream.rs            # NEW: `hypercolor stream frames|events|metrics|spectrum`
```

The `output.rs` file as it exists today is split into three modules:
`output/mod.rs` keeps `OutputContext` and format dispatch, `output/painter.rs`
is new and owns all color logic, `output/table.rs` holds the table renderer
currently at `output.rs:115-162`. The existing `src/client.rs` is relocated
into `client/http.rs` and the module becomes a directory.

### 5.2 Execution Flow

```
 ┌──────────────┐
 │   clap parse │
 └──────┬───────┘
        │
        ▼
 ┌──────────────────┐         ┌─────────────────┐
 │  config::load    │────────▶│  cli.toml       │
 │  (resolve profile)│         │  profiles table │
 └──────┬───────────┘         └─────────────────┘
        │
        ▼
 ┌──────────────────┐         ┌─────────────────┐
 │ Painter::new     │────────▶│  opaline theme  │
 │ (resolve theme)  │         │  silkcircuit    │
 └──────┬───────────┘         └─────────────────┘
        │
        ▼
 ┌──────────────────┐
 │ OutputContext    │
 │ {format, painter,│
 │  quiet, ...}     │
 └──────┬───────────┘
        │
        ▼
 ┌──────────────────┐         ┌─────────────────┐
 │ HttpClient::new  │────────▶│ reqwest         │
 │ WsClient::new    │────────▶│ tokio-tungstenite│
 └──────┬───────────┘         └─────────────────┘
        │
        ▼
 ┌──────────────────┐
 │ commands::X::    │
 │   execute(args,  │
 │    ctx, http, ws)│
 └──────────────────┘
```

Every command handler receives the same tuple: the parsed args for its
subcommand, an `&OutputContext` (which already carries the `Painter`), an
`&HttpClient`, and an `&WsClient`. Commands that do not need WebSocket simply
ignore the `WsClient`. Commands that need streaming do not construct their
own; they reuse the shared one.

### 5.3 Theme Resolution Order

Identical between CLI and TUI:

1. `--theme <name>` command-line flag (global)
2. `HYPERCOLOR_THEME` environment variable
3. `defaults.theme` in `~/.config/hypercolor/cli.toml`
4. Hardcoded default: `"silkcircuit-neon"`

If a theme name cannot be resolved (opaline returns `None`), fall back to the
hardcoded default and emit a warning through the painter's own warning path.
Never panic on theme resolution.

### 5.4 Output Context Changes

`OutputContext` gains a `painter: Painter` field and drops its inline ANSI
helpers. The methods `success`, `error`, `warning`, and `info` still exist on
`OutputContext` (so existing call sites compile unchanged after a targeted
renaming pass), but internally they delegate to the painter:

```rust
pub struct OutputContext {
    pub format: OutputFormat,
    pub quiet: bool,
    pub painter: Painter,
}

impl OutputContext {
    pub fn success(&self, msg: &str) {
        if self.quiet { return; }
        println!("  {} {}", self.painter.success_icon(), msg);
    }
    // error / warning / info analogously
}
```

Tables call `ctx.painter` directly when they need to style individual cells.
JSON output never touches the painter.

---

## 6. Command Inventory

This section is the authoritative list of every `hypercolor` subcommand after
this spec lands. Existing commands are marked **(existing)**; new commands
are marked **(new)**. Subcommands that are partially implemented today are
marked **(existing, extended)** with a note about what changes.

### 6.1 Top-Level Subcommands

| Subcommand    | Status             | Purpose                                                                                         |
| ------------- | ------------------ | ----------------------------------------------------------------------------------------------- |
| `status`      | existing, extended | System overview (extended with streaming `--watch`)                                             |
| `devices`     | existing, extended | Device discovery, info, mutation (extended with attachments and logical subtrees)               |
| `effects`     | existing, extended | Effect list, activate, stop, info (extended with `patch`, `reset`, `rescan`, `layout`, `watch`) |
| `scenes`      | existing           | Scene CRUD and activation                                                                       |
| `profiles`    | existing           | Profile CRUD and application (daemon-side profiles)                                             |
| `library`     | existing, extended | Favorites, presets, playlists (extended with update subcommands)                                |
| `layouts`     | existing, extended | Layout CRUD (extended with `active` and `preview`)                                              |
| `attachments` | **new**            | Attachment templates, categories, vendors                                                       |
| `brightness`  | **new**            | Global output brightness                                                                        |
| `audio`       | **new**            | Audio input device listing                                                                      |
| `server`      | **new**            | Daemon identity, version, capabilities                                                          |
| `stream`      | **new**            | Raw WebSocket channel dumps (frames, events, metrics, spectrum)                                 |
| `config`      | existing           | CLI configuration (existing) plus daemon config (existing, same subcommand tree)                |
| `service`     | existing           | Daemon service lifecycle (systemd, launchd, SC)                                                 |
| `diagnose`    | existing           | Health checks and reports                                                                       |
| `servers`     | existing, extended | mDNS discovery (extended with `adopt` to write the selected server to `cli.toml`)               |
| `completions` | existing           | Shell completion generation                                                                     |

### 6.2 Extended Subcommand: `effects`

```
hypercolor effects list                              # (existing)
hypercolor effects info <name>                       # (existing)
hypercolor effects activate <name> [--param ...]     # (existing)
hypercolor effects stop                              # (existing)
hypercolor effects patch --param key=value ...       # (new)  PATCH /effects/current/controls
hypercolor effects reset                             # (new)  POST  /effects/current/reset
hypercolor effects rescan                            # (new)  POST  /effects/rescan
hypercolor effects layout show <name>                # (new)  GET   /effects/{id}/layout
hypercolor effects layout set <name> <layout-id>     # (new)  PUT   /effects/{id}/layout
hypercolor effects layout clear <name>               # (new)  DELETE /effects/{id}/layout
hypercolor effects watch                             # (new)  WS events channel, filters to effect events
```

`patch` accepts the same `--param key=value` syntax as `activate` but
translates to a `PATCH` instead of a fresh `apply`, so controls change
without interrupting the render loop.

`rescan` is a thin wrapper around the POST endpoint and prints the number of
effects discovered, comparable to the existing `devices discover` output
shape.

### 6.3 Extended Subcommand: `devices`

```
hypercolor devices list [--backend-id ...] [--driver ...]        # (existing)
hypercolor devices info <id>                                     # (existing)
hypercolor devices discover [--target ...] [--timeout ...]       # (existing)
hypercolor devices pair <id>                                     # (existing)
hypercolor devices unpair <id>                                   # (new)     DELETE /devices/{id}/pair
hypercolor devices identify <id>                                 # (existing)
hypercolor devices identify <id> zone <zone-id>                  # (new)     POST   /devices/{id}/zones/{zone_id}/identify
hypercolor devices identify <id> slot <slot-id>                  # (new)     POST   /devices/{id}/attachments/{slot_id}/identify
hypercolor devices set-color <id> <color>                        # (existing)
hypercolor devices delete <id>                                   # (new)     DELETE /devices/{id}
hypercolor devices update <id> [--name ...] [--enabled ...]      # (new)     PUT    /devices/{id}
hypercolor devices attachments show <id>                         # (new)     GET    /devices/{id}/attachments
hypercolor devices attachments set <id> --slot <n> --template <t># (new)     PUT    /devices/{id}/attachments
hypercolor devices attachments preview <id> --slot ...           # (new)     POST   /devices/{id}/attachments/preview
hypercolor devices attachments clear <id>                        # (new)     DELETE /devices/{id}/attachments
hypercolor devices logical list <id>                             # (new)     GET    /devices/{id}/logical-devices
hypercolor devices logical create <id> --name ... --zones ...    # (new)     POST   /devices/{id}/logical-devices
```

`devices identify` becomes a group command. The bare form
(`hypercolor devices identify <id>`) keeps its existing meaning for backward
compatibility; `zone` and `slot` are new positional subcommand variants.

### 6.4 New Subcommand: `attachments`

```
hypercolor attachments templates list                                   # GET    /attachments/templates
hypercolor attachments templates info <id>                              # GET    /attachments/templates/{id}
hypercolor attachments templates create --name ... --category ... ...   # POST   /attachments/templates
hypercolor attachments templates update <id> [--name ...] [...]         # PUT    /attachments/templates/{id}
hypercolor attachments templates delete <id>                            # DELETE /attachments/templates/{id}
hypercolor attachments categories                                       # GET    /attachments/categories
hypercolor attachments vendors                                          # GET    /attachments/vendors
```

Template creation requires at least a name and category; all other fields
are optional and map directly onto the daemon's template schema. The exact
JSON body shape lives in
`crates/hypercolor-daemon/src/api/attachments.rs` and is not reproduced
here to avoid drift.

### 6.5 New Subcommand: `logical`

```
hypercolor logical list                                                 # GET    /logical-devices
hypercolor logical info <id>                                            # GET    /logical-devices/{id}
hypercolor logical create --device <id> --name ... --zones ...          # POST   /logical-devices
hypercolor logical update <id> [--name ...] [--zones ...]               # PUT    /logical-devices/{id}
hypercolor logical delete <id>                                          # DELETE /logical-devices/{id}
```

Logical devices are cross-device LED segment groupings. The `--zones`
argument takes a comma-separated list of `device_id:zone_id` pairs.

### 6.6 Extended Subcommand: `layouts`

```
hypercolor layouts list                                        # (existing)
hypercolor layouts show <id>                                   # (existing)
hypercolor layouts update <id>                                 # (existing)
hypercolor layouts create --name ... --file <path>             # (new)     POST   /layouts
hypercolor layouts delete <id>                                 # (new)     DELETE /layouts/{id}
hypercolor layouts active                                      # (new)     GET    /layouts/active
hypercolor layouts apply <id>                                  # (new)     POST   /layouts/{id}/apply
hypercolor layouts preview <id>                                # (new)     PUT    /layouts/active/preview
```

### 6.7 Extended Subcommand: `library`

```
hypercolor library favorites list                             # (existing)
hypercolor library favorites add <effect>                     # (existing)
hypercolor library favorites remove <effect>                  # (existing)
hypercolor library presets list                               # (existing)
hypercolor library presets info <id>                          # (existing)
hypercolor library presets create ...                         # (existing)
hypercolor library presets update <id> ...                    # (new)   PUT    /library/presets/{id}
hypercolor library presets apply <id>                         # (existing)
hypercolor library presets delete <id>                        # (existing)
hypercolor library playlists list                             # (existing)
hypercolor library playlists info <id>                        # (existing)
hypercolor library playlists create ...                       # (existing)
hypercolor library playlists update <id> ...                  # (new)   PUT    /library/playlists/{id}
hypercolor library playlists activate <id>                    # (existing)
hypercolor library playlists active                           # (existing)
hypercolor library playlists stop                             # (existing)
hypercolor library playlists delete <id>                      # (existing)
```

### 6.8 New Subcommand: `brightness`

```
hypercolor brightness get                                     # GET /settings/brightness
hypercolor brightness set <value>                             # PUT /settings/brightness  (value: 0-100)
```

### 6.9 New Subcommand: `audio`

```
hypercolor audio devices                                      # GET /audio/devices
```

Lists available audio input devices and marks the currently selected one.
Setting the audio source lives in `hypercolor config set audio.device ...` using
the existing config tree, so this subcommand is read-only.

### 6.10 New Subcommand: `server`

```
hypercolor server info                                        # GET /server
hypercolor server health                                      # GET /health
```

Replaces the current implicit "query the daemon to see if it's up" pattern
with explicit commands that script authors can rely on.

### 6.11 New Subcommand: `stream`

```
hypercolor stream frames   [--zone <id>] [--format rgb|rgba] [--fps <n>]
hypercolor stream events   [--filter <type>]
hypercolor stream metrics  [--fps <n>]
hypercolor stream spectrum [--bins 8|16|32|64|128] [--fps <n>]
hypercolor stream canvas   [--format rgb|rgba] [--fps <n>]
```

Each `stream` subcommand opens the corresponding WebSocket channel and
renders output as the chosen format. For `--json`, each message is emitted
as a newline-delimited JSON object. For `--format table` (the default),
the subcommand renders a live one-line-per-message feed using the painter
for consistency. Streaming commands exit on SIGINT or when the WebSocket
closes; they never time out on their own.

`stream canvas` is a special case because canvas payloads are binary. In
table mode it prints frame-arrival metadata (timestamp, byte length, format,
fps) rather than trying to render pixel data to the terminal. In JSON mode
it base64-encodes the canvas bytes into the message envelope. A future
extension could render ASCII art previews, but that is out of scope here.

### 6.12 Extended Subcommand: `servers`

```
hypercolor servers discover [--timeout <s>]                   # (existing)
hypercolor servers adopt <instance-name> [--as <profile>]     # (new)
```

`adopt` takes the instance name from a previous `discover` run and writes
a new profile entry into `cli.toml`. The `--as` flag sets the profile name
(defaults to the instance name). This closes the loop from discovery to
persistent use without manual TOML editing.

### 6.13 Extended Subcommand: `status`

```
hypercolor status                                             # (existing)
hypercolor status --watch [--interval <s>]                    # (existing, reimplemented over WS)
```

The `--watch` flag is reimplemented on top of the `events` channel, so
updates arrive the moment they happen on the daemon rather than on the
poll interval. The `--interval` flag is preserved but becomes advisory:
the command refreshes its rendered view at most once per interval even if
more events arrive, to keep the terminal readable.

---

## 7. Coverage Matrix

### 7.1 Before Spec 37

| Domain              | REST endpoints |  CLI coverage |
| ------------------- | -------------: | ------------: |
| Devices (core)      |             18 |             9 |
| Logical devices     |              8 |             0 |
| Attachments         |              9 |             0 |
| Effects             |             11 |             4 |
| Scenes              |              6 |             6 |
| Profiles            |              6 |             6 |
| Library (favorites) |              3 |             3 |
| Library (presets)   |              6 |             5 |
| Library (playlists) |              8 |             7 |
| Layouts             |              8 |             3 |
| Status / server     |              4 |             1 |
| Settings            |              3 |             0 |
| Config              |              4 |             4 |
| Diagnose            |              1 |             1 |
| WebSocket channels  |              6 |             0 |
| **Total**           |        **101** | **49 (~49%)** |

(The ~70% figure from the audit counted "partially covered" domains as
covered; this matrix uses strict endpoint-level coverage.)

### 7.2 After Spec 37

| Domain              | REST endpoints |   CLI coverage |
| ------------------- | -------------: | -------------: |
| Devices (core)      |             18 |             18 |
| Logical devices     |              8 |              8 |
| Attachments         |              9 |              9 |
| Effects             |             11 |             11 |
| Scenes              |              6 |              6 |
| Profiles            |              6 |              6 |
| Library (favorites) |              3 |              3 |
| Library (presets)   |              6 |              6 |
| Library (playlists) |              8 |              8 |
| Layouts             |              8 |              8 |
| Status / server     |              4 |              4 |
| Settings            |              3 |              3 |
| Config              |              4 |              4 |
| Diagnose            |              1 |              1 |
| WebSocket channels  |              6 |              5 |
| **Total**           |        **101** | **100 (~99%)** |

The one WebSocket channel without first-class CLI coverage is
`screen_canvas`, which is nearly identical to `canvas` and is best served by
adding a `--source screen` flag to `hypercolor stream canvas` rather than a
dedicated subcommand. The debug endpoints `/devices/debug/queues` and
`/devices/debug/routing` remain intentionally unexposed; they are daemon
internals, not user-facing surfaces.

---

## 8. Connection Profiles

### 8.1 File Location

`~/.config/hypercolor/cli.toml` — platform-appropriate location via the
existing `dirs` dependency. On Linux that is `$XDG_CONFIG_HOME/hypercolor/`,
on macOS `~/Library/Application Support/hypercolor/`, on Windows
`%APPDATA%\hypercolor\`.

The file is created lazily: if absent, `hypercolor` uses compiled-in defaults.
If a profile flag is supplied and the file does not exist,
`hypercolor --profile home status` exits with code `2` and a message pointing to
`hypercolor servers adopt` as the usual creation path.

### 8.2 Schema

```toml
# ~/.config/hypercolor/cli.toml

[defaults]
profile = "local"         # which profile to use when --profile is not given
theme   = "silkcircuit-neon"
format  = "table"         # table | json | plain
color   = "auto"          # auto | always | never

[profiles.local]
host    = "localhost"
port    = 9420
api_key = ""              # empty string = no auth

[profiles.home]
host    = "hypercolor.lan"
port    = 9420
api_key = "hck_live_..."

[profiles.living-room]
host    = "192.168.1.42"
port    = 9420
api_key = "hck_live_..."
# Optional metadata, displayed by `hypercolor servers list-profiles`
label       = "Living Room (Razer + Hue)"
description = "RGB scenery for the main space"
```

Profile names follow the same rules as clap arg values: alphanumerics,
hyphens, underscores, no spaces. A profile may omit any field; missing
fields inherit from the built-in defaults (localhost:9420, no auth).

### 8.3 Resolution

When `hypercolor` starts:

1. Load `cli.toml` if present.
2. Determine active profile: `--profile <name>` flag, else
   `HYPERCOLOR_PROFILE` env var, else `defaults.profile`, else `"local"`.
3. Merge profile fields with global flags in this order of precedence
   (highest wins): explicit `--host`/`--port`/`--api-key` flags → env vars
   (`HYPERCOLOR_HOST`, `HYPERCOLOR_PORT`, `HYPERCOLOR_API_KEY`) → profile
   fields → compiled-in defaults.
4. Construct the `HttpClient` and `WsClient` from the merged values.

This gives users a clean hierarchy: profiles are durable, env vars are
per-shell overrides, flags are per-invocation overrides.

### 8.4 Profile Management Commands

```
hypercolor config profile list                  # enumerate profiles
hypercolor config profile show [name]           # show a profile's settings (active if omitted)
hypercolor config profile add <name> --host ... # add a new profile
hypercolor config profile set <name> <key> <value>
hypercolor config profile remove <name>
hypercolor config profile default <name>        # set defaults.profile
```

These live under the existing `config` subcommand tree rather than being
top-level, because they manipulate CLI config rather than daemon state.

### 8.5 File Permissions

On Unix platforms, `cli.toml` is created with mode `0600` (owner
read/write only) because it may contain API keys. On Windows, ACLs default
to the current user. The file is never written to stdout or logged.

---

## 9. Styling: The Painter Module

### 9.1 Dependencies

Add to `crates/hypercolor-cli/Cargo.toml` `[dependencies]`:

```toml
opaline     = { workspace = true, features = ["global-state"] }
owo-colors  = { workspace = true }
```

The `opaline` workspace entry already exists for the TUI; it is referenced
here. `owo-colors` is a new workspace dep; add it to the root `Cargo.toml`
workspace dependencies as well.

### 9.2 Painter Struct

`crates/hypercolor-cli/src/output/painter.rs`:

```rust
//! Opaline-backed terminal painter for the Hypercolor CLI.
//!
//! All colored output flows through this module. Semantic helpers map
//! domain concepts to opaline theme tokens so themes can change centrally
//! without touching every command handler.

use opaline::adapters::owo_colors::OwoThemeExt;
use owo_colors::OwoColorize;

pub struct Painter {
    theme: opaline::Theme,
    enabled: bool,
}

impl Painter {
    /// Construct a painter from resolved CLI options.
    pub fn new(theme_name: Option<&str>, enabled: bool) -> Self {
        let theme = Self::load_theme(theme_name);
        Self { theme, enabled }
    }

    /// Plain (no-color) painter. Used when --no-color or NO_COLOR is set.
    pub fn plain() -> Self {
        Self {
            theme: Self::load_theme(None),
            enabled: false,
        }
    }

    fn load_theme(name: Option<&str>) -> opaline::Theme {
        let resolved = name.unwrap_or("silkcircuit-neon");
        opaline::load_by_name(resolved).unwrap_or_else(|| {
            opaline::load_by_name("silkcircuit-neon")
                .expect("builtin silkcircuit-neon theme must exist")
        })
    }

    pub fn is_enabled(&self) -> bool { self.enabled }

    fn paint(&self, text: &str, token: &str) -> String {
        if self.enabled {
            format!("{}", text.style(self.theme.owo_fg(token)))
        } else {
            text.to_string()
        }
    }

    // ── Semantic helpers ───────────────────────────────────────────────

    pub fn name(&self, text: &str)    -> String { self.paint(text, "accent.secondary") }
    pub fn keyword(&self, text: &str) -> String { self.paint(text, "accent.primary") }
    pub fn number(&self, text: &str)  -> String { self.paint(text, "code.number") }
    pub fn id(&self, text: &str)      -> String { self.paint(text, "text.dim") }
    pub fn muted(&self, text: &str)   -> String { self.paint(text, "text.muted") }
    pub fn success(&self, text: &str) -> String { self.paint(text, "success") }
    pub fn error(&self, text: &str)   -> String { self.paint(text, "error") }
    pub fn warning(&self, text: &str) -> String { self.paint(text, "warning") }

    // ── Domain helpers ─────────────────────────────────────────────────

    /// Device connection state.
    pub fn device_state(&self, state: &str) -> String {
        match state.to_ascii_lowercase().as_str() {
            "online" | "connected" | "ready"       => self.success(state),
            "offline" | "disconnected" | "missing" => self.error(state),
            "paired" | "pairing"                   => self.warning(state),
            _                                      => self.muted(state),
        }
    }

    /// Effect activity state.
    pub fn effect_state(&self, state: &str) -> String {
        match state.to_ascii_lowercase().as_str() {
            "running" | "active" => self.success(state),
            "stopped" | "idle"   => self.muted(state),
            "error" | "failed"   => self.error(state),
            _                    => self.warning(state),
        }
    }

    /// Boolean display with colored yes/no.
    pub fn yesno(&self, value: bool) -> String {
        if value { self.success("yes") } else { self.error("no") }
    }

    // ── Status icons ───────────────────────────────────────────────────

    pub fn success_icon(&self) -> String { self.paint("\u{2726}", "success") }
    pub fn error_icon(&self)   -> String { self.paint("\u{2717}", "error") }
    pub fn warning_icon(&self) -> String { self.paint("!",       "warning") }
}
```

### 9.3 Token Mapping

The tokens the painter uses are a strict subset of what opaline's
`silkcircuit-neon` theme already ships, so no custom token registration is
needed at CLI startup:

| Painter helper    | Token                              |
| ----------------- | ---------------------------------- |
| `name`            | `accent.secondary` (neon cyan)     |
| `keyword`         | `accent.primary` (electric purple) |
| `number`          | `code.number` (coral)              |
| `id`, `mac`-style | `text.dim`                         |
| `muted`           | `text.muted`                       |
| `success`         | `success` (neon green)             |
| `error`           | `error` (red)                      |
| `warning`         | `warning` (yellow)                 |

If future themes want to override these, they do so in the theme TOML file,
not in the CLI source.

### 9.4 Call-Site Migration

Every existing call to `ctx.success(...)` / `ctx.error(...)` / `ctx.warning(...)`
continues to work. The `OutputContext` method bodies change to use the
painter internally. No command handler needs to import `Painter` directly
for simple status messages.

New usages in table rendering look like:

```rust
let rows: Vec<Vec<String>> = devices.iter().map(|d| {
    vec![
        ctx.painter.id(&d.id),
        ctx.painter.name(&d.name),
        ctx.painter.device_state(&d.status),
        ctx.painter.number(&format!("{}", d.zone_count)),
    ]
}).collect();
ctx.print_table(&["ID", "NAME", "STATUS", "ZONES"], &rows);
```

The existing `print_table` implementation in `output.rs:119-162` handles
colored cells correctly because it aligns on display width, not byte
length. Spec 37 requires verifying this assumption during Phase 1 and, if
necessary, switching the column-width calculation to use the
`unicode-width` crate with ANSI-stripped values.

### 9.5 Disabled State

When the `--no-color` flag is set, `NO_COLOR` env var is defined,
`HYPERCOLOR_COLOR=never` is set, or stdout is not a TTY, the painter is
constructed with `enabled: false`. In that mode every semantic helper
returns its input unchanged, so no ANSI bytes reach the terminal. This is
the only path by which color output can be suppressed; there are no
fallback ANSI literals anywhere in the CLI after Phase 1 completes.

---

## 10. WebSocket Streaming Client

### 10.1 Dependency

Add to `crates/hypercolor-cli/Cargo.toml` `[dependencies]`:

```toml
tokio-tungstenite = { workspace = true, features = ["rustls-tls-native-roots"] }
futures-util      = { workspace = true }
```

`tokio-tungstenite` is already pulled in elsewhere in the workspace; this
adds it to the CLI's direct dependencies.

### 10.2 WsClient Struct

`crates/hypercolor-cli/src/client/ws.rs`:

```rust
//! WebSocket streaming client for the Hypercolor CLI.
//!
//! Wraps tokio-tungstenite with the same host/port/auth resolution as
//! HttpClient, exposes typed subscription handles, and translates
//! daemon messages into serde_json::Value streams for command handlers.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

pub struct WsClient {
    base_url: String,
    api_key: Option<String>,
}

pub struct Subscription {
    inner: /* tokio-tungstenite WebSocketStream wrapper */,
}

impl WsClient {
    pub fn new(host: &str, port: u16, api_key: Option<&str>) -> Self { ... }

    /// Open a subscription on the /ws endpoint with the given channel config.
    pub async fn subscribe(&self, channels: SubscribeRequest) -> anyhow::Result<Subscription> { ... }
}

impl Subscription {
    /// Receive the next JSON message, or None if the stream closed.
    pub async fn next_message(&mut self) -> anyhow::Result<Option<serde_json::Value>> { ... }

    /// Receive the next binary message (for frames/canvas channels).
    pub async fn next_binary(&mut self) -> anyhow::Result<Option<Vec<u8>>> { ... }

    pub async fn close(self) -> anyhow::Result<()> { ... }
}
```

The `SubscribeRequest` type is a thin mirror of the daemon's existing
subscription schema in `crates/hypercolor-daemon/src/api/ws.rs:77-94` and
is shared through a new `hypercolor-types::ws` module so both sides stay in
sync. Adding that types module is the only daemon-side change this spec
requires; the wire format does not change.

### 10.3 Subscription Payloads

Each `stream` subcommand constructs its own `SubscribeRequest`:

- `stream frames` → `frames` channel, fps and zone list from flags
- `stream events` → `events` channel, optional client-side type filter
- `stream metrics` → `metrics` channel, fps from flag
- `stream spectrum` → `spectrum` channel, bins and fps from flags
- `stream canvas` → `canvas` (or `screen_canvas` with `--source screen`)

Commands that only need one channel (`status --watch` uses `events`,
`effects watch` uses `events` filtered to effect events) call
`WsClient::subscribe` once and drop the subscription on Ctrl-C.

### 10.4 Termination and Signals

Streaming commands install a Ctrl-C handler via `tokio::signal::ctrl_c` and
close the WebSocket gracefully on signal. They exit with code `0` on
user-initiated close, `1` on unexpected stream errors, `2` on connection
failure. The exit-code table in Section 11 records these.

### 10.5 Backpressure

The CLI does not need backpressure handling for human-facing streams: if
the terminal cannot keep up with 60 FPS frame metadata, messages queue in
the kernel pipe and eventually the terminal catches up. For `--json`
output piped into a file, the OS handles buffering. The one exception is
`stream canvas` in JSON mode with base64 encoding: on slow storage or
network destinations, the CLI should drop to a configurable FPS ceiling
rather than buffer unbounded bytes. A `--max-fps` flag on `stream canvas`
enforces this; default is 15.

---

## 11. Dependency Changes

### 11.1 New CLI Dependencies

Add to `crates/hypercolor-cli/Cargo.toml` `[dependencies]`:

```toml
opaline           = { workspace = true, features = ["global-state"] }
owo-colors        = { workspace = true }
tokio-tungstenite = { workspace = true, features = ["rustls-tls-native-roots"] }
futures-util      = { workspace = true }
toml              = { workspace = true }
unicode-width     = { workspace = true }
```

### 11.2 Workspace Cargo.toml Additions

If not already present in the workspace `[workspace.dependencies]` section,
add:

```toml
owo-colors        = "4"
unicode-width     = "0.2"
toml              = "0.8"
futures-util      = "0.3"
```

`opaline` and `tokio-tungstenite` are already workspace dependencies used
by other crates; no version changes needed.

### 11.3 Exit Code Table

| Code  | Meaning                                                                   |
| ----- | ------------------------------------------------------------------------- |
| `0`   | Success                                                                   |
| `1`   | Generic command failure (also: HTTP 4xx/5xx response)                     |
| `2`   | Configuration error (missing profile, bad `cli.toml`, unreachable daemon) |
| `3`   | Authentication failure (HTTP 401)                                         |
| `4`   | Not found (HTTP 404, e.g. effect/device/profile id)                       |
| `5`   | Streaming error (WebSocket closed unexpectedly)                           |
| `64`  | Clap argument parse error (default clap value)                            |
| `130` | Interrupted (SIGINT, e.g. during `stream` or `status --watch`)            |

These codes are stable and are part of the CLI's public contract. Scripts
built against any post-Spec-37 `hypercolor` can rely on them.

---

## 12. Migration Plan

This is a large spec, so it lands in six phases. Each phase ends in a
green `just verify` and can ship independently.

### Phase 1: Styling Parity

Files:

- `crates/hypercolor-cli/Cargo.toml` — add `opaline`, `owo-colors`,
  `unicode-width`
- `crates/hypercolor-cli/src/output/painter.rs` — new module
- `crates/hypercolor-cli/src/output/mod.rs` — split from current `output.rs`,
  integrate `Painter` into `OutputContext`
- `crates/hypercolor-cli/src/output/table.rs` — extract table renderer;
  update column width calculation to strip ANSI before measuring width
- `crates/hypercolor-cli/src/main.rs` — pass theme name into painter
  construction; add `--theme` global flag

Tasks:

- Introduce `Painter` with the full semantic helper set
- Replace every `\x1b[38;2;...` literal in the existing command handlers
  with the equivalent `painter.xxx()` call
- Migrate `success` / `error` / `warning` / `info` on `OutputContext` to
  delegate through the painter
- Verify table alignment with colored cells on both Unicode-width and
  ASCII-width inputs

Verification:

- `just verify` passes
- All existing commands render visibly identical output to before when
  the default theme is `silkcircuit-neon`
- Snapshot tests (new) lock in the byte-exact colored output of
  `hypercolor status`, `hypercolor devices list`, `hypercolor effects list`
- `NO_COLOR=1 hypercolor status` produces zero ANSI bytes

### Phase 2: Connection Profiles

Files:

- `crates/hypercolor-cli/src/config/mod.rs` — new module
- `crates/hypercolor-cli/src/config/profiles.rs` — new module
- `crates/hypercolor-cli/src/main.rs` — profile resolution at startup
- `crates/hypercolor-cli/src/commands/config.rs` — add `profile` subtree
- `crates/hypercolor-cli/src/commands/servers.rs` — add `adopt` subcommand

Tasks:

- Define `CliConfig` and `Profile` TOML schemas with serde
- Implement the flag → env → profile → default resolution precedence
- Add `--profile` global flag and `HYPERCOLOR_PROFILE` env var
- Implement `hypercolor config profile {list, show, add, set, remove, default}`
- Implement `hypercolor servers adopt`, writing the selected mDNS instance into
  `cli.toml` with mode `0600` on Unix
- Create `cli.toml` lazily on first write; never on read

Verification:

- `just verify` passes
- Integration test with a temp dir as `$XDG_CONFIG_HOME` exercises the
  full add → show → default → resolve cycle
- Starting `hypercolor` without a `cli.toml` uses compiled-in localhost defaults
- `hypercolor --profile nonexistent` exits with code `2` and a helpful message

### Phase 3: Coverage Catch-Up (REST)

Files:

- `crates/hypercolor-cli/src/commands/attachments.rs` — new
- `crates/hypercolor-cli/src/commands/logical.rs` — new
- `crates/hypercolor-cli/src/commands/brightness.rs` — new
- `crates/hypercolor-cli/src/commands/audio.rs` — new
- `crates/hypercolor-cli/src/commands/server.rs` — new
- `crates/hypercolor-cli/src/commands/effects.rs` — extend with `patch`,
  `reset`, `rescan`, `layout`, `watch` (watch stub until Phase 5)
- `crates/hypercolor-cli/src/commands/devices.rs` — extend with attachments
  subtree, logical subtree, zone/slot identify variants, delete, update,
  unpair
- `crates/hypercolor-cli/src/commands/layouts.rs` — extend with `create`,
  `delete`, `active`, `apply`, `preview`
- `crates/hypercolor-cli/src/commands/library.rs` — extend with presets
  `update` and playlists `update`
- `crates/hypercolor-cli/src/main.rs` — wire new top-level subcommands
- `crates/hypercolor-cli/src/client/http.rs` — add request builders for
  any missing endpoint shapes

Tasks:

- For every new subcommand, add: clap struct, handler function, table
  renderer, JSON renderer, doc strings
- Snapshot tests for the default render of each new command
- Update `hypercolor completions` fixtures if generation is tested

Verification:

- `just verify` passes
- Each new command makes exactly one daemon call (verified by
  `wiremock`-based integration test) except for intentionally compound
  commands like `devices logical create`
- Every new command honors `--json`
- Every new command honors `--no-color`

### Phase 4: Shared WebSocket Types

Files:

- `crates/hypercolor-types/src/ws.rs` — new module exposing
  `SubscribeRequest`, `ChannelConfig`, `ChannelKind`, `WsMessage`
- `crates/hypercolor-types/src/lib.rs` — re-export
- `crates/hypercolor-daemon/src/api/ws.rs` — migrate its existing
  subscription structs to `hypercolor_types::ws::*` re-exports, maintaining
  wire-format identity

Tasks:

- Audit the daemon's current WebSocket subscription types and move them
  into `hypercolor-types`
- Add derive(Serialize, Deserialize) with the same `#[serde(...)]`
  attributes that the daemon currently uses
- Verify wire format by running the daemon's existing WebSocket tests
  unchanged

Verification:

- `just verify` passes
- `just test-crate hypercolor-daemon` — no regressions in WebSocket tests
- Manual smoke test with a browser-based WS client against a local daemon

### Phase 5: Streaming CLI Commands

Files:

- `crates/hypercolor-cli/src/client/ws.rs` — new module
- `crates/hypercolor-cli/src/commands/stream.rs` — new subcommand tree
- `crates/hypercolor-cli/src/commands/status.rs` — reimplement `--watch`
  over WebSocket
- `crates/hypercolor-cli/src/commands/effects.rs` — implement `watch`
  subcommand (stub from Phase 3)

Tasks:

- Implement `WsClient` and `Subscription` types
- Wire Ctrl-C handling via `tokio::signal::ctrl_c`
- Implement `stream frames|events|metrics|spectrum|canvas`
- Reimplement `status --watch` and `effects watch` on top of `events`
- Add `--max-fps` to `stream canvas`
- Add integration tests with an in-process daemon that exercise the
  subscribe → receive → close cycle

Verification:

- `just verify` passes
- `hypercolor stream events` against a local daemon receives a scene activation
  broadcast within one second of triggering it from another terminal
- `hypercolor status --watch` updates on event rather than on timer
- `hypercolor stream canvas --max-fps 5` never exceeds five messages per second
  in JSON mode

### Phase 6: Polish and Documentation

Files:

- `docs/specs/15-cli-commands.md` — append a "Spec 37 delta" section
  listing the new commands
- `crates/hypercolor-cli/src/commands/completions.rs` — verify generation
  picks up new subcommands without source changes
- `README.md` — update CLI section if it lists commands
- Man page or help-text polish for any command whose doc strings are thin

Tasks:

- Regenerate shell completion fixtures (if committed)
- Review every new subcommand's `--help` output
- Update any project-level CLI documentation

Verification:

- `just verify` passes
- `hypercolor completions bash | bash` loads cleanly in a subshell
- Every new command's help text reads well and matches the section
  definitions in this spec

---

## 13. Verification Strategy

### 13.1 Unit Tests

Files: `crates/hypercolor-cli/tests/*.rs`

- `painter_tests.rs` — every semantic helper returns uncolored input when
  disabled; returns colored output with expected ANSI when enabled;
  domain helpers (`device_state`, `effect_state`, `yesno`) map inputs
  correctly
- `config_profile_tests.rs` — profile resolution precedence, missing
  profile errors, file permissions on Unix, serde round-trip for
  `CliConfig`
- `output_table_tests.rs` — column width math handles ANSI bytes
  correctly; `--no-color` mode produces zero escape sequences

### 13.2 Integration Tests

Files: `crates/hypercolor-cli/tests/integration_*.rs`

- `integration_http.rs` — spin up the daemon in-process and exercise every
  new subcommand end-to-end; assert JSON response shapes and exit codes
- `integration_ws.rs` — subscribe via `WsClient`, emit daemon events,
  verify the CLI receives them

Both suites reuse the existing `hypercolor-daemon` dev-dependency pattern
already present in `crates/hypercolor-cli/Cargo.toml:30-34`.

### 13.3 Snapshot Tests

Use `insta` for byte-exact snapshots of colored output on a small set of
canonical commands:

- `hypercolor status`
- `hypercolor devices list`
- `hypercolor effects list`
- `hypercolor stream events` (first five lines against a scripted event
  sequence)

Snapshot tests catch accidental theme drift early and force intentional
review on any visual change.

### 13.4 Manual Verification

Before merging each phase:

- Run `hypercolor` against a real local daemon on the dev machine
- Run `hypercolor --profile remote` against a daemon on a second machine over
  the LAN
- Run every `stream` subcommand for at least 30 seconds and Ctrl-C out
  cleanly
- Confirm `NO_COLOR=1 hypercolor status` is visually monochrome

### 13.5 Regression Guards

- `just verify` on every push
- Clippy pedantic is already enforced; new command modules must pass
- `cargo deny` (runs in CI) catches any license or advisory issues from
  new deps (`opaline`, `owo-colors`, `tokio-tungstenite`, `futures-util`,
  `toml`, `unicode-width`)

---

## 14. Open Questions

The following choices are deliberately left for implementation or for a
brief discussion before Phase 1 starts. None of them blocks the spec's
shape.

1. **Should `hypercolor --profile X` also accept bare positional shorthand?**
   For example, `hypercolor @home effects list` as sugar for
   `hypercolor --profile home effects list`. It reads nicely but costs a clap
   custom parser. The spec assumes `--profile` is the only form.
2. **Should `stream canvas` render ASCII art by default?** Terminal image
   rendering is a nice party trick but is out of scope here. The spec
   assumes table-mode `stream canvas` prints metadata only.
3. **Does `hypercolor audio set <device>` belong as a proper subcommand?**
   The spec routes audio device selection through `hypercolor config set
audio.device ...` on the grounds that it is CLI config, not an API
   call. If the daemon grows a dedicated `PUT /audio/device` endpoint,
   we add a subcommand then.
4. **Should `hypercolor effects patch` support `--json-patch` for complex
   nested control shapes?** The daemon's `PATCH /effects/current/controls`
   accepts a JSON object today. `--param k=v` is easy; `--json-patch` would
   be for scripts that need more than scalar overrides. The spec defers
   this until a real use case appears.
5. **Credential vault.** API keys live in `cli.toml` in plain text with
   mode `0600` for v1. A follow-up spec can integrate the system keyring
   (macOS Keychain, Secret Service on Linux, Credential Manager on
   Windows). This spec explicitly does not attempt it.

---

## 15. Recommendation

Build this in the order laid out in Section 12, with Phase 1 as the
anchor. The painter migration is the phase with the broadest surface area
and the strongest payoff: it touches every existing command handler,
establishes the token vocabulary new commands will inherit, and replaces
the latent divergence problem between CLI and TUI with a shared theme
source of truth. Everything after Phase 1 is additive.

The clear choice is to complete the CLI as a full control surface rather
than keep it at the happy-path fraction it occupies today. The daemon's
API investment is almost entirely unreached by scripts, the visual
inconsistency between CLI and TUI is a latent bug waiting to ship, and
the remote story punishes the exact use cases hypercolor is best at —
walking into a room, reaching for a terminal, and changing the scene.

Once this lands, the project has a single scriptable, themable, remote-
capable entry point that covers every daemon capability a user can
reasonably script. That unlocks three things downstream: shell-level
automation ("before sunset, `hypercolor scenes activate evening`"), CI
integration testing of real daemons from remote runners, and a clean
base for any future scripting surface (python bindings, a TypeScript
SDK, etc.) that would otherwise have to re-derive the same REST calls
from scratch.

If we do not make this investment now, every new daemon feature will
continue to ship without a CLI touchpoint, the CLI/TUI visual drift will
accumulate theme by theme, and remote use will remain a technique rather
than a first-class mode.
