#!/usr/bin/env bash
# One-command installer for Hypercolor.
# Downloads pre-built binaries from GitHub Releases and installs to ~/.local.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/get-hypercolor.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/get-hypercolor.sh | sh -s -- --version 0.2.0
#   curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/get-hypercolor.sh | sh -s -- --uninstall
#
# Environment:
#   HYPERCOLOR_VERSION   Override version to install
#   HYPERCOLOR_PREFIX    Override install prefix (default: ~/.local)
#   HYPERCOLOR_NO_MODIFY_PATH  Skip PATH modification hint

set -euo pipefail

REPO="hyperb1iss/hypercolor"
PREFIX="${HYPERCOLOR_PREFIX:-${HOME}/.local}"
VERSION="${HYPERCOLOR_VERSION:-}"
UNINSTALL=0

# ── Pretty Output ────────────────────────────────────────────
if [[ -t 1 ]]; then
  CYAN='\033[38;2;128;255;234m'
  GREEN='\033[38;2;80;250;123m'
  YELLOW='\033[38;2;241;250;140m'
  RED='\033[38;2;255;99;99m'
  PURPLE='\033[38;2;225;53;255m'
  BOLD='\033[1m'
  RESET='\033[0m'
else
  CYAN='' GREEN='' YELLOW='' RED='' PURPLE='' BOLD='' RESET=''
fi

info()  { printf "${CYAN}→${RESET} %s\n" "$*"; }
ok()    { printf "${GREEN}✅${RESET} %s\n" "$*"; }
warn()  { printf "${YELLOW}⚠${RESET}  %s\n" "$*" >&2; }
die()   { printf "${RED}✗${RESET} %s\n" "$*" >&2; exit 1; }

banner() {
  printf "\n${PURPLE}${BOLD}"
  cat <<'ART'
  ╦ ╦╦ ╦╔═╗╔═╗╦═╗╔═╗╔═╗╦  ╔═╗╦═╗
  ╠═╣╚╦╝╠═╝║╣ ╠╦╝║  ║ ║║  ║ ║╠╦╝
  ╩ ╩ ╩ ╩  ╚═╝╩╚═╚═╝╚═╝╩═╝╚═╝╩╚═
ART
  printf "${RESET}\n"
}

# ── Argument Parsing ─────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version|-v)    VERSION="$2"; shift 2 ;;
    --prefix)        PREFIX="$2"; shift 2 ;;
    --uninstall)     UNINSTALL=1; shift ;;
    -h|--help)
      cat <<EOF
Usage: curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/get-hypercolor.sh | sh

Options:
  --version <ver>   Install a specific version (default: latest)
  --prefix <path>   Install prefix (default: ~/.local)
  --uninstall       Remove Hypercolor
  -h, --help        Show this help
EOF
      exit 0
      ;;
    *) die "unknown option: $1" ;;
  esac
done

# ── Platform Detection ───────────────────────────────────────
detect_platform() {
  local os arch platform

  os="$(uname -s)"
  arch="$(uname -m)"

  case "${os}" in
    Linux)  os="linux" ;;
    Darwin) os="macos" ;;
    *)      die "Unsupported OS: ${os}. Hypercolor supports Linux and macOS." ;;
  esac

  case "${arch}" in
    x86_64|amd64)   arch="amd64" ;;
    aarch64|arm64)   arch="arm64" ;;
    *)               die "Unsupported architecture: ${arch}" ;;
  esac

  platform="${os}-${arch}"

  # Validate supported combinations
  case "${platform}" in
    linux-amd64|linux-arm64|macos-arm64) ;;
    macos-amd64)
      warn "macOS x86_64 binaries not pre-built. Consider building from source."
      die "See: https://github.com/${REPO}#building-from-source"
      ;;
    *) die "Unsupported platform: ${platform}" ;;
  esac

  echo "${platform}"
}

# ── Version Resolution ───────────────────────────────────────
resolve_version() {
  if [[ -n "${VERSION}" ]]; then
    # Strip leading 'v' if present
    VERSION="${VERSION#v}"
    echo "${VERSION}"
    return
  fi

  info "Fetching latest version..."
  local latest
  if command -v curl &>/dev/null; then
    latest=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep '"tag_name"' | head -1 | sed 's/.*"v\([^"]*\)".*/\1/')
  elif command -v wget &>/dev/null; then
    latest=$(wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep '"tag_name"' | head -1 | sed 's/.*"v\([^"]*\)".*/\1/')
  else
    die "curl or wget is required"
  fi

  [[ -n "${latest}" ]] || die "Could not determine latest version. Specify with --version."
  echo "${latest}"
}

