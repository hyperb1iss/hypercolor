# Hypercolor — Developer Commands
# Usage: just <recipe>    List: just --list

set dotenv-load := false
set positional-arguments := true

workspace_args := "--workspace"
test_features := "--features hypercolor-daemon/persistence-test-hooks"
daemon_bind := env_var_or_default("HYPERCOLOR_DAEMON_BIND", "127.0.0.1:9420")

# Bundled effects live where they are installed, which for a dev build would be
# next to the binary under target/. Point every recipe at the tree we build.
export HYPERCOLOR_EFFECTS_DIR := env_var_or_default("HYPERCOLOR_EFFECTS_DIR", justfile_directory() / "effects" / "hypercolor")

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
alias a := app
alias py := python-verify

# ─── Core ─────────────────────────────────────────────────

# Run all checks (boundary, format, lint, test)
verify: oss-boundary-check-strict api-doc-route-check macos-gpu-only-check build-wrapper-test cargo-gc-test fmt-check lint test alloc-contracts
    @echo '✅ All checks passed'

# Verify target isolation and Cargo argument normalization
# The fixture needs flock, /proc, and bash 4.2, so it runs on Linux hosts
# and in the Linux CI lane only.
[linux]
build-wrapper-test:
    ./scripts/tests/cargo-cache-build-tests.sh

[macos]
build-wrapper-test:
    @echo 'Cargo wrapper fixture tests run only on Linux hosts'

[windows]
build-wrapper-test:
    @echo 'Build wrapper contract is covered by the Rust packaging tests on Windows'

# Prove stale, recent, locked, and dirty target profiles are handled safely
[linux]
cargo-gc-test:
    ./scripts/tests/cargo-target-gc-tests.sh

[macos]
cargo-gc-test:
    @echo 'Cargo target GC is installed only on Linux hosts'

[windows]
cargo-gc-test:
    @echo 'Cargo target GC is installed only on Linux hosts'

# Check OSS/internal boundary guard scaffolding without strict enforcement
oss-boundary-check:
    ./scripts/check-oss-boundary.sh

# Strict boundary check for commercial cloud extraction
oss-boundary-check-strict:
    ./scripts/check-oss-boundary.sh --strict

# Keep current documentation free of retired public API routes
api-doc-route-check:
    ./scripts/check-retired-api-docs.sh

# Keep production macOS capture native-only while retaining fixture oracles
macos-gpu-only-check:
    ./scripts/check-macos-gpu-only.sh

# Build the workspace with the daemon's full feature set
[unix]
build *args='':
    ./scripts/cargo-cache-build.sh cargo build {{ workspace_args }} {{ args }}

# Build with full symbols for debugger sessions
[unix]
debug-build *args='':
    ./scripts/cargo-cache-build.sh cargo build {{ workspace_args }} --profile debugging {{ args }}

[windows]
debug-build *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo build {{ workspace_args }} --profile debugging {{ args }}

[windows]
build *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo build {{ workspace_args }} {{ args }}

# Build with the runtime-tuned preview profile and full daemon features
[unix]
build-preview *args='':
    ./scripts/cargo-cache-build.sh cargo build {{ workspace_args }} --profile preview {{ args }}

[windows]
build-preview *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo build {{ workspace_args }} --profile preview {{ args }}

# Build a full release bundle with binaries, assets, docs, and agent skills
release *args='':
    ./scripts/dist.sh {{ args }}

# Build release binaries only without assembling a distribution bundle
[unix]
release-bin *args='':
    ./scripts/cargo-cache-build.sh cargo build {{ workspace_args }} --release {{ args }}

[windows]
release-bin *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo build {{ workspace_args }} --release {{ args }}

# Type-check without building
[unix]
check *args='':
    ./scripts/cargo-cache-build.sh cargo check {{ workspace_args }} {{ args }}

[windows]
check *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo check {{ workspace_args }} {{ args }}

# ─── Python Client ────────────────────────────────────────

# Sync Python client dependencies
python-sync:
    cd python && uv sync

# Lint the Python client with Ruff
python-lint:
    cd python && uv run ruff check .

# Format-check the Python client with Ruff
python-fmt-check:
    cd python && uv run ruff format --check .

