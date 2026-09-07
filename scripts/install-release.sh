#!/usr/bin/env bash
# install-release.sh — Hypercolor prebuilt binary installer
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --version v0.1.0
#   curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --uninstall
#
# Environment:
#   HYPERCOLOR_INSTALL_PREFIX  Override the raw install prefix
#   HYPERCOLOR_INSTALL_DIR     Override the raw binary directory
#   NO_COLOR                Disable colored output
#
# Flags:
#   --version <tag>   Install a specific release (default: latest)
#   --no-service      Preserve raw-direct service state
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
BASH_COMPLETION_DIR="${INSTALL_PREFIX}/share/bash-completion/completions"
ZSH_COMPLETION_DIR="${INSTALL_PREFIX}/share/zsh/site-functions"
FISH_COMPLETION_DIR="${HOME}/.config/fish/completions"

SYSTEMD_DIR="${HOME}/.config/systemd/user"
DESKTOP_DIR="${INSTALL_PREFIX}/share/applications"
ICONS_DIR="${INSTALL_PREFIX}/share/icons"
LAUNCHD_DIR="${HOME}/Library/LaunchAgents"
LAUNCHD_LABEL="tech.hyperbliss.hypercolor"
LAUNCHD_PLIST="${LAUNCHD_DIR}/${LAUNCHD_LABEL}.plist"

UDEV_RULES_PATH="/etc/udev/rules.d/99-hypercolor.rules"
INPUT_UDEV_RULES_PATH="/etc/udev/rules.d/70-hypercolor-input.rules"

VERSION=""
NO_SERVICE=false
UNINSTALL=false
SKIP_CONFIRM=false
RELEASE_ARCHIVE=""
RELEASE_CHECKSUM=""
RELEASE_VERIFIER=""

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
  --no-service      Preserve raw-direct service state
  --uninstall       Remove Hypercolor installation
  --yes, -y         Skip confirmation prompts
  --help, -h        Show this help message

Environment:
  HYPERCOLOR_INSTALL_PREFIX  Override the raw install prefix
  HYPERCOLOR_INSTALL_DIR     Override the raw binary directory
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
        Darwin-x86_64)  ARTIFACT_SUFFIX="macos-amd64" ;;
        Darwin-aarch64) ARTIFACT_SUFFIX="macos-arm64" ;;
        *)              fatal "Unsupported platform: ${OS} ${ARCH}" ;;
    esac

    info "Detected platform: ${OS} ${ARCH} (${ARTIFACT_SUFFIX})"
}

validate_install_topology() {
    if [[ "$OS" == Linux ]]; then
        [[ "$INSTALL_PREFIX" == "${HOME}/.local" ]] \
            || fatal "Linux raw installs require HYPERCOLOR_INSTALL_PREFIX=${HOME}/.local"
        [[ "$INSTALL_DIR" == "${INSTALL_PREFIX}/bin" ]] \
            || fatal "Linux raw installs require HYPERCOLOR_INSTALL_DIR=${INSTALL_PREFIX}/bin"
    fi
}

# ─── Prerequisite checks ─────────────────────────────────────────────────────