# ── Download & Extract ───────────────────────────────────────
download_and_extract() {
  local version="$1" platform="$2"
  local tarball="hypercolor-${version}-${platform}.tar.gz"
  local url="https://github.com/${REPO}/releases/download/v${version}/${tarball}"
  local tmpdir

  tmpdir=$(mktemp -d)
  trap 'rm -rf "${tmpdir}"' EXIT

  info "Downloading ${tarball}..."
  if command -v curl &>/dev/null; then
    curl -fSL --progress-bar "${url}" -o "${tmpdir}/${tarball}" \
      || die "Download failed. Check that v${version} exists at:\n  ${url}"
  else
    wget -q --show-progress "${url}" -O "${tmpdir}/${tarball}" \
      || die "Download failed."
  fi

  info "Extracting..."
  tar xzf "${tmpdir}/${tarball}" -C "${tmpdir}"

  local extracted="${tmpdir}/hypercolor-${version}-${platform}"
  [[ -d "${extracted}" ]] || die "Unexpected archive structure"

  echo "${extracted}"
}

# ── Install ──────────────────────────────────────────────────
do_install() {
  local src="$1"
  local bin_dir="${PREFIX}/bin"
  local share_dir="${PREFIX}/share"

  mkdir -p "${bin_dir}"

  # Binaries
  info "Installing binaries to ${bin_dir}"
  for bin in hypercolor-daemon hypercolor hypercolor-app hypercolor-tray hypercolor-tui hypercolor-open; do
    if [[ -f "${src}/bin/${bin}" ]]; then
      install -m 755 "${src}/bin/${bin}" "${bin_dir}/${bin}"
    fi
  done

  # Data files (UI, effects)
  if [[ -d "${src}/share/hypercolor" ]]; then
    info "Installing data files"
    mkdir -p "${share_dir}/hypercolor"
    cp -R "${src}/share/hypercolor/." "${share_dir}/hypercolor/"
  fi

  # Desktop entry (rewrite BIN_DIR for user prefix)
  if [[ -f "${src}/share/applications/hypercolor.desktop" ]]; then
    mkdir -p "${share_dir}/applications"
    sed "s|Exec=/usr/bin/|Exec=${bin_dir}/|g" \
      "${src}/share/applications/hypercolor.desktop" \
      > "${share_dir}/applications/hypercolor.desktop"
  fi

  # Icons
  if [[ -d "${src}/share/icons" ]]; then
    info "Installing icons"
    cp -R "${src}/share/icons/." "${share_dir}/icons/"
    if command -v gtk-update-icon-cache &>/dev/null; then
      gtk-update-icon-cache -f -t "${share_dir}/icons/hicolor" 2>/dev/null || true
    fi
  fi

  # Shell completions
  if [[ -f "${src}/share/bash-completion/completions/hypercolor" ]]; then
    mkdir -p "${share_dir}/bash-completion/completions"
    cp "${src}/share/bash-completion/completions/hypercolor" \
       "${share_dir}/bash-completion/completions/"
  fi
  if [[ -f "${src}/share/zsh/site-functions/_hypercolor" ]]; then
    mkdir -p "${share_dir}/zsh/site-functions"
    cp "${src}/share/zsh/site-functions/_hypercolor" \
       "${share_dir}/zsh/site-functions/"
  fi
  if [[ -f "${src}/share/fish/vendor_completions.d/hypercolor.fish" ]]; then
    mkdir -p "${HOME}/.config/fish/completions"
    cp "${src}/share/fish/vendor_completions.d/hypercolor.fish" \
       "${HOME}/.config/fish/completions/"
  fi

  # Systemd user service (Linux)
  if [[ -f "${src}/lib/systemd/user/hypercolor.service" ]]; then
    local systemd_dir="${HOME}/.config/systemd/user"
    mkdir -p "${systemd_dir}"
    # Rewrite ExecStart to use actual binary path
    sed "s|ExecStart=.*hypercolor-daemon|ExecStart=${bin_dir}/hypercolor-daemon|g; \
         s|%h/.local/share/hypercolor|${share_dir}/hypercolor|g" \
      "${src}/lib/systemd/user/hypercolor.service" \
      > "${systemd_dir}/hypercolor.service"

    if command -v systemctl &>/dev/null; then
      systemctl --user daemon-reload 2>/dev/null || true
      info "Enabling hypercolor service"
      systemctl --user enable hypercolor.service 2>/dev/null || true
    fi
  fi

  # macOS launchd
  if [[ -f "${src}/share/hypercolor/launchd/tech.hyperbliss.hypercolor.plist" ]]; then
    local agents_dir="${HOME}/Library/LaunchAgents"
    local log_dir="${HOME}/Library/Logs/hypercolor"
    local plist
    mkdir -p "${agents_dir}"
    mkdir -p "${log_dir}"
    plist="$(<"${src}/share/hypercolor/launchd/tech.hyperbliss.hypercolor.plist")"
    plist="${plist//@BIN_DIR@/${bin_dir}}"
    plist="${plist//@UI_DIR@/${share_dir}/hypercolor/ui}"
    plist="${plist//@LOG_DIR@/${log_dir}}"
    plist="${plist//~\/.local\/bin\/hypercolor-daemon/${bin_dir}/hypercolor-daemon}"
    printf "%s\n" "$plist" > "${agents_dir}/tech.hyperbliss.hypercolor.plist"
    info "Launchd plist installed — load with: launchctl load ${agents_dir}/tech.hyperbliss.hypercolor.plist"
  fi

  # udev rules (Linux, requires sudo)
  if [[ -f "${src}/lib/udev/rules.d/99-hypercolor.rules" ]]; then
    printf "\n"
    info "Hypercolor needs udev rules for USB device access."
    printf "    Run the following to install them:\n"
    printf "    ${CYAN}sudo install -m644 ${share_dir}/hypercolor/udev/99-hypercolor.rules /etc/udev/rules.d/${RESET}\n"
    printf "    ${CYAN}sudo udevadm control --reload-rules && sudo udevadm trigger${RESET}\n"
    # Stash a copy for the user to install manually
    mkdir -p "${share_dir}/hypercolor/udev"
    cp "${src}/lib/udev/rules.d/99-hypercolor.rules" "${share_dir}/hypercolor/udev/"
    if [[ -f "${src}/etc/modules-load.d/i2c-dev.conf" ]]; then
      cp "${src}/etc/modules-load.d/i2c-dev.conf" "${share_dir}/hypercolor/udev/"
    fi
  fi
}

