#!/usr/bin/env bash
# install-release.sh — Hypercolor prebuilt binary installer
#
# Usage:
#   curl -fsSL https://install.hypercolor.dev | bash
#   curl -fsSL https://install.hypercolor.dev | bash -s -- --version v0.5.0
#   curl -fsSL https://install.hypercolor.dev | bash -s -- --uninstall
#
# Environment:
#   HYPERCOLOR_INSTALL_PREFIX  Override install prefix (default: ~/.local)
#   HYPERCOLOR_INSTALL_DIR     Override install directory (default: <prefix>/bin)
#   NO_COLOR                Disable colored output
#
# Flags:
#   --version <tag>   Install a specific release (default: latest)
#   --no-service      Skip systemd/launchd service setup
#   --uninstall       Remove Hypercolor (prompts for confirmation)
#   --yes             Skip confirmation prompts (for CI)

set -euo pipefail

# ─── Constants ────────────────────────────────────────────────────────────────

GITHUB_REPO="hyperb1iss/hypercolor"
GITHUB_API="https://api.github.com"
GITHUB_DL="https://github.com/${GITHUB_REPO}/releases/download"
INSTALL_PREFIX="${HYPERCOLOR_INSTALL_PREFIX:-${HOME}/.local}"
INSTALL_DIR="${HYPERCOLOR_INSTALL_DIR:-${INSTALL_PREFIX}/bin}"
DATA_DIR="${INSTALL_PREFIX}/share/hypercolor"
UI_DIR="${DATA_DIR}/ui"
EFFECTS_DIR="${DATA_DIR}/effects/bundled"
BASH_COMPLETION_DIR="${INSTALL_PREFIX}/share/bash-completion/completions"
ZSH_COMPLETION_DIR="${INSTALL_PREFIX}/share/zsh/site-functions"
FISH_COMPLETION_DIR="${HOME}/.config/fish/completions"

SYSTEMD_DIR="${HOME}/.config/systemd/user"
DESKTOP_DIR="${INSTALL_PREFIX}/share/applications"
ICONS_DIR="${INSTALL_PREFIX}/share/icons"
LAUNCHD_DIR="${HOME}/Library/LaunchAgents"
LAUNCHD_LABEL="tech.hyperbliss.hypercolor"
LAUNCHD_PLIST="${LAUNCHD_DIR}/${LAUNCHD_LABEL}.plist"
LOG_DIR="${HOME}/Library/Logs/hypercolor"

UDEV_RULES_PATH="/etc/udev/rules.d/99-hypercolor.rules"

VERSION=""
NO_SERVICE=false
UNINSTALL=false
SKIP_CONFIRM=false
RELEASE_DIR=""

# ─── Colors ───────────────────────────────────────────────────────────────────

setup_colors() {
    if [[ -n "${NO_COLOR:-}" ]] || [[ ! -t 1 ]]; then
        BOLD="" DIM="" RESET=""
        MAGENTA="" CYAN="" GREEN="" RED="" YELLOW=""
    else
        BOLD="\033[1m" DIM="\033[2m" RESET="\033[0m"
        MAGENTA="\033[38;5;198m"   # SilkCircuit magenta accent
        CYAN="\033[38;5;87m"       # SilkCircuit cyan accent
        GREEN="\033[38;5;84m"
        RED="\033[38;5;196m"
        YELLOW="\033[38;5;220m"
    fi
}

# ─── Output helpers ───────────────────────────────────────────────────────────

info()    { printf "${CYAN}  ▸${RESET} %s\n" "$*"; }
success() { printf "${GREEN}  ✓${RESET} %s\n" "$*"; }
warn()    { printf "${YELLOW}  ⚠${RESET} %s\n" "$*" >&2; }
error()   { printf "${RED}  ✗${RESET} %s\n" "$*" >&2; }
fatal()   { error "$@"; exit 1; }

banner() {
    printf "\n"
    printf "${MAGENTA}${BOLD}"
    printf "  ╦ ╦┬ ┬┌─┐┌─┐┬─┐┌─┐┌─┐┬  ┌─┐┬─┐\n"
    printf "  ╠═╣└┬┘├─┘├┤ ├┬┘│  │ ││  │ │├┬┘\n"
    printf "  ╩ ╩ ┴ ┴  └─┘┴└─└─┘└─┘┴─┘└─┘┴└─\n"
    printf "${RESET}"
    printf "${DIM}  RGB Lighting Orchestration Engine${RESET}\n"
    printf "\n"
}

