# Hypercolor

Open-source RGB lighting orchestration engine for Linux, written in Rust.

## Quick Start

```bash
# Primary interface — use justfile recipes
just verify          # fmt + lint + test (run after every change)
just check           # Type-check without building
just build           # Debug build
just daemon          # Run daemon (preview profile, debug logging)
just ui-dev          # Leptos UI dev server (Trunk + hot reload)
just sdk-dev         # SDK dev server with HMR

# Direct cargo commands
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

## Project Structure

```
crates/
  hypercolor-types/   # Pure data types — zero deps, no logic, no I/O
  hypercolor-core/    # Engine: traits, bus, sampler, config, render loop, HAL
  hypercolor-hal/     # Hardware abstraction — USB/HID drivers (Razer, etc.)
  hypercolor-daemon/  # Binary: daemon + REST API + WebSocket + embedded UI
  hypercolor-cli/     # Binary: `hyper` CLI tool
  hypercolor-ui/      # Leptos 0.8 CSR web UI (WASM, built with Trunk)
  hypercolor-sdk/     # TypeScript SDK for HTML effects (Vite + Bun)
sdk/                  # SDK workspace (Bun monorepo)
docs/specs/           # Implementation specs
docs/design/          # Design documents
```

## Crate Ownership

Each crate has clear boundaries. Do NOT create cross-crate circular dependencies.

| Crate | Depends On | Responsibility |
|-------|-----------|----------------|
| `hypercolor-types` | (none) | Shared vocabulary — import from here, never sibling internals |
| `hypercolor-core` | `types` | Traits, engine logic, effect registry, audio pipeline |
| `hypercolor-hal` | `types`, `core` | USB/HID device drivers, protocol implementations |
| `hypercolor-daemon` | `core`, `hal` | HTTP/WS server, REST API, daemon lifecycle |
| `hypercolor-cli` | `core` | CLI parsing, output formatting, IPC client |
| `hypercolor-ui` | (standalone) | Leptos WASM app, excluded from workspace — see UI section |

## UI Crate

`hypercolor-ui` is a **Leptos 0.8 CSR** app compiled to WASM via **Trunk**. It is **excluded from
the Cargo workspace** — `cargo check --workspace` does NOT cover it.

```bash
# Build/check UI separately
just ui-dev              # Dev server on :9430, proxies API to :9420
cd crates/hypercolor-ui && trunk build   # One-shot build

# UI tech stack
# - Leptos 0.8 with fine-grained signals (signal(), Memo, Signal::derive)
# - Tailwind CSS v4 with custom @theme tokens (SilkCircuit palette)
# - leptos_icons (Icon component, style prop is MaybeProp<String> — no closures)
# - wasm-bindgen + web-sys for browser APIs
```

**Key pattern:** `leptos_icons::Icon`'s `style` prop accepts `MaybeProp<String>` — it takes
`&str` or `String`, NOT closures. Use conditional rendering to vary icon styles reactively.

## SDK

TypeScript SDK for building HTML effects, managed with **Bun**:

```bash
just sdk-install         # bun install
just sdk-dev             # Dev server with HMR
just sdk-build           # Build packages
just effects-build       # Build all effects -> effects/hypercolor/*.html
just effect-build NAME   # Build single effect
```

**Generated effects rule:** `effects/hypercolor/` is generated build output and is gitignored on
purpose. Never hand-edit files under `effects/hypercolor/`, never treat them as source of truth, and
never re-add them to version control. Make effect changes in `sdk/src/effects/` (and related SDK
sources) only, then regenerate locally as needed for verification.

## Conventions

- **Edition 2024**, Rust 1.85+
- **Tests in `tests/` directory** — NOT inline `#[cfg(test)]` blocks
- **When adding coverage, update or create files under each crate's `tests/` directory** — do not grow inline test modules in source files
- **`unsafe_code` is forbidden** across the entire workspace
- **Clippy pedantic** is enforced — `deny` level, see `Cargo.toml` for allowed exceptions
- **`unwrap()` is forbidden** — use `?`, `.ok()`, `expect("reason")`, or handle errors properly
- **`thiserror`** for library error types, **`anyhow`** for application error handling
- **`tracing`** for all logging (never `println!` in library code)
- **Serde** with `#[serde(rename_all = "snake_case")]` on enums, `#[serde(default)]` for backwards compat
- **Conventional commits**: `feat(scope):`, `fix(scope):`, `refactor(scope):`, etc.
- **Apache-2.0** license

