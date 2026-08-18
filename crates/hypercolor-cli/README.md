# hypercolor-cli

*The primary user-facing command line for Hypercolor.*

This crate builds the `hypercolor` binary — the main interface for controlling Hypercolor from
a terminal. It communicates with a running daemon over HTTP REST (port 9420 by default) and
renders output as styled tables, plain text, or JSON via the `opaline` theming layer. When built
with the `tui` feature (on by default), `hypercolor tui` hands off to hypercolor-tui for the
full-screen terminal UI rather than routing through the REST client.

## Role in the Workspace

Leaf binary. Depends on hypercolor-core for shared types and config helpers, and optionally
on hypercolor-tui (feature-gated). Nothing in the workspace depends on this crate.

## Binary

| Binary | Command |
|--------|---------|
| `hypercolor` | `just cli` |

## Subcommands

| Subcommand | Description |
|------------|-------------|
| `effects` | List, activate, and patch effects |
| `brightness` | Set device brightness |
| `scenes` | List and activate scenes |
| `devices` | Show connected devices |
| `layouts` | Manage spatial layouts |
| `audio` | Audio input configuration |
| `access` | Explicit protected input and screen-capture actions |
| `library` | Manage favorite effects |
| `profiles` | Save and load profiles |
| `server` | Daemon connection settings |
| `servers` | Multi-server management |
| `service` | Daemon lifecycle and macOS owner selection |
| `status` | Quick daemon status or event-driven watch |
| `controls` | Adjust live effect controls |
| `config` | CLI configuration |
| `drivers` | Driver diagnostics |
| `completions` | Generate shell completions |
| `diagnose` | Run diagnostics |
| `tui` | Launch the terminal UI (requires `tui` feature) |

## Cargo Features

| Feature | Default | Description |
|---------|---------|-------------|
| `tui` | yes | Embeds hypercolor-tui and wires the `tui` subcommand |

## Usage

```bash
hypercolor effects list            # List available effects
hypercolor effects activate <id>   # Activate an effect by name
hypercolor scenes activate <id>    # Activate a scene
hypercolor brightness set 80       # Set global brightness to 80%
hypercolor status --watch          # Refresh status from ownership/input events
hypercolor access authorize-input-monitoring
hypercolor access authorize-screen-recording
hypercolor access choose-screen-source
hypercolor service choose-owner app-sidecar
hypercolor service choose-owner direct-launchd
hypercolor service choose-owner homebrew
hypercolor tui                     # Launch the full-screen terminal UI
hypercolor completions zsh         # Generate zsh completions
```

Protected access commands never prompt during daemon startup. On macOS they
ask the active protected-capability owner to perform one explicit action. A
headless owner that cannot present the system picker returns a typed app-UI
remedy.

The macOS owner command coordinates the desktop app sidecar, direct launchd
service, and Homebrew service through one durable local handoff. A standalone
daemon is reported with a stop remedy rather than terminated remotely. Only one
topology can hold the per-user daemon guard.

Apple Silicon supports the native HDR capture path. Intel Macs use SDR and
report HDR as unsupported. On macOS 26 Tahoe, compatible selections can expose
paired SDR and HDR reference diagnostics; SDR-only selections remain explicitly
single-range.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB lighting
orchestration for Linux. Apache-2.0 licensed.
