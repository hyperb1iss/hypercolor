# Hypercolor

Open-source RGB lighting orchestration engine for Linux, written in Rust.

## Build & Test

**IMPORTANT:** Chocolatey installed an old `cargo` that shadows rustup's proxy.
Always prepend PATH or use the full path to ensure the correct toolchain:

```bash
# Fix PATH for this session (do this first!)
export PATH="/c/Users/Stefanie/.cargo/bin:$PATH"

# Check everything compiles
cargo check --workspace

# Run all tests
cargo test --workspace

# Lint (must pass with zero warnings)
cargo clippy --workspace --all-targets -- -D warnings

# Format
cargo fmt --all

# Full verification (run after every change)
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

## Project Structure

```
crates/
  hypercolor-types/   # Pure data types — zero deps, no logic, no I/O
  hypercolor-core/    # Engine: traits, bus, sampler, config, render loop
  hypercolor-daemon/  # Binary: daemon + REST API + WebSocket
  hypercolor-cli/     # Binary: `hyper` CLI tool
```

## Conventions

- **Edition 2024**, Rust 1.85+
- **Tests in `tests/` directory** — NOT inline `#[cfg(test)]` blocks
- **`unsafe_code` is forbidden** across the entire workspace
- **Clippy pedantic** is enforced — `deny` level, see `Cargo.toml` for allowed exceptions
- **`unwrap()` is forbidden** — use `?`, `.ok()`, `expect("reason")`, or handle errors properly
- **`thiserror`** for library error types, **`anyhow`** for application error handling
- **`tracing`** for all logging (never `println!` in library code)
- **Conventional commits**: `feat(scope):`, `fix(scope):`, `refactor(scope):`, etc.
- **Apache-2.0** license

## Crate Ownership

Each crate has clear boundaries. Do NOT create cross-crate circular dependencies.

- `hypercolor-types`: Shared vocabulary. Import from here, never from sibling crate internals.
- `hypercolor-core`: Depends on `types`. Contains traits, engine logic, backends.
- `hypercolor-daemon`: Depends on `core`. HTTP server, WebSocket, daemon lifecycle.
- `hypercolor-cli`: Depends on `core`. CLI parsing, output formatting, IPC client.

## Agent Coordination

Multiple agents may work simultaneously. Follow these rules:

1. **Own your files** — only modify files in your assigned module
2. **Never touch `lib.rs`** of another crate without coordination
3. **`cargo check --workspace`** must pass after your changes
4. **No placeholder implementations** — implement the real logic or don't create the file
5. **Tests are mandatory** — every public type/function needs test coverage in `tests/`

## Specs & Design Docs

Implementation specs live in `docs/specs/`. Design docs in `docs/design/`.
Always check the relevant spec before implementing a module.
