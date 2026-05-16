# hypercolor-app

*Production unified desktop front door — starts and supervises the daemon, owns the tray.*

hypercolor-app is the one-click packaged experience for Hypercolor. Unlike hypercolor-desktop,
which is a thin shell that assumes a daemon is already running, hypercolor-app owns the full
application lifecycle: it embeds a Tauri 2 webview and system tray, supervises a bundled
`hypercolor-daemon` child process (or defers to a running systemd user service on Linux, or
an already-running daemon), handles platform concerns like autostart registration, a
single-instance guard, and minimized-start support. The window hides instead of closing when
the user presses the close button, so the daemon keeps running in the tray. Installation of
bundled runtime assets (effects, UI) from the Tauri resource directory is handled on first
launch.

## Role in the Workspace

Leaf binary. Depends on hypercolor-core and hypercolor-types for config path resolution, plus
Tauri 2 and several Tauri plugins. Has no direct dependency on hypercolor-daemon. Included in
the Cargo workspace; CI builds and tests this crate separately from the main workspace check
(it is excluded from the default `cargo check --workspace` pass but included in its own Tauri
build job).

## Binary

| Binary | Description |
|--------|-------------|
| `hypercolor-app` | Unified desktop app — starts daemon, owns tray, manages window |

## Key Modules

| Module | Responsibility |
|--------|---------------|
| `supervisor` | Resolves daemon path candidates, probes health, defers to systemd, spawns child daemon |
| `tray` | Registers tray icon and context menu |
| `window` | Window show/hide helpers; hide-on-close behavior |
| `resources` | Installs bundled runtime assets to the data directory |
| `cli` | `--quit` / `--minimized` / `--show` argument parsing |
| `daemon_client` | Lightweight async daemon health client |

## CLI Arguments

| Flag | Effect |
|------|--------|
| `--minimized` / `--hidden` | Start with main window hidden |
| `--show` | Bring an already-running instance to the foreground |
| `--quit` | Ask an already-running instance to quit |

## Platform Notes

- **Linux** — defers to `hypercolor.service` systemd user service if active; uses WebKitGTK;
  re-execs with WebKit environment variables if needed
- **Windows** — uses a Win32 Job Object to tie daemon lifetime to the app process
- **macOS** — registers a Launch Agent for autostart via tauri-plugin-autostart

## Cargo Features

None defined in `[features]`. Platform-conditional behavior is wired via `[target]` deps.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB lighting
orchestration for Linux. Apache-2.0 licensed.
