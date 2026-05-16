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
| `library` | Manage favorite effects |
| `profiles` | Save and load profiles |
| `cloud` | Cloud login and account |
| `server` | Daemon connection settings |
| `servers` | Multi-server management |
| `service` | Daemon lifecycle (start/stop/status) |
| `status` | Quick daemon status |
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
hypercolor tui                     # Launch the full-screen terminal UI
hypercolor completions zsh         # Generate zsh completions
```

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB lighting
orchestration for Linux. Apache-2.0 licensed.
