# Hypercolor — Developer Commands
# Usage: just <recipe>    List: just --list

set dotenv-load := false
set positional-arguments := true

workspace_args := "--workspace --exclude hypercolor-desktop"

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
    ./scripts/cargo-cache-build.sh cargo build {{ workspace_args }} {{ args }}

# Build with the runtime-tuned preview profile
build-preview *args='':
    ./scripts/cargo-cache-build.sh cargo build {{ workspace_args }} --profile preview {{ args }}

# Build in release mode
release *args='':
    ./scripts/cargo-cache-build.sh cargo build {{ workspace_args }} --release {{ args }}

# Type-check without building
check *args='':
    ./scripts/cargo-cache-build.sh cargo check {{ workspace_args }} {{ args }}

# ─── Testing ──────────────────────────────────────────────

# Run all tests
test *args='':
    ./scripts/cargo-cache-build.sh cargo test {{ workspace_args }} {{ args }}

# Run tests for a specific crate
test-crate crate *args='':
    ./scripts/cargo-cache-build.sh cargo test -p {{ crate }} {{ args }}

# Run a specific test by name
test-one name *args='':
    ./scripts/cargo-cache-build.sh cargo test {{ workspace_args }} {{ name }} {{ args }}

# Compile and smoke-run benchmark targets without full measurement
bench-smoke:
    ./scripts/cargo-cache-build.sh cargo test -p hypercolor-core --bench core_pipeline
    ./scripts/cargo-cache-build.sh cargo test -p hypercolor-hal --bench protocol_encoding

# Run the core benchmark suite (Criterion HTML reports land in target/criterion/)
bench-core *args='':
    ./scripts/cargo-cache-build.sh cargo bench -p hypercolor-core --bench core_pipeline -- {{ args }}

# Run the HAL protocol benchmark suite
bench-hal *args='':
    ./scripts/cargo-cache-build.sh cargo bench -p hypercolor-hal --bench protocol_encoding -- {{ args }}

# Run all benchmark suites
bench:
    just bench-core
    just bench-hal

# Save a named Criterion baseline for all benchmark suites
bench-baseline name:
    just bench-core -- --save-baseline {{ name }}
    just bench-hal -- --save-baseline {{ name }}

# Compare all benchmark suites against a named Criterion baseline
bench-compare name:
    just bench-core -- --baseline {{ name }}
    just bench-hal -- --baseline {{ name }}

# ─── Linting & Formatting ────────────────────────────────

# Run clippy with deny warnings
lint *args='':
    ./scripts/cargo-cache-build.sh cargo clippy {{ workspace_args }} --all-targets -- -D warnings {{ args }}

# Fix clippy suggestions automatically
lint-fix *args='':
    ./scripts/cargo-cache-build.sh cargo clippy {{ workspace_args }} --all-targets --fix --allow-dirty --allow-staged {{ args }}

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
    ./scripts/cargo-cache-build.sh cargo doc {{ workspace_args }} --no-deps {{ args }}

# Build and open docs in browser
doc-open: (doc "--open")

# Serve the Zola documentation site (hot reload on :9440)
docs-dev:
    cd docs && zola serve --port 9440

# Build the Zola documentation site
docs-build:
    cd docs && zola build

# ─── Running ──────────────────────────────────────────────

# Run the daemon
daemon *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview -- --log-level debug {{ args }}

# Run the CLI
cli *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-cli --bin hypercolor -- {{ args }}

# Run the system tray applet
tray *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-tray -- {{ args }}

# Run the daemon in release mode
daemon-release *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --release -- {{ args }}

# Run Servo daemon (dev profile) with cache wrapper
daemon-servo *args='':
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features servo -- --log-level debug --bind 127.0.0.1:9420 {{ args }}

# Run Servo daemon in release mode with cache wrapper
daemon-servo-release *args='':
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --release --features servo -- --bind 127.0.0.1:9420 {{ args }}

