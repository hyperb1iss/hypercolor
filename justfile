# Hypercolor — Developer Commands
# Usage: just <recipe>    List: just --list

set dotenv-load := false
set positional-arguments := true

# ─── Aliases ──────────────────────────────────────────────

alias b := build
alias c := check
alias t := test
alias l := lint
alias f := fmt

# ─── Core ─────────────────────────────────────────────────

# Run all checks (format, lint, test)
verify: fmt-check lint test
    @echo '✅ All checks passed'

# Build the workspace
build *args='':
    cargo build --workspace {{ args }}

# Build in release mode
release *args='':
    cargo build --workspace --release {{ args }}

# Type-check without building
check *args='':
    cargo check --workspace {{ args }}

# ─── Testing ──────────────────────────────────────────────

# Run all tests
test *args='':
    cargo test --workspace {{ args }}

# Run tests for a specific crate
test-crate crate *args='':
    cargo test -p {{ crate }} {{ args }}

# Run a specific test by name
test-one name *args='':
    cargo test --workspace {{ name }} {{ args }}

# ─── Linting & Formatting ────────────────────────────────

# Run clippy with deny warnings
lint *args='':
    cargo clippy --workspace --all-targets -- -D warnings {{ args }}

# Fix clippy suggestions automatically
lint-fix *args='':
    cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged {{ args }}

# Format all code
fmt:
    cargo fmt --all

# Check formatting without modifying
fmt-check:
    cargo fmt --all -- --check

# ─── Supply Chain ─────────────────────────────────────────

# Audit dependencies (licenses, advisories, bans)
deny *args='':
    cargo deny check {{ args }}

# ─── Documentation ────────────────────────────────────────

# Build docs for all crates
doc *args='':
    cargo doc --workspace --no-deps {{ args }}

# Build and open docs in browser
doc-open: (doc "--open")

# ─── Running ──────────────────────────────────────────────

# Run the daemon
daemon *args='':
    cargo run -p hypercolor-daemon --bin hypercolor -- {{ args }}

# Run the CLI
cli *args='':
    cargo run -p hypercolor-cli -- {{ args }}

# Run the daemon in release mode
daemon-release *args='':
    cargo run -p hypercolor-daemon --bin hypercolor --release -- {{ args }}

# Run Servo daemon (dev profile) with cache wrapper
daemon-servo *args='':
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor --features servo -- --bind 127.0.0.1:9420 {{ args }}

# Run Servo daemon in release mode with cache wrapper
daemon-servo-release *args='':
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor --release --features servo -- --bind 127.0.0.1:9420 {{ args }}

# Build Servo daemon release artifacts once (faster repeat launches)
build-servo-release:
    ./scripts/servo-cache-build.sh cargo build -p hypercolor-daemon --release --features servo

# Run prebuilt Servo daemon release binary from cache target dir
run-servo-release-bin *args='':
    ~/.cache/hypercolor/target/release/hypercolor --bind 127.0.0.1:9420 {{ args }}

# ─── UI ──────────────────────────────────────────────────

# Start the UI dev server (Trunk + hot reload)
ui-dev:
    cd crates/hypercolor-ui && trunk serve

# Build the UI for production
ui-build:
    cd crates/hypercolor-ui && trunk build --release

# Build UI and copy dist for daemon embedding
ui-dist: ui-build
    @echo '✅ UI built at crates/hypercolor-ui/dist/'

# ─── Housekeeping ─────────────────────────────────────────

# Clean build artifacts
clean:
    cargo clean

# Show workspace dependency tree
deps:
    cargo tree --workspace

# Show outdated dependencies
outdated:
    cargo outdated -wR

# Count lines of code (requires tokei)
loc:
    @tokei crates/ --sort code 2>/dev/null || echo 'Install tokei: cargo install tokei'