# Apply Python client Ruff fixes
python-fix:
    cd python && uv run ruff check --fix .
    cd python && uv run ruff format .

# Type-check the Python client with ty
python-typecheck:
    cd python && uv run ty check

# Generate the Python OpenAPI client
python-generate *args='':
    cd python && uv run python scripts/generate_openapi_client.py {{ args }}

# Verify the generated Python OpenAPI client is current
python-generate-check:
    cd python && uv run python scripts/generate_openapi_client.py --check

# Regenerate the WebSocket protocol manifest from the topic registry
ws-manifest:
    ./scripts/cargo-cache-build.sh cargo run -q -p hypercolor-daemon --bin hypercolor-ws-manifest

# Verify the WebSocket protocol manifest matches the topic registry
ws-manifest-check:
    ./scripts/cargo-cache-build.sh cargo run -q -p hypercolor-daemon --bin hypercolor-ws-manifest -- --check

# Generate Python WebSocket protocol constants
python-ws-protocol-generate:
    cd python && uv run python scripts/generate_ws_protocol.py

# Verify Python WebSocket protocol constants are current
python-ws-protocol-check:
    cd python && uv run python scripts/generate_ws_protocol.py --check

# Test the Python client
python-test:
    cd python && uv run pytest

# Build Python client sdist and wheel with uv-build
python-build:
    cd python && uv build

# Run the full Python client verification suite
python-verify: python-lint python-fmt-check python-typecheck python-ws-protocol-check python-test
    @echo '✅ Python checks passed'

# ─── Testing ──────────────────────────────────────────────

# Run all tests
[unix]
test *args='':
    ./scripts/cargo-cache-build.sh cargo test {{ workspace_args }} {{ test_features }} {{ args }}

[windows]
test *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo test {{ workspace_args }} {{ test_features }} {{ args }}

# Run process-global allocation counters without concurrent test threads
[unix]
alloc-contracts:
    ./scripts/cargo-cache-build.sh cargo test -p hypercolor-core --no-default-features --features allocation-contract-tests --test alloc_contract_tests --test media_input_allocation_tests --test screen_cpu_fanout_allocation_tests --test spatial_area_reuse_tests -- --test-threads=1
    ./scripts/cargo-cache-build.sh cargo test -p hypercolor-windows-input --test alloc_contract_tests -- --test-threads=1
    ./scripts/cargo-cache-build.sh cargo test -p hypercolor-daemon --no-default-features --features wgpu,allocation-contract-tests --test alloc_contract_tests -- --test-threads=1

[windows]
alloc-contracts:
    HYPERCOLOR_NO_FAST_LINK=1 powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo test -p hypercolor-core --no-default-features --features allocation-contract-tests --test alloc_contract_tests --test media_input_allocation_tests --test screen_cpu_fanout_allocation_tests --test spatial_area_reuse_tests -- --test-threads=1
    HYPERCOLOR_NO_FAST_LINK=1 powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo test -p hypercolor-windows-input --test alloc_contract_tests -- --test-threads=1
    HYPERCOLOR_NO_FAST_LINK=1 powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo test -p hypercolor-daemon --no-default-features --features wgpu,allocation-contract-tests --test alloc_contract_tests -- --test-threads=1

# Run tests for a specific crate (iteration-shaped: keeps incremental rebuilds)
[unix]
test-crate crate *args='':
    HYPERCOLOR_ITERATE=1 ./scripts/cargo-cache-build.sh cargo test -p {{ crate }} {{ if crate == "hypercolor-daemon" { "--features persistence-test-hooks" } else { "" } }} {{ args }}

[windows]
test-crate crate *args='':
    HYPERCOLOR_ITERATE=1 powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo test -p {{ crate }} {{ if crate == "hypercolor-daemon" { "--features persistence-test-hooks" } else { "" } }} {{ args }}

# Run a specific test by name (iteration-shaped: keeps incremental rebuilds)
[unix]
test-one name *args='':
    HYPERCOLOR_ITERATE=1 ./scripts/cargo-cache-build.sh cargo test {{ workspace_args }} {{ test_features }} {{ name }} {{ args }}

[windows]
test-one name *args='':
    HYPERCOLOR_ITERATE=1 powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo test {{ workspace_args }} {{ test_features }} {{ name }} {{ args }}

