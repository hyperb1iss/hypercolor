# Hypercolor — Developer Commands
# Usage: just <recipe>    List: just --list

set dotenv-load := false
set positional-arguments := true

# Show available recipes (default when running `just` with no arguments)
[private]
default:
    @just --list

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
    ./scripts/cargo-cache-build.sh cargo build --workspace {{ args }}

# Build with the runtime-tuned preview profile
build-preview *args='':
    ./scripts/cargo-cache-build.sh cargo build --workspace --profile preview {{ args }}

# Build in release mode
release *args='':
    ./scripts/cargo-cache-build.sh cargo build --workspace --release {{ args }}

# Type-check without building
check *args='':
    ./scripts/cargo-cache-build.sh cargo check --workspace {{ args }}

# ─── Testing ──────────────────────────────────────────────

# Run all tests
test *args='':
    ./scripts/cargo-cache-build.sh cargo test --workspace {{ args }}

# Run tests for a specific crate
test-crate crate *args='':
    ./scripts/cargo-cache-build.sh cargo test -p {{ crate }} {{ args }}

# Run a specific test by name
test-one name *args='':
    ./scripts/cargo-cache-build.sh cargo test --workspace {{ name }} {{ args }}

# ─── Linting & Formatting ────────────────────────────────

# Run clippy with deny warnings
lint *args='':
    ./scripts/cargo-cache-build.sh cargo clippy --workspace --all-targets -- -D warnings {{ args }}

# Fix clippy suggestions automatically
lint-fix *args='':
    ./scripts/cargo-cache-build.sh cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged {{ args }}

# Format all code
fmt:
    cargo fmt --all

# Check formatting without modifying
fmt-check:
    cargo fmt --all -- --check

# ─── Supply Chain ─────────────────────────────────────────

# Audit dependencies (licenses, advisories, bans)
deny *args='':
    ./scripts/cargo-cache-build.sh cargo deny check {{ args }}

# ─── Documentation ────────────────────────────────────────

# Build docs for all crates
doc *args='':
    ./scripts/cargo-cache-build.sh cargo doc --workspace --no-deps {{ args }}

# Build and open docs in browser
doc-open: (doc "--open")

# ─── Running ──────────────────────────────────────────────

# Run the daemon
daemon *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor --profile preview -- --log-level debug {{ args }}

# Run the CLI
cli *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-cli -- {{ args }}

# Run the system tray applet
tray *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-tray -- {{ args }}

# Run the daemon in release mode
daemon-release *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor --release -- {{ args }}

# Run Servo daemon (dev profile) with cache wrapper
daemon-servo *args='':
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor --profile preview --features servo -- --log-level debug --bind 127.0.0.1:9420 {{ args }}

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

# Run daemon + UI dev server together (daemon on :9420, UI on :9430 proxying API)
dev *args='':
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor --profile preview --features servo -- --log-level debug --bind 127.0.0.1:9420 {{ args }} &
    sleep 2
    cd crates/hypercolor-ui && trunk serve --dist .dist-dev &
    wait

# Start the UI dev server (Trunk + hot reload on :9430)
ui-dev:
    cd crates/hypercolor-ui && trunk serve --dist .dist-dev

# Build the UI for production
ui-build:
    cd crates/hypercolor-ui && trunk build --release

# Build UI and copy dist for daemon embedding
ui-dist: ui-build
    @echo '✅ UI built at crates/hypercolor-ui/dist/'

# ─── SDK ─────────────────────────────────────────────────

# Install SDK dependencies
sdk-install:
    cd sdk && bun install

# Build SDK packages
sdk-build:
    cd sdk && bun run build

# Start SDK dev server with HMR
sdk-dev:
    cd sdk && bun run dev

# Typecheck SDK
sdk-check:
    cd sdk && bun run typecheck

# Lint & format SDK
sdk-lint:
    cd sdk && bun run check

# Build all SDK effects → effects/hypercolor/*.html
effects-build:
    cd sdk && bun run build:effects

# Build a single SDK effect (e.g., just effect-build borealis)
effect-build name:
    cd sdk && bun scripts/build-effect.ts src/effects/{{ name }}/main.ts

# ─── Site ─────────────────────────────────────────────────

# Start marketing site dev server (:9440)
site-dev:
    cd site && pnpm dev

# Build marketing site for production
site-build:
    cd site && pnpm build

# Typecheck + lint marketing site
site-check:
    cd site && pnpm check

# ─── Setup ───────────────────────────────────────────────

# Install all project dependencies (Rust targets, UI deps, SDK deps)
setup:
    #!/usr/bin/env bash
    set -euo pipefail
    echo '→ Checking rustup targets...'
    rustup target add wasm32-unknown-unknown
    if ! command -v bun &>/dev/null; then
        echo '→ Installing bun...'
        if [[ "$(uname -s)" == "Darwin" ]] && command -v brew &>/dev/null; then
            brew install oven-sh/bun/bun
        else
            curl -fsSL https://bun.sh/install | bash
        fi
    fi
    echo '→ Installing UI dependencies...'
    cd "{{justfile_directory()}}/crates/hypercolor-ui" && npm install
    echo '→ Installing SDK dependencies...'
    cd "{{justfile_directory()}}/sdk" && bun install
    echo '✅ All dependencies installed'

# Install Hypercolor locally under ~/.local and set up host integration
install *args='':
    ./scripts/install.sh {{ args }}

# Install udev rules for USB device access (requires sudo)
udev-install:
    sudo cp udev/99-hypercolor.rules /etc/udev/rules.d/
    sudo udevadm control --reload-rules
    sudo udevadm trigger --action=add --subsystem-match=hidraw
    sudo udevadm trigger --action=add --subsystem-match=usb
    sudo udevadm trigger --action=add --subsystem-match=tty
    sudo udevadm trigger --action=add --subsystem-match=i2c-dev
    @echo '✅ udev rules installed and applied'

# ─── Housekeeping ─────────────────────────────────────────

# Clean build artifacts
clean:
    ./scripts/cargo-cache-build.sh cargo clean

# Show workspace dependency tree
deps:
    cargo tree --workspace

# Show outdated dependencies
outdated:
    cargo outdated -wR

# Count lines of code (requires tokei)
loc:
    @tokei crates/ --sort code 2>/dev/null || echo 'Install tokei: cargo install tokei'