## API Surface

The daemon exposes a REST + WebSocket API on `:9420`:

- `GET /api/v1/effects` — List all effects (returns `EffectSummary[]`)
- `GET /api/v1/effects/:id` — Effect detail with controls
- `POST /api/v1/effects/:id/apply` — Apply effect to devices
- `GET/POST/DELETE /api/v1/library/favorites` — Favorites CRUD
- `GET /api/v1/devices` — Connected devices
- `WebSocket /api/v1/ws` — Real-time state updates

## Agent Coordination

Multiple agents may work simultaneously. Follow these rules:

1. **Own your files** — only modify files in your assigned module
2. **Never touch `lib.rs`** of another crate without coordination
3. **`cargo check --workspace`** must pass after your changes (does NOT cover `hypercolor-ui`)
4. **No placeholder implementations** — implement the real logic or don't create the file
5. **Tests are mandatory** — every public type/function needs test coverage in `tests/`
6. **Never edit generated code** — especially anything under `effects/hypercolor/`; generated files are
   build artifacts, not source files

## HAL Driver Best Practices

When writing or modifying device drivers in `hypercolor-hal`, follow these patterns:

### Wire-Format Structs with `zerocopy`

Use `zerocopy` typed structs for all fixed-size protocol packets. Never use manual byte-offset
indexing (`buffer[4] = value`) when a struct can describe the layout instead.

```rust
use zerocopy::{FromZeros, IntoBytes, KnownLayout, Immutable};

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct MyDevicePacket {
    padding: u8,
    command: u8,
    channel: u8,
    data: [u8; 61],
}
```

Key rules:
- **`#[repr(C)]`** is required for deterministic field layout
- **Compile-time size assertions** — every packet struct must have one:
  ```rust
  const _: () = assert!(
      std::mem::size_of::<MyDevicePacket>() == EXPECTED_SIZE,
      "MyDevicePacket must match wire size"
  );
  ```
- **`FromZeros` + `IntoBytes`** for write-only packets (frame encoding)
- **`FromBytes` + `IntoBytes`** for packets that are also parsed from device responses
- **`FromBytes` implies `FromZeros`** — never derive both, it causes `E0119`
- Use `read_from_prefix()` (not `read_from_bytes()`) when parsing responses — HID transports
  may return buffers larger than the struct (extra report ID bytes, etc.)
- Use `zerocopy::byteorder::{LittleEndian, U16}` for multi-byte wire fields

### Zero-Copy Data Flow

Frame encoding runs at 30-60 FPS per device. Minimize allocations in the hot path:

- **`CommandBuffer::push_struct`** writes zerocopy structs directly into reusable command
  buffers — no intermediate `Vec<u8>` allocation per packet
- **`encode_frame_into`** over `encode_frame` — the `_into` variant reuses the command vector
  across frames instead of allocating a new one every tick
- **`Cow<'a, [[u8; 3]]>`** for frame normalization — borrow the input slice when the LED count
  already matches; only allocate when truncation/padding is needed
- **Avoid `.to_vec()` in loops** — if you're calling `.as_bytes().to_vec()` inside a per-chunk
  loop, use `push_struct` instead to write directly into the reusable buffer

### Protocol Implementation Checklist

Every new `impl Protocol` should:

1. Define typed packet structs with compile-time size assertions
2. Implement `encode_frame_into` (not just `encode_frame`) for buffer reuse
3. Use `CommandBuffer` with `push_struct` or `push_fill` — never build `Vec<ProtocolCommand>`
   with fresh allocations per frame
4. Use `Cow` normalization for the color input slice
5. Keep the hot path (frame encoding) allocation-free after warmup

## Specs & Design Docs

Implementation specs live in `docs/specs/`. Design docs in `docs/design/`.
Always check the relevant spec before implementing a module.
