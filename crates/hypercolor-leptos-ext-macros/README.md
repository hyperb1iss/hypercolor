# hypercolor-leptos-ext-macros

*Proc-macro companion to `hypercolor-leptos-ext` — provides `#[derive(BinaryFrame)]`.*

This is a `proc-macro = true` crate. It provides one derive macro used when
defining typed binary WebSocket frame structs in the Hypercolor WebSocket
protocol stack. Users of `hypercolor-leptos-ext` get this macro automatically as
a transitive dependency; they rarely need to add this crate directly.

## Role and Position

```
hypercolor-leptos-ext-macros ← you are here (proc macros)
           ↓
hypercolor-leptos-ext        (WebSocket traits, browser helpers, Leptos adapters)
           ↓
hypercolor-ui                (Leptos CSR WASM frontend)
```

Depends only on `proc-macro2`, `quote`, and `syn`. No workspace or runtime
crates.

## Public Surface

### `#[derive(BinaryFrame)]`

Implements `hypercolor_leptos_ext::ws::BinaryFrameSchema` for the annotated
struct, generating `TAG: u8`, `SCHEMA: u8`, and `NAME: &'static str` associated
constants.

```rust
use hypercolor_leptos_ext::BinaryFrame;

#[derive(BinaryFrame)]
#[frame(tag = 0x01, schema = 1)]
struct CanvasFrame {
    width: u16,
    height: u16,
    data: Vec<u8>,
}

// Generated:
// impl BinaryFrameSchema for CanvasFrame {
//     const TAG: u8 = 1;
//     const SCHEMA: u8 = 1;
//     const NAME: &'static str = "CanvasFrame";
// }
```

The `frame` attribute accepts `tag` and `schema` as integer literals in
decimal, hex (`0x01`), octal (`0o1`), or binary (`0b1`) form. Both are
required; omitting either is a compile error.

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Apache-2.0.