# Manually run the Cinder/Leptos extension design audit snapshot generator
cinder-audit:
    ./scripts/cinder-audit.sh >/dev/null

# Regenerate the compatibility matrix from data/drivers/vendors/*.toml
compat *args='':
    bun scripts/gen-compat.ts {{ args }}

# Verify the compatibility matrix is up to date (gated by the `compat` CI job)
compat-check:
    bun scripts/gen-compat.ts --check

# Stamp a release version across every version-bearing file (see RELEASING.md)
set-version version:
    bun scripts/set-version.ts {{ version }}

# Verify every version-bearing file carries the given version
set-version-check version:
    bun scripts/set-version.ts {{ version }} --verify

# Observe an already-running daemon for graphics pipeline soak regressions
graphics-soak *args='':
    bun scripts/graphics-pipeline-soak.ts {{ args }}

# Diagnose an already-running daemon through the cross-platform REST snapshot
diagnose *args='':
    bun scripts/diagnose-daemon.ts {{ args }}

# Observe an already-running daemon for the 30-minute graphics acceptance soak
graphics-soak-30 *args='':
    out_dir="${CARGO_TARGET_DIR:-target}/graphics-soak"; mkdir -p "$out_dir"; bun scripts/graphics-pipeline-soak.ts --duration 30m --out "$out_dir/latest.json" {{ args }}

# Observe Servo GPU import/readback performance on an already-running daemon
servo-import-bench *args='':
    bun scripts/servo-gpu-import-benchmark.ts {{ args }}

# Run repeatable Servo GPU import A/B measurements with managed daemon restarts
servo-import-compare *args='':
    bun scripts/servo-gpu-import-compare.ts {{ args }}

# Compile and smoke-run benchmark targets without full measurement
bench-smoke:
    ./scripts/cargo-cache-build.sh cargo test -p hypercolor-core --bench core_pipeline
    ./scripts/cargo-cache-build.sh cargo test -p hypercolor-hal --bench protocol_encoding
    ./scripts/cargo-cache-build.sh cargo test -p hypercolor-daemon --bench render_pipeline

# Run the core benchmark suite (Criterion HTML reports land in the configured Cargo target)
bench-core *args='':
    ./scripts/cargo-cache-build.sh cargo bench -p hypercolor-core --bench core_pipeline -- {{ args }}

# Run the HAL protocol benchmark suite
bench-hal *args='':
    ./scripts/cargo-cache-build.sh cargo bench -p hypercolor-hal --bench protocol_encoding -- {{ args }}

# Run the daemon render-pipeline benchmark suite
bench-daemon *args='':
    ./scripts/cargo-cache-build.sh cargo bench -p hypercolor-daemon --bench render_pipeline -- {{ args }}

# Measure D3D11 capture reduction with Windows-native build caching
[windows]
bench-windows-capture *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo bench -p hypercolor-windows-capture --features capture-bench --bench capture_reduction -- {{ args }}

# Run all benchmark suites
bench:
    just bench-core
    just bench-hal
    just bench-daemon

# Check local Criterion output against render-pipeline warning budgets
bench-gate *args='':
    ./scripts/graphics-benchmark-gate.sh {{ args }}

# Save a named Criterion baseline for all benchmark suites
bench-baseline name:
    just bench-core -- --save-baseline {{ name }}
    just bench-hal -- --save-baseline {{ name }}
    just bench-daemon -- --save-baseline {{ name }}

# Compare all benchmark suites against a named Criterion baseline
bench-compare name:
    just bench-core -- --baseline {{ name }}
    just bench-hal -- --baseline {{ name }}
    just bench-daemon -- --baseline {{ name }}

# ─── Linting & Formatting ────────────────────────────────

# Run clippy with deny warnings
[unix]
lint *args='':
    ./scripts/cargo-cache-build.sh cargo clippy {{ workspace_args }} {{ test_features }} --all-targets -- -D warnings {{ args }}

[windows]
lint *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo clippy {{ workspace_args }} {{ test_features }} --all-targets -- -D warnings {{ args }}

# Fix clippy suggestions automatically
[unix]
lint-fix *args='':
    ./scripts/cargo-cache-build.sh cargo clippy {{ workspace_args }} --all-targets --fix --allow-dirty --allow-staged {{ args }}

