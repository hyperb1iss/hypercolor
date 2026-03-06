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
just ui-dev              # Dev server on :3000, proxies API to :9420
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

## Specs & Design Docs

Implementation specs live in `docs/specs/`. Design docs in `docs/design/`.
Always check the relevant spec before implementing a module.
