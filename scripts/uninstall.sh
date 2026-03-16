#!/usr/bin/env bash
# Uninstall Hypercolor from user-space (~/.local).
# System hooks (udev, modules-load) optionally removed with --system.
#
# Usage:
#   ./scripts/uninstall.sh              # remove user files only
#   ./scripts/uninstall.sh --system     # also remove udev rules + module persistence
#   ./scripts/uninstall.sh --purge      # also remove config + data directories

set -euo pipefail

REMOVE_SYSTEM=0
PURGE=0

PREFIX="${HOME}/.local"
BIN_DIR="${PREFIX}/bin"
DATA_DIR="${PREFIX}/share/hypercolor"
APP_DIR="${PREFIX}/share/applications"
ICON_DIRS=(
  "${PREFIX}/share/icons/hicolor/scalable/apps/hypercolor.svg"
  "${PREFIX}/share/icons/hicolor/48x48/apps/hypercolor.png"
  "${PREFIX}/share/icons/hicolor/128x128/apps/hypercolor.png"
  "${PREFIX}/share/icons/hicolor/256x256/apps/hypercolor.png"
)
BASH_COMPLETION="${PREFIX}/share/bash-completion/completions/hyper"
ZSH_COMPLETION="${PREFIX}/share/zsh/site-functions/_hyper"
FISH_COMPLETION="${HOME}/.config/fish/completions/hyper.fish"
SYSTEMD_UNIT="${HOME}/.config/systemd/user/hypercolor.service"
LAUNCHD_PLIST="${HOME}/Library/LaunchAgents/tech.hyperbliss.hypercolor.plist"

CONFIG_DIR="${HOME}/.config/hypercolor"
CACHE_DIR="${HOME}/.cache/hypercolor"

info()  { printf '\033[38;2;128;255;234m→\033[0m %s\n' "$*"; }
ok()    { printf '\033[38;2;80;250;123m✅\033[0m %s\n' "$*"; }
warn()  { printf '\033[38;2;241;250;140m⚠\033[0m  %s\n' "$*" >&2; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --system) REMOVE_SYSTEM=1; shift ;;
    --purge)  PURGE=1; REMOVE_SYSTEM=1; shift ;;
    -h|--help)
      cat <<'EOF'
Usage: ./scripts/uninstall.sh [options]

Options:
  --system    Also remove udev rules and modules-load config (requires sudo)
  --purge     Remove everything including config and data directories
  -h, --help  Show this help
EOF
      exit 0
      ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

# ── Stop & Disable Service ───────────────────────────────────
if command -v systemctl &>/dev/null; then
  if systemctl --user is-active hypercolor.service &>/dev/null; then
    info "Stopping hypercolor service"
    systemctl --user stop hypercolor.service
  fi
  if systemctl --user is-enabled hypercolor.service &>/dev/null; then
    info "Disabling hypercolor service"
    systemctl --user disable hypercolor.service
  fi
fi

# macOS launchd
if [[ -f "${LAUNCHD_PLIST}" ]]; then
  info "Unloading launchd service"
  launchctl unload "${LAUNCHD_PLIST}" 2>/dev/null || true
  rm -f "${LAUNCHD_PLIST}"
fi

# ── Remove Binaries ──────────────────────────────────────────
for bin in hypercolor hyper hypercolor-tray hypercolor-tui hypercolor-open; do
  if [[ -f "${BIN_DIR}/${bin}" ]]; then
    info "Removing ${BIN_DIR}/${bin}"
    rm -f "${BIN_DIR}/${bin}"
  fi
done

# ── Remove Data ──────────────────────────────────────────────
if [[ -d "${DATA_DIR}" ]]; then
  info "Removing ${DATA_DIR}"
  rm -rf "${DATA_DIR}"
fi

# ── Remove Desktop Integration ───────────────────────────────
rm -f "${APP_DIR}/hypercolor.desktop"
for icon in "${ICON_DIRS[@]}"; do
  rm -f "${icon}"
done

# Update icon cache if available
if command -v gtk-update-icon-cache &>/dev/null; then
  gtk-update-icon-cache -f -t "${PREFIX}/share/icons/hicolor" 2>/dev/null || true
fi

# ── Remove Completions ───────────────────────────────────────
rm -f "${BASH_COMPLETION}" "${ZSH_COMPLETION}" "${FISH_COMPLETION}"

# ── Remove Systemd Unit ──────────────────────────────────────
if [[ -f "${SYSTEMD_UNIT}" ]]; then
  info "Removing systemd unit"
  rm -f "${SYSTEMD_UNIT}"
  systemctl --user daemon-reload 2>/dev/null || true
fi

# ── System Hooks ─────────────────────────────────────────────
if [[ "${REMOVE_SYSTEM}" -eq 1 ]]; then
  if [[ -f /etc/udev/rules.d/99-hypercolor.rules ]]; then
    info "Removing udev rules (requires sudo)"
    sudo rm -f /etc/udev/rules.d/99-hypercolor.rules
    sudo udevadm control --reload-rules 2>/dev/null || true
  fi
  if [[ -f /etc/modules-load.d/i2c-dev.conf ]]; then
    info "Removing i2c-dev module persistence"
    sudo rm -f /etc/modules-load.d/i2c-dev.conf
  fi
fi

# ── Purge Config & Cache ────────────────────────────────────
if [[ "${PURGE}" -eq 1 ]]; then
  if [[ -d "${CONFIG_DIR}" ]]; then
    info "Removing config directory ${CONFIG_DIR}"
    rm -rf "${CONFIG_DIR}"
  fi
  if [[ -d "${CACHE_DIR}" ]]; then
    info "Removing cache directory ${CACHE_DIR}"
    rm -rf "${CACHE_DIR}"
  fi
fi

ok "Hypercolor has been uninstalled"
