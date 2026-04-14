#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="release"
SKIP_BUILD=0
SKIP_SYSTEM_HOOKS=0
ENABLE_SERVICE=1
START_SERVICE=1

PREFIX="${HOME}/.local"
BIN_DIR="${PREFIX}/bin"
DATA_DIR="${PREFIX}/share/hypercolor"
UI_DIR="${DATA_DIR}/ui"
EFFECTS_DIR="${DATA_DIR}/effects/bundled"
APP_DIR="${PREFIX}/share/applications"
BASH_COMPLETION_DIR="${PREFIX}/share/bash-completion/completions"
ZSH_COMPLETION_DIR="${PREFIX}/share/zsh/site-functions"
FISH_COMPLETION_DIR="${HOME}/.config/fish/completions"
SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"

usage() {
  cat <<'EOF'
Usage: ./scripts/install.sh [options]

Options:
  --profile <release|preview>  Build profile to install (default: release)
  --skip-build                 Reuse existing build artifacts
  --skip-system-hooks          Skip sudo-managed udev/modules-load setup
  --no-enable-service          Install the user unit but do not enable it
  --no-start-service           Enable the user unit but do not start it now
  -h, --help                   Show this help
EOF
}

info() {
  printf '[install] %s\n' "$*"
}

warn() {
  printf '[install] warning: %s\n' "$*" >&2
}