[windows]
lint-fix *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo clippy {{ workspace_args }} --all-targets --fix --allow-dirty --allow-staged {{ args }}

# Apply automatic Rust and SDK fixes
fix *args='':
    just lint-fix {{ args }}
    just fmt
    just sdk-fix
    @echo '✅ Automatic fixes applied'

# Format all code
[unix]
fmt:
    cargo fmt --all

[windows]
fmt:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-fmt-workspace.ps1

# Check formatting without modifying
[unix]
fmt-check:
    cargo fmt --all -- --check
    cargo fmt --manifest-path crates/hypercolor-ui/Cargo.toml -- --check

[windows]
fmt-check:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-fmt-workspace.ps1 -Check

# Format all Markdown prose with Prettier
prettier:
    npx --yes prettier --write "**/*.md"

# Check Markdown prose formatting without modifying
prettier-check:
    npx --yes prettier --check "**/*.md"

# Format all code (rustfmt) and prose (prettier)
format: fmt prettier
    @echo '✅ Formatted'

# ─── Supply Chain ─────────────────────────────────────────

# Audit dependencies (licenses, advisories, bans)
[unix]
deny *args='':
    ./scripts/cargo-cache-build.sh cargo deny check {{ args }}

[windows]
deny *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo deny check {{ args }}

# ─── Documentation ────────────────────────────────────────

# Build docs for all crates
[unix]
doc *args='':
    ./scripts/cargo-cache-build.sh cargo doc {{ workspace_args }} --no-deps {{ args }}

[windows]
doc *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo doc {{ workspace_args }} --no-deps {{ args }}

# Build and open docs in browser
doc-open: (doc "--open")

# Serve the Zola documentation site (hot reload on :9440)
docs-dev:
    cd docs && zola serve --port 9440

# Build the Zola documentation site
docs-build:
    cd docs && zola build

# ─── Running ──────────────────────────────────────────────

# Run the daemon with the full renderer set enabled
[unix]
daemon *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview -- --log-level debug {{ args }}

[windows]
daemon *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview -- --log-level debug {{ args }}

# Run the daemon with the GPU compositor explicitly selected
[unix]
daemon-wgpu *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features wgpu -- --log-level debug --compositor-acceleration-mode gpu {{ args }}

[windows]
daemon-wgpu *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features wgpu -- --log-level debug --compositor-acceleration-mode gpu {{ args }}

# Run the CLI
[unix]
cli *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-cli --bin hypercolor -- {{ args }}

[windows]
cli *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo run -p hypercolor-cli --bin hypercolor -- {{ args }}

# Build UI + effects so tauri.conf.json's workspace-relative resource paths exist.
# Both targets are incremental, so no-op rebuilds are cheap and we never bundle stale artifacts.
app-assets:
    just ui-build
    just effects-build

# Run the unified desktop app (iteration-shaped: keeps incremental rebuilds)
[unix]
app *args='': app-assets
    HYPERCOLOR_ITERATE=1 ./scripts/cargo-cache-build.sh cargo build -p hypercolor-daemon --bin hypercolor-daemon -p hypercolor-app --bin hypercolor-app --profile preview
    "${CARGO_TARGET_DIR:-target}/preview/hypercolor-app" {{ args }}

[windows]
app *args='': app-assets
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo run -p hypercolor-app --bin hypercolor-app -- {{ args }}

# Build the unified desktop app
[unix]
app-build *args='': app-assets
    ./scripts/cargo-cache-build.sh cargo build -p hypercolor-daemon --bin hypercolor-daemon -p hypercolor-app --bin hypercolor-app --profile preview {{ args }}

[windows]
app-build *args='': app-assets
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo build -p hypercolor-app --bin hypercolor-app {{ args }}

# Build the native sidecars consumed by the Tauri bundle stage.
[unix]
app-bundle-binaries:
    ./scripts/cargo-cache-build.sh cargo build --release -p hypercolor-daemon --bin hypercolor-daemon -p hypercolor-cli --bin hypercolor