# ── Uninstall ────────────────────────────────────────────────
do_uninstall() {
  local bin_dir="${PREFIX}/bin"

  # Stop service first
  if command -v systemctl &>/dev/null; then
    systemctl --user stop hypercolor.service 2>/dev/null || true
    systemctl --user disable hypercolor.service 2>/dev/null || true
  fi

  info "Removing binaries"
  for bin in hypercolor-daemon hypercolor hypercolor-app hypercolor-tray hypercolor-tui hypercolor-open; do
    rm -f "${bin_dir}/${bin}"
  done

  info "Removing data files"
  rm -rf "${PREFIX}/share/hypercolor"
  rm -f "${PREFIX}/share/applications/hypercolor.desktop"
  rm -f "${PREFIX}/share/icons/hicolor/scalable/apps/hypercolor.svg"
  rm -f "${PREFIX}/share/icons/hicolor/48x48/apps/hypercolor.png"
  rm -f "${PREFIX}/share/icons/hicolor/128x128/apps/hypercolor.png"
  rm -f "${PREFIX}/share/icons/hicolor/256x256/apps/hypercolor.png"
  rm -f "${PREFIX}/share/bash-completion/completions/hypercolor"
  rm -f "${PREFIX}/share/zsh/site-functions/_hypercolor"
  rm -f "${HOME}/.config/fish/completions/hypercolor.fish"
  rm -f "${PREFIX}/share/bash-completion/completions/hyper"
  rm -f "${PREFIX}/share/zsh/site-functions/_hyper"
  rm -f "${HOME}/.config/fish/completions/hyper.fish"
  rm -f "${HOME}/.config/systemd/user/hypercolor.service"
  rm -f "${HOME}/Library/LaunchAgents/tech.hyperbliss.hypercolor.plist"

  if command -v systemctl &>/dev/null; then
    systemctl --user daemon-reload 2>/dev/null || true
  fi

  ok "Hypercolor has been uninstalled"
  warn "Config (~/.config/hypercolor) and udev rules (/etc/udev/rules.d/99-hypercolor.rules) were preserved."
  warn "Remove manually if desired."
}

# ── Main ─────────────────────────────────────────────────────
banner

if [[ "${UNINSTALL}" -eq 1 ]]; then
  do_uninstall
  exit 0
fi

PLATFORM=$(detect_platform)
VERSION=$(resolve_version)

info "Installing Hypercolor v${VERSION} for ${PLATFORM}"

EXTRACTED=$(download_and_extract "${VERSION}" "${PLATFORM}")
do_install "${EXTRACTED}"

printf "\n"
ok "Hypercolor v${VERSION} installed successfully!"
printf "\n"

# PATH check
if [[ ":${PATH}:" != *":${PREFIX}/bin:"* ]]; then
  if [[ -z "${HYPERCOLOR_NO_MODIFY_PATH:-}" ]]; then
    warn "${PREFIX}/bin is not in your PATH"
    printf "    Add it to your shell profile:\n"
    printf "    ${CYAN}export PATH=\"${PREFIX}/bin:\$PATH\"${RESET}\n\n"
  fi
fi

printf "  ${BOLD}Quick start:${RESET}\n"
printf "    ${CYAN}hypercolor-daemon${RESET}  Start the daemon\n"
printf "    ${CYAN}hypercolor status${RESET}  Check daemon status\n"
printf "    ${CYAN}hypercolor-tui${RESET}     Launch the terminal UI\n"
printf "    ${CYAN}hypercolor-open${RESET}    Open the web UI in your browser\n"
printf "\n"
printf "  ${BOLD}Manage the service:${RESET}\n"
if [[ "$(uname -s)" == "Linux" ]]; then
  printf "    ${CYAN}systemctl --user start hypercolor${RESET}\n"
  printf "    ${CYAN}systemctl --user status hypercolor${RESET}\n"
fi
printf "\n"
printf "  ${BOLD}Uninstall:${RESET}\n"
printf "    ${CYAN}curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/get-hypercolor.sh | sh -s -- --uninstall${RESET}\n"
printf "\n"
