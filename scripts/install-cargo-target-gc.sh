#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
LIBEXEC_DIR="$HOME/.local/share/hypercolor/libexec"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
ENABLE=1
UNINSTALL=0

for arg in "$@"; do
  case "$arg" in
    --no-enable) ENABLE=0 ;;
    --uninstall) UNINSTALL=1 ;;
    -h | --help)
      echo "Usage: install-cargo-target-gc.sh [--no-enable|--uninstall]"
      exit 0
      ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

if [ "$UNINSTALL" -eq 1 ]; then
  systemctl --user disable --now hypercolor-cargo-target-gc.timer 2>/dev/null || true
  rm -f \
    "$UNIT_DIR/hypercolor-cargo-target-gc.service" \
    "$UNIT_DIR/hypercolor-cargo-target-gc.timer" \
    "$LIBEXEC_DIR/cargo-target-gc"
  systemctl --user daemon-reload
  echo "[cargo-gc] user timer removed"
  exit 0
fi

if ! command -v cargo-sweep >/dev/null 2>&1; then
  cargo install --locked --version 0.8.0 cargo-sweep
fi

install -Dm755 "$ROOT_DIR/scripts/cargo-target-gc.sh" \
  "$LIBEXEC_DIR/cargo-target-gc"
install -Dm644 "$ROOT_DIR/packaging/systemd/user/hypercolor-cargo-target-gc.service" \
  "$UNIT_DIR/hypercolor-cargo-target-gc.service"
install -Dm644 "$ROOT_DIR/packaging/systemd/user/hypercolor-cargo-target-gc.timer" \
  "$UNIT_DIR/hypercolor-cargo-target-gc.timer"
systemctl --user daemon-reload
if [ "$ENABLE" -eq 1 ]; then
  systemctl --user enable --now hypercolor-cargo-target-gc.timer
fi
echo "[cargo-gc] installed pressure-triggered target collection"