# ─── Argument parsing ─────────────────────────────────────────────────────────

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --version)
                [[ $# -lt 2 ]] && fatal "--version requires a tag argument"
                VERSION="$2"
                shift 2
                ;;
            --no-service)
                NO_SERVICE=true
                shift
                ;;
            --uninstall)
                UNINSTALL=true
                shift
                ;;
            --yes|-y)
                SKIP_CONFIRM=true
                shift
                ;;
            --help|-h)
                usage
                exit 0
                ;;
            *)
                fatal "Unknown option: $1 (try --help)"
                ;;
        esac
    done
}

usage() {
    cat <<'USAGE'
Usage: install-release.sh [OPTIONS]

Options:
  --version <tag>   Install a specific version (default: latest)
  --no-service      Skip systemd/launchd service setup
  --uninstall       Remove Hypercolor installation
  --yes, -y         Skip confirmation prompts
  --help, -h        Show this help message

Environment:
  HYPERCOLOR_INSTALL_PREFIX  Override install prefix (default: ~/.local)
  HYPERCOLOR_INSTALL_DIR     Override install directory (default: <prefix>/bin)
  NO_COLOR                   Disable colored output
USAGE
}

# ─── Platform detection ───────────────────────────────────────────────────────

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    # Normalize architecture
    case "$ARCH" in
        x86_64)  ARCH="x86_64" ;;
        aarch64) ARCH="aarch64" ;;
        arm64)   ARCH="aarch64" ;;
        *)       fatal "Unsupported architecture: ${ARCH}" ;;
    esac

    # Build artifact suffix
    case "${OS}-${ARCH}" in
        Linux-x86_64)   ARTIFACT_SUFFIX="linux-amd64" ;;
        Linux-aarch64)  ARTIFACT_SUFFIX="linux-arm64" ;;
        Darwin-aarch64) ARTIFACT_SUFFIX="macos-arm64" ;;
        *)              fatal "Unsupported platform: ${OS} ${ARCH}" ;;
    esac

    info "Detected platform: ${OS} ${ARCH} (${ARTIFACT_SUFFIX})"
}

# ─── Prerequisite checks ─────────────────────────────────────────────────────

check_dependencies() {
    local missing=()
    for cmd in curl tar; do
        command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        fatal "Missing required tools: ${missing[*]}"
    fi
}

# ─── GitHub API helpers ───────────────────────────────────────────────────────

fetch_latest_version() {
    if [[ -n "$VERSION" ]]; then
        info "Using specified version: ${VERSION}"
        return
    fi

    info "Fetching latest release..."
    local response
    response="$(curl -fsSL "${GITHUB_API}/repos/${GITHUB_REPO}/releases/latest" 2>&1)" \
        || fatal "Failed to fetch latest release from GitHub API. Check your internet connection."

    VERSION="$(printf '%s' "$response" | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"

    if [[ -z "$VERSION" ]]; then
        fatal "Could not determine latest version from GitHub API response"
    fi

    info "Latest version: ${VERSION}"
}

download_release_artifact() {
    local version_no_v="${VERSION#v}"
    local tarball="hypercolor-${version_no_v}-${ARTIFACT_SUFFIX}.tar.gz"
    local url="${GITHUB_DL}/${VERSION}/${tarball}"
    local dest="${TMPDIR_INSTALL}/${tarball}"

    info "Downloading ${tarball}..."
    if ! curl -fsSL --progress-bar -o "$dest" "$url"; then
        fatal "Failed to download ${url}"
    fi

    if [[ ! -s "$dest" ]]; then
        fatal "Downloaded file is empty: ${tarball}"
    fi

    info "Extracting ${tarball}..."
    tar -xzf "$dest" -C "$TMPDIR_INSTALL"

    RELEASE_DIR="${TMPDIR_INSTALL}/hypercolor-${version_no_v}-${ARTIFACT_SUFFIX}"
    if [[ ! -d "$RELEASE_DIR" ]]; then
        fatal "Unexpected archive layout in ${tarball}"
    fi

    success "Downloaded release payload"
}

