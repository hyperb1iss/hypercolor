+++
title = "Contributing"
description = "Development setup, code style, and contribution guidelines"
weight = 1
sort_by = "weight"
template = "section.html"
+++

Hypercolor is open source under the Apache-2.0 license. Contributions are welcome — bug fixes, new device drivers, effects, documentation improvements, and feature work.

## Development Setup

```bash
# Clone and enter the repository
git clone https://github.com/hyperb1iss/hypercolor.git
cd hypercolor

# Install all dependencies (Rust targets, Bun for SDK, UI deps)
just setup

# Run the full verification suite
just verify
```

`just verify` runs formatting checks, clippy lints, and the test suite. Run it after every change before committing.

## Build Commands

| Command | Description |
|---|---|
| `just build` | Debug build |
| `just release` | Release build |
| `just check` | Type-check without building |
| `just test` | Run all tests |
| `just lint` | Clippy with `-D warnings` |
| `just fmt` | Format all code |
| `just verify` | Format check + lint + test (run before committing) |
| `just daemon` | Run daemon in preview mode |
| `just ui-dev` | Leptos UI dev server |
| `just sdk-dev` | SDK dev server with HMR |

## Code Style

### Rust Conventions

- **Edition 2024**, Rust 1.85+
- **Clippy pedantic** enforced at `deny` level
- **`unsafe` code is forbidden** across the entire workspace
- **`unwrap()` is forbidden** — use `?`, `.ok()`, `expect("reason")`, or proper error handling
- **`thiserror`** for library error types, **`anyhow`** for application errors
- **`tracing`** for all logging — never `println!` in library code
- **Serde** with `#[serde(rename_all = "snake_case")]` on enums, `#[serde(default)]` for backwards compatibility

### Testing

Tests go in the `tests/` directory of each crate, not inline `#[cfg(test)]` blocks:

```
crates/hypercolor-core/
  src/
    engine.rs
  tests/
    engine_tests.rs    # Tests for engine.rs
    sampler_tests.rs   # Tests for spatial sampler
```

Every public type and function needs test coverage. When adding features, add tests. When fixing bugs, add a regression test.

### Commit Conventions

Use [conventional commits](https://www.conventionalcommits.org/):

```
feat(hal): add Corsair iCUE protocol driver
fix(core): prevent panic on empty color frame
refactor(daemon): extract scene manager from API layer
test(hal): add PrismRGB frame encoding roundtrip tests
docs(api): document WebSocket binary frame format
```

Scopes match crate names: `core`, `hal`, `daemon`, `cli`, `tui`, `ui`, `sdk`, `types`.

## Crate Boundaries

Each crate has clear ownership. Respect the dependency graph:

- **`hypercolor-types`** depends on nothing. All shared data types live here.
- **`hypercolor-core`** depends on `types`. Engine logic, traits, and abstractions.
- **`hypercolor-hal`** depends on `types` and `core`. Device drivers only.
- **`hypercolor-daemon`** depends on `core` and `hal`. Never imported by other crates.

Do NOT create circular dependencies between crates. If you need a type in multiple crates, it belongs in `hypercolor-types`.

## HAL Driver Checklist

When implementing a new device driver:

1. Define wire-format packet structs with `zerocopy` derives (`FromZeros`, `IntoBytes`, `KnownLayout`, `Immutable`)
2. Add `#[repr(C)]` and compile-time size assertions for every packet struct
3. Implement `encode_frame_into` (not just `encode_frame`) for buffer reuse
4. Use `CommandBuffer` with `push_struct` or `push_fill` — no `Vec<ProtocolCommand>` allocations per frame
5. Use `Cow` normalization for the input color slice
6. Keep the hot path allocation-free after warmup
7. Add tests in `crates/hypercolor-hal/tests/`

## Pull Request Process

1. Create a feature branch from `main`
2. Make your changes with tests
3. Run `just verify` — it must pass
4. Push and open a PR with a clear description of what and why
5. Respond to review feedback

{% callout(type="tip", title="Check the specs") %}
Implementation specs live in `docs/specs/` and design documents in `docs/design/`. Check for a relevant spec before implementing a module — it may contain important design decisions and API contracts.
{% end %}
