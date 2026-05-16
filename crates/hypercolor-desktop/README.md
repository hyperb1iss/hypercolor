# hypercolor-desktop

*Thin Tauri 2 webview shell for the Hypercolor UI — assumes a daemon is already running.*

hypercolor-desktop is a minimal native window that loads the Hypercolor web UI from a running
daemon instance. It points a system webview at `http://127.0.0.1:9420` (overridable via the
`HYPERCOLOR_URL` environment variable) and does nothing else — no daemon management, no
supervision, no tray. In debug builds the Tauri devtools panel opens automatically.

This is an earlier, simpler shell than hypercolor-app. If you want the one-click packaged
experience that starts and supervises the daemon, see hypercolor-app instead.

## Role in the Workspace

Leaf binary. Depends only on Tauri 2, anyhow, serde, tracing, and url — no workspace crate
dependencies beyond those. Included in the Cargo workspace but **excluded from default CI**;
it is built and tested separately.

## Binary

| Binary | Description |
|--------|-------------|
| `hypercolor-desktop` | Opens a webview window pointed at the daemon URL |

## Cargo Features

None defined.

## Usage

Start the daemon first, then launch the shell:

```bash
just daemon &
HYPERCOLOR_URL=http://127.0.0.1:9420 hypercolor-desktop
```

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB lighting
orchestration for Linux. Apache-2.0 licensed.