# ─── Temp directory with cleanup ──────────────────────────────────────────────

TMPDIR_INSTALL=""

setup_tmpdir() {
    TMPDIR_INSTALL="$(mktemp -d 2>/dev/null || mktemp -d -t hypercolor-install)"
    trap cleanup EXIT INT TERM
}

cleanup() {
    if [[ -n "$TMPDIR_INSTALL" ]] && [[ -d "$TMPDIR_INSTALL" ]]; then
        rm -rf "$TMPDIR_INSTALL"
    fi
}

# ─── Install logic ────────────────────────────────────────────────────────────

install_release_payload() {
    mkdir -p "$INSTALL_DIR"

    # Stop existing service before replacing files (idempotent)
    stop_service_if_running

    local bin
    for bin in hypercolor-daemon hypercolor hypercolor-tray hypercolor-tui hypercolor-open; do
        if [[ -f "${RELEASE_DIR}/bin/${bin}" ]]; then
            install -Dm755 "${RELEASE_DIR}/bin/${bin}" "${INSTALL_DIR}/${bin}"
        fi
    done
    success "Installed binaries to ${INSTALL_DIR}/"

    if [[ -d "${RELEASE_DIR}/share/hypercolor/ui" ]]; then
        rm -rf "$UI_DIR"
        mkdir -p "$UI_DIR"
        cp -R "${RELEASE_DIR}/share/hypercolor/ui/." "$UI_DIR/"
        success "Installed bundled UI to ${UI_DIR}"
    fi

    if [[ -d "${RELEASE_DIR}/share/hypercolor/effects" ]]; then
        rm -rf "${DATA_DIR}/effects"
        mkdir -p "$(dirname "$EFFECTS_DIR")"
        cp -R "${RELEASE_DIR}/share/hypercolor/effects/." "${DATA_DIR}/effects/"
        success "Installed bundled effects to ${EFFECTS_DIR}"
    fi

    install_desktop_entry
    install_icons
    install_completions
    check_path
}

check_path() {
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            warn "${INSTALL_DIR} is not in your PATH"
            printf "\n"
            info "Add it to your shell profile:"
            printf "    ${DIM}export PATH=\"%s:\$PATH\"${RESET}\n" "$INSTALL_DIR"
            printf "\n"
            ;;
    esac
}

stop_service_if_running() {
    case "$OS" in
        Linux)
            if command -v systemctl >/dev/null 2>&1; then
                systemctl --user stop hypercolor.service 2>/dev/null || true
            fi
            ;;
        Darwin)
            if launchctl list "$LAUNCHD_LABEL" >/dev/null 2>&1; then
                launchctl unload "$LAUNCHD_PLIST" 2>/dev/null || true
            fi
            ;;
    esac
}

# ─── Linux: systemd service ──────────────────────────────────────────────────

install_systemd_service() {
    if [[ "$NO_SERVICE" == true ]]; then
        info "Skipping systemd service setup (--no-service)"
        return
    fi

    if ! command -v systemctl >/dev/null 2>&1; then
        warn "systemctl not found, skipping service setup"
        return
    fi

    mkdir -p "$SYSTEMD_DIR"

    cat > "${SYSTEMD_DIR}/hypercolor.service" <<UNIT
[Unit]
Description=Hypercolor RGB Lighting Daemon
Documentation=https://github.com/hyperb1iss/hypercolor
After=graphical-session.target dbus.socket
Wants=graphical-session.target

[Service]
Type=notify
ExecStart=${INSTALL_DIR}/hypercolor-daemon --ui-dir ${UI_DIR}
WatchdogSec=30
Restart=on-failure
RestartSec=3
Environment=HYPERCOLOR_LOG=info
Environment=RUST_BACKTRACE=1
MemoryMax=512M
CPUQuota=25%
ProtectHome=read-only
ProtectSystem=strict
ReadWritePaths=%h/.config/hypercolor ${DATA_DIR} %h/.local/state/hypercolor
PrivateTmp=true
NoNewPrivileges=true

[Install]
WantedBy=default.target
UNIT

    success "Installed systemd service to ${SYSTEMD_DIR}/hypercolor.service"

    systemctl --user daemon-reload
    systemctl --user enable hypercolor.service 2>/dev/null || true
    systemctl --user start hypercolor.service 2>/dev/null || true

    success "Enabled and started hypercolor.service"
}