die() {
  printf '[install] error: %s\n' "$*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

require_file() {
  [[ -e "$1" ]] || die "missing required file: $1"
}

profile_dir() {
  if [[ "$PROFILE" == "preview" ]]; then
    printf 'preview'
    return
  fi
  printf 'release'
}

render_desktop_entry() {
  local target="$1"
  sed "s|@BIN_DIR@|${BIN_DIR}|g" \
    "${ROOT_DIR}/packaging/desktop/hypercolor.desktop.in" > "${target}"
}

install_completions() {
  install -d "${BASH_COMPLETION_DIR}" "${ZSH_COMPLETION_DIR}" "${FISH_COMPLETION_DIR}"
  "${BIN_DIR}/hyper" completions bash > "${BASH_COMPLETION_DIR}/hyper"
  "${BIN_DIR}/hyper" completions zsh > "${ZSH_COMPLETION_DIR}/_hyper"
  "${BIN_DIR}/hyper" completions fish > "${FISH_COMPLETION_DIR}/hyper.fish"
}

build_ui() {
  require_cmd npm
  require_cmd trunk

  if [[ ! -d "${ROOT_DIR}/crates/hypercolor-ui/node_modules" ]]; then
    info "installing UI npm dependencies"
    (
      cd "${ROOT_DIR}/crates/hypercolor-ui"
      npm install
    )
  fi

  if command -v rustup >/dev/null 2>&1; then
    info "ensuring wasm target is installed"
    rustup target add wasm32-unknown-unknown >/dev/null
  else
    warn "rustup not found; assuming wasm32-unknown-unknown target already exists"
  fi

  info "building web UI"
  (
    cd "${ROOT_DIR}/crates/hypercolor-ui"
    env -u NO_COLOR trunk build --release
  )
}

build_effects() {
  if [[ ! -d "${ROOT_DIR}/sdk/node_modules" ]]; then
    info "installing SDK dependencies"
    (cd "${ROOT_DIR}/sdk" && bun install)
  fi

  info "building SDK effects"
  (cd "${ROOT_DIR}/sdk" && bun run build:effects)
}

build_binaries() {
  local cargo_profile_flag=()

  require_cmd cargo
  if [[ "$PROFILE" == "preview" ]]; then
    cargo_profile_flag=(--profile preview)
  elif [[ "$PROFILE" != "release" ]]; then
    die "unsupported profile: ${PROFILE}"
  else
    cargo_profile_flag=(--release)
  fi

  info "building hypercolor daemon (with Servo)"
  cargo build -p hypercolor-daemon --bin hypercolor --features servo "${cargo_profile_flag[@]}"

  info "building hyper CLI"
  cargo build -p hypercolor-cli --bin hyper "${cargo_profile_flag[@]}"

  info "building hypercolor-tui"
  cargo build -p hypercolor-tui "${cargo_profile_flag[@]}"

  info "building hypercolor-tray"
  cargo build -p hypercolor-tray "${cargo_profile_flag[@]}"

  build_ui
  build_effects
}

install_icons() {
  local svg_src="${ROOT_DIR}/packaging/icons/hypercolor.svg"
  local icon_base="${PREFIX}/share/icons/hicolor"

  if [[ ! -f "${svg_src}" ]]; then
    warn "no icon SVG found at ${svg_src}"
    return
  fi

  install -d "${icon_base}/scalable/apps"
  install -Dm644 "${svg_src}" "${icon_base}/scalable/apps/hypercolor.svg"

  if command -v rsvg-convert &>/dev/null; then
    for size in 48 128 256; do
      install -d "${icon_base}/${size}x${size}/apps"
      rsvg-convert -w "${size}" -h "${size}" "${svg_src}" \
        -o "${icon_base}/${size}x${size}/apps/hypercolor.png"
    done
  fi

  if command -v gtk-update-icon-cache &>/dev/null; then
    gtk-update-icon-cache -f -t "${icon_base}" 2>/dev/null || true
  fi
}

install_user_files() {
  local target_dir
  target_dir="$(profile_dir)"

  require_file "${ROOT_DIR}/target/${target_dir}/hypercolor"
  require_file "${ROOT_DIR}/target/${target_dir}/hyper"
  require_file "${ROOT_DIR}/crates/hypercolor-ui/dist/index.html"

  install -d "${BIN_DIR}" "${DATA_DIR}" "${APP_DIR}" "${SYSTEMD_USER_DIR}"

  install -Dm755 \
    "${ROOT_DIR}/target/${target_dir}/hypercolor" \
    "${BIN_DIR}/hypercolor"
  install -Dm755 \
    "${ROOT_DIR}/target/${target_dir}/hyper" \
    "${BIN_DIR}/hyper"
  install -Dm755 \
    "${ROOT_DIR}/target/${target_dir}/hypercolor-tui" \
    "${BIN_DIR}/hypercolor-tui"
  install -Dm755 \
    "${ROOT_DIR}/target/${target_dir}/hypercolor-tray" \
    "${BIN_DIR}/hypercolor-tray"
  install -Dm755 \
    "${ROOT_DIR}/packaging/bin/hypercolor-open" \
    "${BIN_DIR}/hypercolor-open"
  install -Dm644 \
    "${ROOT_DIR}/packaging/systemd/user/hypercolor.service" \
    "${SYSTEMD_USER_DIR}/hypercolor.service"

  rm -rf "${UI_DIR}"
  install -d "${UI_DIR}"
  cp -R "${ROOT_DIR}/crates/hypercolor-ui/dist/." "${UI_DIR}/"

  rm -rf "${EFFECTS_DIR}"
  install -d "${EFFECTS_DIR}"
  if [[ -d "${ROOT_DIR}/effects/hypercolor" ]]; then
    cp -R "${ROOT_DIR}/effects/hypercolor/." "${EFFECTS_DIR}/"
    info "installed bundled effects into ${EFFECTS_DIR}"
  else
    warn "no built effects found at effects/hypercolor/; run 'just effects-build' first"
  fi

  render_desktop_entry "${APP_DIR}/hypercolor.desktop"
  install_icons
  install_completions
}

install_system_hooks() {
  require_cmd sudo
  require_cmd modprobe
  require_cmd udevadm

  info "installing udev rules"
  sudo install -Dm644 \
    "${ROOT_DIR}/udev/99-hypercolor.rules" \
    /etc/udev/rules.d/99-hypercolor.rules

  info "persisting i2c-dev kernel module"
  sudo install -Dm644 \
    "${ROOT_DIR}/packaging/modules-load/i2c-dev.conf" \
    /etc/modules-load.d/i2c-dev.conf

  info "loading i2c-dev now"
  if ! sudo modprobe i2c-dev; then
    warn "modprobe i2c-dev failed; SMBus RGB may stay unavailable until the module exists on this kernel"
  fi

  info "reloading udev rules"
  sudo udevadm control --reload-rules
  sudo udevadm trigger --action=add --subsystem-match=hidraw
  sudo udevadm trigger --action=add --subsystem-match=usb
  sudo udevadm trigger --action=add --subsystem-match=tty
  sudo udevadm trigger --action=add --subsystem-match=i2c-dev || true
}

configure_service() {
  if ! command -v systemctl >/dev/null 2>&1; then
    warn "systemctl not found; installed unit file but did not reload or enable it"
    return
  fi

  if ! systemctl --user daemon-reload >/dev/null 2>&1; then
    warn "systemd user manager unavailable; unit was installed to ${SYSTEMD_USER_DIR}"
    return
  fi

  if [[ "${ENABLE_SERVICE}" -eq 1 ]]; then
    info "enabling hypercolor user service"
    systemctl --user enable hypercolor.service >/dev/null
  fi

  if [[ "${START_SERVICE}" -eq 1 ]]; then
    info "starting hypercolor user service"
    systemctl --user restart hypercolor.service
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      [[ $# -ge 2 ]] || die "--profile requires a value"
      PROFILE="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --skip-system-hooks)
      SKIP_SYSTEM_HOOKS=1
      shift
      ;;
    --no-enable-service)
      ENABLE_SERVICE=0
      START_SERVICE=0
      shift
      ;;
    --no-start-service)
      START_SERVICE=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

if [[ "${SKIP_BUILD}" -eq 0 ]]; then
  build_binaries
fi

install_user_files

if [[ "${SKIP_SYSTEM_HOOKS}" -eq 0 ]]; then
  install_system_hooks
else
  warn "skipping udev/modules-load installation"
fi

configure_service

info "installed user binaries into ${BIN_DIR}"
info "installed web UI into ${UI_DIR}"
info "installed bundled effects into ${EFFECTS_DIR}"
info "desktop launcher: ${APP_DIR}/hypercolor.desktop"
info "systemd user unit: ${SYSTEMD_USER_DIR}/hypercolor.service"

if [[ "${SKIP_SYSTEM_HOOKS}" -eq 0 ]]; then
  info "udev rules and i2c-dev persistence are installed"
else
  warn "udev rules and i2c-dev persistence were not installed"
fi