[windows]
app-bundle-binaries:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo build --release -p hypercolor-daemon --bin hypercolor-daemon -p hypercolor-cli --bin hypercolor -p hypercolor-windows-pawnio --bin hypercolor-smbus-service -p hypercolor-windows-helper --bin hypercolor-windows-helper

# Stage triple-suffixed sidecars (and Windows-only PawnIO/SMBus payloads) under target/bundle-stage/
[unix]
app-bundle-assets *args='': app-bundle-binaries
    ./scripts/stage-app-bundle-assets.sh {{ args }}

[windows]
app-bundle-assets *args='': app-bundle-binaries
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/stage-app-bundle-assets.ps1 {{ args }}

# Build native Tauri bundles for the unified desktop app. On macOS the
# bundle signs with APPLE_SIGNING_IDENTITY, falling back to the local
# "Hypercolor Dev" certificate so TCC grants survive rebuilds (ad-hoc
# signatures change identity every build); see docs/development/DEV_SETUP.md.
[unix]
app-bundle *args='': app-assets app-bundle-assets
    cd crates/hypercolor-app && APPLE_SIGNING_IDENTITY="$(../../scripts/macos-dev-signing-identity.sh)" HYPERCOLOR_FORCE_SCCACHE=1 ../../scripts/cargo-cache-build.sh cargo tauri build --config tauri.bundle.conf.json {{ args }}
    ./scripts/macos-dev-postsign.sh

[windows]
app-bundle *args='': app-assets app-bundle-assets
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -Command "Set-Location crates/hypercolor-app; cargo tauri build --config tauri.bundle.conf.json --config tauri.windows.bundle.conf.json {{ args }}"

# Build the full unsigned Windows NSIS installer package
[windows]
windows-installer *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/build-windows-installer.ps1 {{ args }}

# Build the macOS .app + .dmg bundle (unsigned unless APPLE_SIGNING_IDENTITY is set)
[macos]
mac-installer *args='':
    ./scripts/build-mac-installer.sh {{ args }}

# Regenerate the app icon set from the brand masters (assets/brand)
mac-icons:
    ./scripts/generate-mac-icons.sh

# Run the daemon in release mode with the full renderer set enabled
[unix]
daemon-release *args='':
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --release -- {{ args }}

[windows]
daemon-release *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo run -p hypercolor-daemon --bin hypercolor-daemon --release -- {{ args }}

# Diagnose Windows service, PawnIO, and daemon API state
[windows]
windows-diagnose *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/diagnose-windows.ps1 {{ args }}

# Build the narrow Windows SMBus broker service binary.
[windows]
windows-smbus-service-build:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo build -p hypercolor-windows-pawnio --bin hypercolor-smbus-service --profile preview

# Install the narrow Windows SMBus broker service. Run from an elevated shell.
[windows]
windows-smbus-service-install *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo build -p hypercolor-windows-pawnio --bin hypercolor-smbus-service --profile preview
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/install-windows-smbus-service.ps1 {{ args }}

# Uninstall the narrow Windows SMBus broker service. Run from an elevated shell.
[windows]
windows-smbus-service-uninstall *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/uninstall-windows-service.ps1 -ServiceName HypercolorSmBus {{ args }}

# Install the Windows service. Run from an elevated shell.
[windows]
windows-service-install *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/install-windows-service.ps1 {{ args }}

# Uninstall the Windows service. Run from an elevated shell.
[windows]
windows-service-uninstall *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/uninstall-windows-service.ps1 {{ args }}

# Create or update a virtual display simulator, apply an effect, and print a browser preview URL
simulator-demo *args='':
    ./scripts/simulator-demo.sh {{ args }}

# Create or update a simulator and wait for its frame endpoint to produce image data
simulator-smoke *args='':
    ./scripts/simulator-demo.sh --ephemeral --wait-frame {{ args }}

# Run Servo daemon (dev profile) with cache wrapper
[unix]
daemon-servo *args='':
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features servo -- --log-level debug --bind '{{ daemon_bind }}' {{ args }}

[windows]
daemon-servo *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features servo -- --log-level debug --bind '{{ daemon_bind }}' {{ args }}

# Run Servo daemon with the GPU compositor enabled
[unix]
daemon-servo-wgpu *args='':
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features "servo wgpu" -- --log-level debug --compositor-acceleration-mode gpu --bind '{{ daemon_bind }}' {{ args }}

