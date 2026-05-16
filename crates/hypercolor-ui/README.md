# hypercolor-ui

*Leptos 0.8 CSR web frontend for the Hypercolor lighting engine.*

This crate is the browser-facing half of Hypercolor: a client-side rendered WASM
application built with Leptos 0.8 and compiled by Trunk. It talks to the running
daemon over HTTP and WebSocket and provides a live UI for controlling every
aspect of the lighting engine.

## Workspace Status

**This crate is excluded from the Cargo workspace.** `cargo check --workspace`
and `just verify` do not cover it. Build and test separately:

```bash
just ui-dev          # Trunk dev server on :9430, proxies API to :9420
just ui-build        # Production WASM build
just ui-test         # Run UI crate tests
```

Trunk orchestrates Tailwind v4 compilation before the WASM build. See
`crates/hypercolor-ui/Trunk.toml` for the full build configuration.

## Role and Position

Leaf WASM application. Depends on `hypercolor-types` and `hypercolor-leptos-ext`
(via relative paths in `Cargo.toml`) for shared engine types and the WebSocket
client. All other dependencies are third-party WASM/browser crates: `leptos`,
`leptos_router`, `leptos_meta`, `gloo-net`, `wasm-bindgen`, `web-sys`,
`leptos_icons`, `leptoaster`. Version pinning is manual (no workspace
inheritance).

## Key Entry Points

- `main()` — mounts the Leptos `App` component to the document body.
- `app::App` — root component with router and layout shell.
- `pages/` — top-level page components: dashboard, effects browser, devices,
  displays, spatial layout designer, settings.
- `components/` — reusable Leptos components: device cards, canvas preview,
  layout builder, color wheel, and more.
- `ws/` — WebSocket integration using `hypercolor-leptos-ext`.
- `tauri_bridge` — optional native hooks when running inside `hypercolor-app`.

## Cargo Features

None. Build variants are Trunk-managed.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Apache-2.0.
