# hypercolor-tui

*Full-screen Ratatui terminal UI for Hypercolor — a live instrument for controlling light.*

hypercolor-tui is a library crate, not a standalone binary. It is launched exclusively via the
`hypercolor tui` subcommand in hypercolor-cli. The TUI connects to a running daemon's REST API
and WebSocket, renders a live dashboard of effects, devices, layouts, and performance metrics,
and supports mouse and keyboard interaction. Live LED canvas previews are displayed using
Kitty, Sixel, iTerm2, or halfblock protocols depending on what the terminal supports. Themes
are persisted in a user config file. Output is logged to a file rather than stderr to avoid
corrupting the alternate screen.

## Role in the Workspace

Library consumed by hypercolor-cli (behind the `tui` feature flag). Depends on
hypercolor-types, hypercolor-color, and hypercolor-core for shared vocabulary;
hypercolor-leptos-ext (feature `ws-core`) for the canonical binary WebSocket frame codecs,
so the TUI never hand-rolls a frame layout; and third-party UI crates: ratatui, crossterm,
tachyonfx, ratatui-image, tokio-tungstenite. No workspace crate depends on it other than
hypercolor-cli.

## Public Entry Point

`launch` is the intended entry point:

```rust
pub async fn launch(
    host: String,
    port: u16,
    api_key: Option<String>,
    theme: Option<String>,
    log_level: &str,
) -> anyhow::Result<()>
```

This is called by hypercolor-cli when the `tui` subcommand is dispatched. The function takes
over the terminal, runs the event loop, and returns when the user quits. The crate's other
modules are public for testing and internal composition and carry no stability guarantee.

## Cargo Features

None. hypercolor-tui has no `[features]` table; the full UI surface is always compiled in.

## Usage

The TUI is not invoked directly — run it through the CLI:

```bash
hypercolor tui
just tui          # Via the justfile (auto-starts daemon if needed)
```

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB lighting
orchestration for Linux, Windows, and macOS. Apache-2.0 licensed.