[windows]
daemon-servo-wgpu *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features "servo wgpu" -- --log-level debug --compositor-acceleration-mode gpu --bind '{{ daemon_bind }}' {{ args }}

# Run Servo daemon in release mode with cache wrapper
[unix]
daemon-servo-release *args='':
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --release --features servo -- --bind '{{ daemon_bind }}' {{ args }}

[windows]
daemon-servo-release *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo run -p hypercolor-daemon --bin hypercolor-daemon --release --features servo -- --bind '{{ daemon_bind }}' {{ args }}

# Build Servo daemon release artifacts once (faster repeat launches)
[unix]
build-servo-release:
    ./scripts/servo-cache-build.sh cargo build -p hypercolor-daemon --release --features servo

[windows]
build-servo-release:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo build -p hypercolor-daemon --release --features servo

# Run prebuilt Servo daemon release binary from the configured target dir
run-servo-release-bin *args='':
    "${CARGO_TARGET_DIR:-target}/release/hypercolor-daemon" --bind '{{ daemon_bind }}' {{ args }}

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
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features servo -- --log-level debug --bind '{{ daemon_bind }}' &
    sleep 2
    ./scripts/cargo-cache-build.sh cargo run -p hypercolor-cli --bin hypercolor -- tui {{ args }} &
    wait

# ─── UI ──────────────────────────────────────────────────

ui-deps:
    cd crates/hypercolor-ui && bun install --frozen-lockfile

[private]
prepare-dev-assets:
    cd sdk && bun scripts/build-effect.ts --all

# Run Servo daemon + UI dev server together (daemon bind from config, UI on :9430)
[unix]
dev *args='': ui-deps
    #!/usr/bin/env bash
    set -euo pipefail
    daemon_pid=""
    trunk_pid=""

    send_signal() {
      local signal="$1"
      local pid="$2"
      [[ -n "${pid}" ]] || return 0
      pkill "-${signal}" -P "${pid}" 2>/dev/null || true
      kill "-${signal}" "${pid}" 2>/dev/null || true
    }

    cleanup() {
      local status=$?
      trap - EXIT INT TERM
      send_signal TERM "${trunk_pid}"
      send_signal INT "${daemon_pid}"
      [[ -z "${trunk_pid}" ]] || wait "${trunk_pid}" 2>/dev/null || true
      [[ -z "${daemon_pid}" ]] || wait "${daemon_pid}" 2>/dev/null || true
      exit "${status}"
    }

    wait_for_first_exit() {
      local status
      while true; do
        if ! jobs -pr | grep -qx "${daemon_pid}"; then
          set +e
          wait "${daemon_pid}"
          status=$?
          set -e
          return "${status}"
        fi
        if ! jobs -pr | grep -qx "${trunk_pid}"; then
          set +e
          wait "${trunk_pid}"
          status=$?
          set -e
          return "${status}"
        fi
        sleep 0.25
      done
    }

    trap cleanup EXIT INT TERM
    just prepare-dev-assets
    daemon_args=(--log-level debug)
    if [[ -n "${HYPERCOLOR_COMPOSITOR_ACCELERATION_MODE:-}" ]]; then
      daemon_args+=(--compositor-acceleration-mode "${HYPERCOLOR_COMPOSITOR_ACCELERATION_MODE}")
      echo "[dev] compositor acceleration mode: ${HYPERCOLOR_COMPOSITOR_ACCELERATION_MODE}"
    else
      echo "[dev] compositor acceleration mode: config"
    fi
    servo_gpu_import_mode="${HYPERCOLOR_SERVO_GPU_IMPORT_MODE:-auto}"
    daemon_args+=(--servo-gpu-import-mode "${servo_gpu_import_mode}")
    if [[ -n "${HYPERCOLOR_SERVO_GPU_IMPORT_MODE:-}" ]]; then
      echo "[dev] Servo GPU import mode: ${servo_gpu_import_mode}"
    else
      echo "[dev] Servo GPU import mode: ${servo_gpu_import_mode} (default)"
    fi
    ./scripts/servo-cache-build.sh cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features "servo wgpu servo-gpu-import" -- "${daemon_args[@]}" {{ args }} &
    daemon_pid=$!
    sleep 2
    (cd crates/hypercolor-ui && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}" HYPERCOLOR_ITERATE=1 env -u NO_COLOR ../../scripts/cargo-cache-build.sh trunk serve --dist .dist-dev) &
    trunk_pid=$!
    wait_for_first_exit

