# hypercolor-tray

*Lightweight system tray / menu bar applet for Hypercolor.*

The `hypercolor-tray` binary provides platform-native tray presence for a running Hypercolor
daemon. It shows current connection status (connected / paused / disconnected) as a tray icon
and a context menu with direct effect and profile activation, brightness control, pause/resume,
multi-server switching, and an "Open Web UI" action that opens the browser to the daemon's UI.

Architecture: the main thread runs the platform event loop (required by tray-icon); a
background Tokio thread drives async daemon communication via REST and WebSocket;
`std::sync::mpsc` bridges daemon state updates to the UI thread.

## Role in the Workspace

Leaf binary. Depends on hypercolor-core and hypercolor-types. Has no dependency on
hypercolor-daemon or hypercolor-cli — it communicates with the daemon solely over the network
API, the same way any external client would.

## Binary

| Binary | Command |
|--------|---------|
| `hypercolor-tray` | `just tray` |

## Platform Support

- **Linux** — libappindicator via gtk
- **macOS** — AppKit via objc2
- **Windows** — Win32 event loop via tao

## Cargo Features

None defined.

## Usage

```bash
just tray           # Launch the tray applet
hypercolor-tray     # Run directly
```

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB lighting
orchestration for Linux. Apache-2.0 licensed.
