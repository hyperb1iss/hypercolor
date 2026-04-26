"""Shared constants for the Hypercolor client."""

from __future__ import annotations

from . import ws_protocol

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 9420
DEFAULT_TIMEOUT = 10.0
API_PREFIX = "/api/v1"
WS_PATH = f"{API_PREFIX}/ws"
WS_SUBPROTOCOL = ws_protocol.WS_SUBPROTOCOL

SILKCIRCUIT_PALETTE = {
    "electric_purple": "#e135ff",
    "neon_cyan": "#80ffea",
    "coral": "#ff6ac1",
    "electric_yellow": "#f1fa8c",
    "success_green": "#50fa7b",
    "error_red": "#ff6363",
}
