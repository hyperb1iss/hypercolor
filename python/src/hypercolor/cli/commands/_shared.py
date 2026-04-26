"""Shared CLI helpers."""

from __future__ import annotations

from typing import Any

import typer

from ...sync_client import SyncHypercolorClient
from ..app import ClientOptions


def create_client(ctx: typer.Context) -> SyncHypercolorClient:
    """Create a sync client from the Typer context options."""
    options = _require_options(ctx)
    return SyncHypercolorClient(
        host=options.host,
        port=options.port,
        api_key=options.api_key,
        timeout=options.timeout,
    )


def parse_controls(pairs: list[str]) -> dict[str, Any]:
    """Parse repeated key=value CLI control arguments."""
    controls: dict[str, Any] = {}
    for pair in pairs:
        if "=" not in pair:
            msg = f"Invalid control override: {pair}"
            raise typer.BadParameter(msg)
        key, raw_value = pair.split("=", 1)
        controls[key] = _coerce_value(raw_value)
    return controls


def _require_options(ctx: typer.Context) -> ClientOptions:
    if not isinstance(ctx.obj, ClientOptions):
        msg = "CLI context missing Hypercolor client options"
        raise TypeError(msg)
    return ctx.obj


def _coerce_value(value: str) -> Any:
    lowered = value.lower()
    if lowered in {"true", "false"}:
        return lowered == "true"
    for cast in (int, float):
        try:
            return cast(value)
        except ValueError:
            continue
    return value