# ─── Linux: desktop entry ────────────────────────────────────────────────────

install_desktop_entry() {
    local source="${RELEASE_DIR}/share/applications/hypercolor.desktop"
    if [[ ! -f "$source" ]]; then
        warn "Release payload does not contain a desktop entry, skipping"
        return
    fi

    mkdir -p "$DESKTOP_DIR"
    sed "s|Exec=/usr/bin/|Exec=${INSTALL_DIR}/|g" "$source" > "${DESKTOP_DIR}/hypercolor.desktop"
    success "Installed desktop entry to ${DESKTOP_DIR}/hypercolor.desktop"
}

install_icons() {
    local source="${RELEASE_DIR}/share/icons"
    if [[ ! -d "$source" ]]; then
        warn "Release payload does not contain icons, skipping"
        return
    fi

    mkdir -p "$ICONS_DIR"
    cp -R "${source}/." "$ICONS_DIR/"
    success "Installed icons to ${ICONS_DIR}"
}

# ─── Linux: udev rules ───────────────────────────────────────────────────────

prompt_udev_rules() {
    if [[ -f "$UDEV_RULES_PATH" ]]; then
        info "udev rules already installed at ${UDEV_RULES_PATH}"
        return
    fi

    printf "\n"
    info "Hypercolor needs udev rules for USB device access."
    info "This requires sudo to install to ${UDEV_RULES_PATH}"
    printf "\n"

    if [[ "$SKIP_CONFIRM" != true ]]; then
        printf "  Install udev rules now? [y/N] "
        read -r answer
        case "$answer" in
            [yY]|[yY][eE][sS]) ;;
            *) info "Skipping udev rules (you can install them later)"; return ;;
        esac
    fi

    local rules_src="${RELEASE_DIR}/lib/udev/rules.d/99-hypercolor.rules"
    if [[ ! -f "$rules_src" ]]; then
        warn "Release payload does not contain udev rules, skipping"
        return
    fi

    info "Installing udev rules..."
    sudo install -Dm644 "$rules_src" "$UDEV_RULES_PATH"

    # Load i2c-dev module if not already loaded
    if ! lsmod 2>/dev/null | grep -q i2c_dev; then
        sudo modprobe i2c-dev 2>/dev/null || true
    fi

    # Persist i2c-dev module across reboots
    local module_src="${RELEASE_DIR}/etc/modules-load.d/i2c-dev.conf"
    if [[ -f "$module_src" ]]; then
        sudo install -Dm644 "$module_src" /etc/modules-load.d/i2c-dev.conf
    fi

    # Reload udev
    sudo udevadm control --reload-rules 2>/dev/null || true
    sudo udevadm trigger 2>/dev/null || true

    success "Installed udev rules and reloaded udev"
}

# ─── macOS: launchd agent ─────────────────────────────────────────────────────

install_launchd_agent() {
    if [[ "$NO_SERVICE" == true ]]; then
        info "Skipping launchd agent setup (--no-service)"
        return
    fi

    mkdir -p "$LAUNCHD_DIR"
    mkdir -p "$LOG_DIR"

    cat > "$LAUNCHD_PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>tech.hyperbliss.hypercolor</string>
    <key>ProgramArguments</key>
    <array>
        <string>${INSTALL_DIR}/hypercolor</string>
        <string>--ui-dir</string>
        <string>${UI_DIR}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>3</integer>
    <key>StandardOutPath</key>
    <string>~/Library/Logs/hypercolor/hypercolor.log</string>
    <key>StandardErrorPath</key>
    <string>~/Library/Logs/hypercolor/hypercolor.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HYPERCOLOR_LOG</key>
        <string>info</string>
        <key>PATH</key>
        <string>/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:${INSTALL_DIR}</string>
    </dict>
    <key>ProcessType</key>
    <string>Standard</string>
    <key>LowPriorityBackgroundIO</key>
    <true/>
</dict>
</plist>
PLIST

    success "Installed launchd plist to ${LAUNCHD_PLIST}"

    launchctl load "$LAUNCHD_PLIST" 2>/dev/null || true
    success "Loaded launchd agent"
}