check_dependencies() {
    local missing=()
    for cmd in curl; do
        command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
    done
    case "$OS" in
        Linux)
            command -v python3 >/dev/null 2>&1 || missing+=("python3")
            ;;
        Darwin)
            command -v tar >/dev/null 2>&1 || missing+=("tar")
            if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
                missing+=("sha256sum or shasum")
            fi
            ;;
    esac
    if [[ ${#missing[@]} -gt 0 ]]; then
        fatal "Missing required tools: ${missing[*]}"
    fi
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print tolower($1)}'
    else
        shasum -a 256 "$1" | awk '{print tolower($1)}'
    fi
}

verify_release_artifact() {
    local file="$1"
    local checksum_file="$2"
    local expected actual

    expected="$(awk 'NF { print tolower($1); exit }' "$checksum_file")"
    [[ -n "$expected" ]] || fatal "Checksum file is empty: ${checksum_file}"
    [[ "$expected" =~ ^[a-f0-9]{64}$ ]] \
        || fatal "Invalid SHA256 checksum file: ${checksum_file}"

    actual="$(sha256_file "$file")"
    if [[ "$actual" != "$expected" ]]; then
        fatal "Checksum mismatch for $(basename "$file")"
    fi

    success "Verified SHA256 checksum"
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
    RELEASE_ARCHIVE="${TMPDIR_INSTALL}/${tarball}"
    RELEASE_CHECKSUM="${RELEASE_ARCHIVE}.sha256"

    info "Downloading ${tarball}..."
    if ! curl -fsSL --progress-bar -o "$RELEASE_ARCHIVE" "$url"; then
        fatal "Failed to download ${url}"
    fi

    info "Downloading ${tarball}.sha256..."
    if ! curl -fsSL -o "$RELEASE_CHECKSUM" "${url}.sha256"; then
        fatal "Failed to download ${url}.sha256"
    fi

    if [[ ! -s "$RELEASE_ARCHIVE" ]]; then
        fatal "Downloaded file is empty: ${tarball}"
    fi
    if [[ ! -s "$RELEASE_CHECKSUM" ]]; then
        fatal "Downloaded checksum is empty: ${tarball}.sha256"
    fi

    success "Downloaded release artifact"
}

download_release_verifier() {
    local url="https://raw.githubusercontent.com/${GITHUB_REPO}/${VERSION}/scripts/verify-release-artifact.sh"
    RELEASE_VERIFIER="${TMPDIR_INSTALL}/verify-release-artifact.sh"
    info "Downloading hardened release verifier..."
    if ! curl -fsSL -o "$RELEASE_VERIFIER" "$url"; then
        fatal "Failed to download release verifier from ${url}"
    fi
    [[ -s "$RELEASE_VERIFIER" ]] || fatal "Downloaded release verifier is empty"
}

install_release_candidate() {
    local candidate_args=(
        --install-candidate
        --archive "$RELEASE_ARCHIVE"
        --checksum "$RELEASE_CHECKSUM"
        --install-prefix "$INSTALL_PREFIX"
        --install-dir "$INSTALL_DIR"
    )
    if [[ "$NO_SERVICE" == true ]]; then
        candidate_args+=(--no-service)
    fi
    bash "$RELEASE_VERIFIER" "${candidate_args[@]}"
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

# ─── Main install flow ────────────────────────────────────────────────────────

do_install() {
    banner
    detect_platform
    validate_install_topology
    check_dependencies
    setup_tmpdir
    fetch_latest_version
    download_release_artifact

    printf "\n"
    info "Installing Hypercolor ${VERSION} into ${INSTALL_PREFIX}"
    printf "\n"

    download_release_verifier
    install_release_candidate
    check_path

    # ─── Success summary ──────────────────────────────────────────────────────

    printf "\n"
    printf "  ${GREEN}${BOLD}Hypercolor ${VERSION} installed successfully!${RESET}\n"
    printf "\n"
    printf "  ${DIM}CLI:${RESET}     ${INSTALL_DIR}/hypercolor\n"
    printf "  ${DIM}Daemon:${RESET}  ${INSTALL_DIR}/hypercolor-daemon\n"
    printf "  ${DIM}App:${RESET}     ${INSTALL_DIR}/hypercolor-app\n"
    printf "  ${DIM}Open UI:${RESET} ${INSTALL_DIR}/hypercolor-open\n"
    printf "  ${DIM}TUI:${RESET}     ${INSTALL_DIR}/hypercolor-tui\n"
    printf "  ${DIM}Web UI:${RESET}  ${CYAN}http://localhost:9420${RESET}\n"
    printf "\n"
    printf "  ${DIM}Quick start:${RESET}\n"
    printf "    hypercolor status     ${DIM}# Check daemon status${RESET}\n"
    printf "    hypercolor effects list ${DIM}# Browse available effects${RESET}\n"
    printf "    hypercolor devices list ${DIM}# List connected devices${RESET}\n"
    printf "\n"

    if [[ "$NO_SERVICE" == true ]]; then
        printf "  ${DIM}Service setup was left unchanged (--no-service).${RESET}\n"
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
                            sudo rm -f "$UDEV_RULES_PATH" "$INPUT_UDEV_RULES_PATH"
                            sudo udevadm control --reload-rules 2>/dev/null || true
                            success "Removed udev rules"
                            ;;
                        *)
                            info "Keeping udev rules"
                            ;;
                    esac
                else
                    sudo rm -f "$UDEV_RULES_PATH" "$INPUT_UDEV_RULES_PATH"
                    sudo udevadm control --reload-rules 2>/dev/null || true
                    success "Removed udev rules"
                fi
            fi
            ;;
        Darwin)
            if [[ -f "$LAUNCHD_PLIST" ]]; then
                info "Booting out launchd agent..."
                launchctl bootout "gui/$(id -u)/${LAUNCHD_LABEL}" 2>/dev/null || true
                rm -f "$LAUNCHD_PLIST"
                success "Removed launchd agent"
            fi
            ;;
    esac

    # Remove binaries
    info "Removing binaries..."
    rm -f "${INSTALL_DIR}/hypercolor"
    rm -f "${INSTALL_DIR}/hypercolor-daemon"
    rm -f "${INSTALL_DIR}/hypercolor-app"
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