[windows]
dev *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/dev-windows.ps1 {{ args }}

# Start the UI dev server (Trunk + hot reload, :9430 by default). Pass a
# port and optionally a bind address to run beside another stack:
# `just ui-dev 9431`, or `just ui-dev 9431 0.0.0.0` to reach it from a
# phone on the LAN. The API proxy target (:9420) is unaffected.
ui-dev port='9430' host='127.0.0.1': ui-deps
    cd crates/hypercolor-ui && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}" HYPERCOLOR_ITERATE=1 env -u NO_COLOR ../../scripts/cargo-cache-build.sh trunk serve --dist .dist-dev --port {{ port }} --address {{ host }}

# Build the UI for production
ui-build: ui-deps
    cd crates/hypercolor-ui && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}" HYPERCOLOR_FORCE_SCCACHE=1 env -u NO_COLOR ../../scripts/cargo-cache-build.sh trunk build --release --locked

# Build UI and copy dist for daemon embedding
ui-dist: ui-build
    @echo '✅ UI built at crates/hypercolor-ui/dist/'

# Install e2e harness dependencies
e2e-install:
    cd e2e && npm ci

# Install Playwright browsers for the e2e harness
e2e-browsers:
    cd e2e && npx playwright install chromium

# Build the normal Servo daemon, CLI, generated effects, and production web UI for e2e
e2e-build:
    ./scripts/cargo-cache-build.sh cargo build -p hypercolor-daemon -p hypercolor-cli
    just effects-build
    just ui-build

# Build the fallback CPU smoke stack without the Servo renderer.
# Isolated target lane: the --no-default-features shape unifies crate features
# differently from the daily builds, and letting it share target/ churns and
# strands artifacts for the whole dependency graph on every alternation.
# CI pins CARGO_TARGET_DIR per lane, so an ambient value wins.
e2e-build-cpu:
    CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-{{ justfile_directory() }}/target/cpu-smoke}" ./scripts/cargo-cache-build.sh cargo build -p hypercolor-daemon --no-default-features --features builtin-drivers
    CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-{{ justfile_directory() }}/target/cpu-smoke}" ./scripts/cargo-cache-build.sh cargo build -p hypercolor-cli
    just effects-build
    just ui-build

# Run the Playwright e2e suite against a hermetic local stack
e2e *args='':
    cd e2e && npm test -- {{ args }}

# Run the standalone UI crate tests
[unix]
ui-test:
    HYPERCOLOR_ITERATE=1 ./scripts/cargo-cache-build.sh cargo test --manifest-path crates/hypercolor-ui/Cargo.toml

[windows]
ui-test:
    HYPERCOLOR_ITERATE=1 powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-cache-build.ps1 cargo test --manifest-path crates/hypercolor-ui/Cargo.toml

# ─── SDK ─────────────────────────────────────────────────

# Install SDK dependencies
sdk-install:
    cd sdk && bun install

# Build SDK packages
sdk-build:
    cd sdk && bun run build

# Watch-rebuild the SDK packages on change
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
    cd sdk && bun run build:effect src/effects/{{ name }}/main.ts

# Build all SDK faces → effects/hypercolor/*.html
faces-build:
    cd sdk && bun run build:faces

# Build a single SDK face (e.g., just face-build silkcircuit-hud)
face-build name:
    cd sdk && bun run build:effect src/faces/{{ name }}/main.ts

# Face authoring loop: build+install+assign to simulator displays, rebuild on save
face-dev name:
    cd sdk && bun scripts/face-dev.ts {{ name }}

# Capture screenshots for every effect via the running daemon (writes effects/screenshots/drafts/)
capture-screenshots *FLAGS:
    cd sdk && bun run capture:screenshots {{ FLAGS }}

# Capture display-face screenshots on the Face Dev simulators (writes effects/screenshots/drafts/)
capture-faces *FLAGS:
    cd sdk && bun run capture:faces {{ FLAGS }}