# ─── Shell completions ────────────────────────────────────────────────────────

install_completions() {
    if [[ -f "${RELEASE_DIR}/share/bash-completion/completions/hypercolor" ]]; then
        mkdir -p "$BASH_COMPLETION_DIR"
        install -Dm644 \
            "${RELEASE_DIR}/share/bash-completion/completions/hypercolor" \
            "${BASH_COMPLETION_DIR}/hypercolor"
    fi

    if [[ -f "${RELEASE_DIR}/share/zsh/site-functions/_hypercolor" ]]; then
        mkdir -p "$ZSH_COMPLETION_DIR"
        install -Dm644 \
            "${RELEASE_DIR}/share/zsh/site-functions/_hypercolor" \
            "${ZSH_COMPLETION_DIR}/_hypercolor"
    fi

    if [[ -f "${RELEASE_DIR}/share/fish/vendor_completions.d/hypercolor.fish" ]]; then
        mkdir -p "$FISH_COMPLETION_DIR"
        install -Dm644 \
            "${RELEASE_DIR}/share/fish/vendor_completions.d/hypercolor.fish" \
            "${FISH_COMPLETION_DIR}/hypercolor.fish"
    fi

    success "Installed available shell completions"
}

# ─── Main install flow ────────────────────────────────────────────────────────

do_install() {
    banner
    detect_platform
    check_dependencies
    setup_tmpdir
    fetch_latest_version
    download_release_artifact

    printf "\n"
    info "Installing Hypercolor ${VERSION} into ${INSTALL_PREFIX}"
    printf "\n"

    install_release_payload

    case "$OS" in
        Linux)
            install_systemd_service
            prompt_udev_rules
            ;;
        Darwin)
            install_launchd_agent
            ;;
    esac

    install_completions

    # ─── Success summary ──────────────────────────────────────────────────────

    printf "\n"
    printf "  ${GREEN}${BOLD}Hypercolor ${VERSION} installed successfully!${RESET}\n"
    printf "\n"
    printf "  ${DIM}CLI:${RESET}     ${INSTALL_DIR}/hypercolor\n"
    printf "  ${DIM}Daemon:${RESET}  ${INSTALL_DIR}/hypercolor-daemon\n"
    printf "  ${DIM}Open UI:${RESET} ${INSTALL_DIR}/hypercolor-open\n"
    printf "  ${DIM}TUI:${RESET}     ${INSTALL_DIR}/hypercolor-tui\n"
    printf "  ${DIM}Web UI:${RESET}  ${CYAN}http://localhost:9420${RESET}\n"
    printf "\n"
    printf "  ${DIM}Quick start:${RESET}\n"
    printf "    hypercolor status     ${DIM}# Check daemon status${RESET}\n"
    printf "    hypercolor effects list ${DIM}# Browse available effects${RESET}\n"
    printf "    hypercolor devices    ${DIM}# List connected devices${RESET}\n"
    printf "\n"

    if [[ "$NO_SERVICE" == true ]]; then
        printf "  ${DIM}To start manually:${RESET}\n"
        printf "    hypercolor-daemon     ${DIM}# Run in foreground${RESET}\n"
        printf "\n"
    fi
}

# ─── Uninstall ────────────────────────────────────────────────────────────────

