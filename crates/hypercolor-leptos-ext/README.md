# hypercolor-leptos-ext

In-tree canary for Hypercolor's future Leptos, browser, and stream extension
crate.

This crate exists to prove the extraction boundary before any public
`cinder-*` crate is published. It keeps the first production user inside
Hypercolor, where API mistakes are cheap to fix and the daemon/UI protocol is
available for real load testing.

## Feature Matrix

- `ws-core`: binary frame traits, schema negotiation, transports, reconnect,
  backpressure policies, and Hypercolor preview frame codecs.
- `ws-client-wasm`: browser `web_sys::WebSocket` transport for
  `wasm32-unknown-unknown`.
- `axum`: server-side Axum WebSocket transport.
- `events`, `canvas`, `raf`, `prelude`: browser helper modules, gated to
  `wasm32`.
- `leptos`: Leptos-specific adapters.
- `devtools`: reserved for diagnostics.

Default features are empty. Consumers opt into exactly the runtime surface they
need.

## Current Extraction Target

The stream core is the first viable public extraction candidate. The intended
public crate shape is:

- `cinder-stream`: `BinaryFrame`, schema negotiation, `CinderTransport`,
  `BinaryChannel`, reconnect policy primitives, and backpressure queues.
- `cinder-web`: browser-native event, animation frame, canvas, and WebSocket
  wrappers once the UI migration proves the ergonomics.
- `cinder-leptos`: Leptos adapters layered above `cinder-web` and
  `cinder-stream`.

The visual preview media path is intentionally not locked into a new protocol
yet. Hypercolor's current raw/JPEG WebSocket preview remains the Year 1 V1 codec
while WebCodecs/WebRTC are evaluated for the long-term preview plane.

## Verification

Run the extension and its known consumers separately:

```bash
cargo test -p hypercolor-leptos-ext --features ws-core,axum
cargo check --workspace
cargo check -p hypercolor-leptos-ext --target wasm32-unknown-unknown --features ws-client-wasm
cargo check --manifest-path crates/hypercolor-ui/Cargo.toml --target wasm32-unknown-unknown
```