# Build Servo daemon release artifacts once (faster repeat launches)
build-servo-release:
    ./scripts/servo-cache-build.sh cargo build -p hypercolor-daemon --release --features servo

# Run prebuilt Servo daemon release binary from cache target dir
run-servo-release-bin *args='':
    ~/.cache/hypercolor/target/release/hypercolor-daemon --bind 127.0.0.1:9420 {{ args }}

# ─── TUI ─────────────────────────────────────────────────

# Run the TUI. Attaches to an existing daemon, or starts a local one if needed.
tui *args='':
    #!/usr/bin/env bash
    set -euo pipefail
    host="${HYPERCOLOR_HOST:-127.0.0.1}"
    port="${HYPERCOLOR_PORT:-9420}"
    daemon_pid=""
    started_daemon=0

    cleanup() {
        if [[ "$started_daemon" -eq 1 && -n "$daemon_pid" ]]; then
            kill "$daemon_pid" 2>/dev/null || true
            wait "$daemon_pid" 2>/dev/null || true
        fi
    }

    trap cleanup EXIT

    health_url="http://${host}:${port}/health"
    can_autostart=0
    bind_host="$host"
    if [[ "$host" == "127.0.0.1" || "$host" == "localhost" ]]; then
        can_autostart=1
        bind_host="127.0.0.1"
    fi

    if ! curl --silent --fail --max-time 1 "$health_url" >/dev/null; then
        if [[ "$can_autostart" -ne 1 ]]; then
            echo "No daemon reachable at ${host}:${port}; start it first or point HYPERCOLOR_HOST at a live daemon." >&2
            exit 1
        fi

        echo "→ starting local daemon on ${bind_host}:${port}"
        ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features servo -- --log-level debug --bind "${bind_host}:${port}" &
        daemon_pid=$!
        started_daemon=1

        for _ in {1..40}; do
            if curl --silent --fail --max-time 1 "$health_url" >/dev/null; then
                break
            fi
            sleep 0.5
        done

        if ! curl --silent --fail --max-time 1 "$health_url" >/dev/null; then
            echo "Daemon failed to become ready at ${bind_host}:${port}" >&2
            exit 1
        fi
    fi

    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-cli --bin hypercolor -- tui {{ args }}

# Run daemon + TUI together
tui-dev *args='':
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features servo -- --log-level debug --bind 127.0.0.1:9420 &
    sleep 2
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-cli --bin hypercolor -- tui {{ args }} &
    wait

# ─── UI ──────────────────────────────────────────────────

# Run daemon + UI dev server together (daemon on :9420, UI on :9430 proxying API)
dev *args='':
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features servo -- --log-level debug --bind 127.0.0.1:9420 {{ args }} &
    sleep 2
    cd crates/hypercolor-ui && env -u NO_COLOR trunk serve --dist .dist-dev &
    wait

# Start the UI dev server (Trunk + hot reload on :9430)
ui-dev:
    cd crates/hypercolor-ui && env -u NO_COLOR trunk serve --dist .dist-dev

# Build the UI for production
ui-build:
    cd crates/hypercolor-ui && env -u NO_COLOR trunk build --release

# Build UI and copy dist for daemon embedding
ui-dist: ui-build
    @echo '✅ UI built at crates/hypercolor-ui/dist/'

# Run the standalone UI crate tests
ui-test:
    cd crates/hypercolor-ui && cargo test

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

# Run SDK lint/format checks without modifying files
sdk-lint:
    cd sdk && bun run check

# Apply SDK lint fixes
sdk-fix:
    cd sdk && bun run check:fix

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

# Build everything end-to-end and create a distribution tarball
dist *args='':
    ./scripts/dist.sh {{ args }}

# Install Hypercolor locally under ~/.local and set up host integration
install *args='':
    ./scripts/install.sh {{ args }}

# Uninstall Hypercolor from ~/.local
uninstall *args='':
    ./scripts/uninstall.sh {{ args }}

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