do_uninstall() {
    banner

    printf "  ${YELLOW}${BOLD}Uninstall Hypercolor${RESET}\n"
    printf "\n"
    printf "  This will remove:\n"
    printf "    - Binaries from ${INSTALL_DIR}\n"
    printf "    - Bundled UI/effects from ${DATA_DIR}\n"
    printf "    - Service configuration (systemd/launchd)\n"
    printf "    - Desktop entry and shell completions\n"
    printf "\n"
    printf "  ${DIM}Your configuration (~/.config/hypercolor) will be preserved.${RESET}\n"
    printf "\n"

    if [[ "$SKIP_CONFIRM" != true ]]; then
        printf "  Are you sure you want to uninstall? [y/N] "
        read -r answer
        case "$answer" in
            [yY]|[yY][eE][sS]) ;;
            *)
                info "Uninstall cancelled."
                exit 0
                ;;
        esac
        printf "\n"
    fi

    detect_platform

    # Stop and remove services
    case "$OS" in
        Linux)
            if command -v systemctl >/dev/null 2>&1; then
                info "Stopping and disabling systemd service..."
                systemctl --user stop hypercolor.service 2>/dev/null || true
                systemctl --user disable hypercolor.service 2>/dev/null || true
                rm -f "${SYSTEMD_DIR}/hypercolor.service"
                systemctl --user daemon-reload 2>/dev/null || true
                success "Removed systemd service"
            fi

            # Desktop entry
            rm -f "${DESKTOP_DIR}/hypercolor.desktop"
            success "Removed desktop entry"

            # udev rules
            if [[ -f "$UDEV_RULES_PATH" ]]; then
                printf "\n"
                info "udev rules found at ${UDEV_RULES_PATH}"
                if [[ "$SKIP_CONFIRM" != true ]]; then
                    printf "  Remove udev rules? (requires sudo) [y/N] "
                    read -r answer
                    case "$answer" in
                        [yY]|[yY][eE][sS])
                            sudo rm -f "$UDEV_RULES_PATH"
                            sudo udevadm control --reload-rules 2>/dev/null || true
                            success "Removed udev rules"
                            ;;
                        *)
                            info "Keeping udev rules"
                            ;;
                    esac
                else
                    sudo rm -f "$UDEV_RULES_PATH"
                    sudo udevadm control --reload-rules 2>/dev/null || true
                    success "Removed udev rules"
                fi
            fi
            ;;
        Darwin)
            if [[ -f "$LAUNCHD_PLIST" ]]; then
                info "Unloading launchd agent..."
                launchctl unload "$LAUNCHD_PLIST" 2>/dev/null || true
                rm -f "$LAUNCHD_PLIST"
                success "Removed launchd agent"
            fi
            ;;
    esac

    # Remove binaries
    info "Removing binaries..."
    rm -f "${INSTALL_DIR}/hypercolor"
    rm -f "${INSTALL_DIR}/hypercolor-daemon"
    rm -f "${INSTALL_DIR}/hypercolor-tray"
    rm -f "${INSTALL_DIR}/hypercolor-tui"
    rm -f "${INSTALL_DIR}/hypercolor-open"
    success "Removed binaries from ${INSTALL_DIR}"

    # Remove completions
    info "Removing shell completions..."
    rm -f "${BASH_COMPLETION_DIR}/hypercolor"
    rm -f "${ZSH_COMPLETION_DIR}/_hypercolor"
    rm -f "${FISH_COMPLETION_DIR}/hypercolor.fish"
    rm -f "${BASH_COMPLETION_DIR}/hyper"
    rm -f "${ZSH_COMPLETION_DIR}/_hyper"
    rm -f "${FISH_COMPLETION_DIR}/hyper.fish"
    success "Removed shell completions"

    info "Removing bundled UI/effects and desktop assets..."
    rm -rf "${DATA_DIR}"
    rm -f "${DESKTOP_DIR}/hypercolor.desktop"
    rm -f "${ICONS_DIR}/hicolor/scalable/apps/hypercolor.svg"
    rm -f "${ICONS_DIR}/hicolor/48x48/apps/hypercolor.png"
    rm -f "${ICONS_DIR}/hicolor/128x128/apps/hypercolor.png"
    rm -f "${ICONS_DIR}/hicolor/256x256/apps/hypercolor.png"
    success "Removed installed assets"

    printf "\n"
    success "Hypercolor has been uninstalled."
    printf "\n"
    warn "Configuration preserved at ~/.config/hypercolor"
    info "To remove it: rm -rf ~/.config/hypercolor"
    printf "\n"
}

# ─── Entry point ──────────────────────────────────────────────────────────────

main() {
    setup_colors
    parse_args "$@"

    if [[ "$UNINSTALL" == true ]]; then
        do_uninstall
    else
        do_install
    fi
}

main "$@"