# Generate the drafts approval gallery (drafts-browser.html at repo root)
browse-drafts:
    cd sdk && bun run capture:browse

# Promote draft rank-1 frames into effects/screenshots/curated/ as WebP q=0.92
promote-screenshots:
    cd sdk && bun run capture:promote

# Install card artwork as sdk/src/<kind>/<id>/cover.webp so builds embed it inline
sync-covers *FLAGS:
    cd sdk && bun run covers:sync {{ FLAGS }}

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

# Bootstrap the dev environment (system pkgs, Rust toolchain, cargo tools, bun, frontend deps)
[unix]
setup *args='':
    ./scripts/setup.sh {{ args }}

[windows]
setup *args='':
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/setup.ps1 {{ args }}

# Quickly add the wasm target only (faster than full setup)
setup-wasm:
    rustup target add wasm32-unknown-unknown

# Build the ready-to-ship distribution bundle and tarball
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
    sudo cp udev/70-hypercolor-input.rules /etc/udev/rules.d/
    sudo udevadm control --reload-rules
    sudo udevadm trigger --action=add --subsystem-match=hidraw
    sudo udevadm trigger --action=add --subsystem-match=usb
    sudo udevadm trigger --action=add --subsystem-match=tty
    sudo udevadm trigger --action=add --subsystem-match=i2c-dev
    sudo udevadm trigger --action=add --subsystem-match=input
    @echo '✅ udev rules installed and applied'

# ─── Housekeeping ─────────────────────────────────────────

# Clean build artifacts
clean:
    ./scripts/cargo-cache-build.sh cargo clean

# Plain sh lines, no shebang blocks: shebang recipes resolve `bash` from
# PATH, which on Windows can be WSL bash that cannot read the temp script
# path. `sh -cu` lines are the pattern every [windows] recipe already
# proves.

# Report build artifact and shared cache disk usage for this checkout
disk:
    @echo '── target profiles ──'
    @if [ -d "${CARGO_TARGET_DIR:-{{ justfile_directory() }}/target}" ]; then du -sh "${CARGO_TARGET_DIR:-{{ justfile_directory() }}/target}"/* 2>/dev/null | sort -rh | head -15 || true; else echo '(no target dir)'; fi
    @echo "── shared caches (${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}) ──"
    @if [ -d "${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}" ]; then du -sh "${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}"/* 2>/dev/null | sort -rh || true; else echo '(no cache dir)'; fi
    @if command -v sccache >/dev/null 2>&1; then echo '── sccache ──'; SCCACHE_SERVER_UDS="${SCCACHE_SERVER_UDS:-${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}/sccache.sock}" SCCACHE_DIR="${SCCACHE_DIR:-${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}/sccache}" sccache --show-stats | grep -E 'Cache hits|Cache misses|Cache size|Max cache' || true; fi

# Preview pressure-triggered collection across public and proprietary worktrees
[linux]
gc:
    ./scripts/cargo-target-gc.sh --dry-run

# Apply pressure-triggered collection across public and proprietary worktrees
[linux]
gc-apply:
    ./scripts/cargo-target-gc.sh --apply

# Reclaim pressure immediately while preserving dirty and Cargo-locked profiles
[linux]
gc-reclaim:
    ./scripts/cargo-target-gc.sh --reclaim-now

# Install and enable the daily user timer
[linux]
gc-install *args='':
    ./scripts/install-cargo-target-gc.sh {{ args }}

# Show the next scheduled collection and the previous service result
[linux]
gc-status:
    systemctl --user list-timers hypercolor-cargo-target-gc.timer --no-pager
    systemctl --user show hypercolor-cargo-target-gc.service --property=Result,ExecMainStatus
    @if [ -f "$HOME/.local/share/hypercolor/libexec/cargo-target-gc" ]; then sha256sum scripts/cargo-target-gc.sh "$HOME/.local/share/hypercolor/libexec/cargo-target-gc"; else echo '(collector is not installed)'; fi

# Show workspace dependency tree
deps:
    cargo tree --workspace

# Show outdated dependencies
outdated:
    cargo outdated -wR

# Count lines of code (requires tokei)
loc:
    @tokei crates/ --sort code 2>/dev/null || echo 'Install tokei: cargo install tokei'
