"""Effect subcommands."""

from __future__ import annotations

from typing import Annotated

import typer

from ..formatting import print_success, render_key_value, render_list_table
from ._shared import create_client, parse_controls

app = typer.Typer(help="Inspect and apply effects")


@app.command("list")
def list_effects(ctx: typer.Context) -> None:
    """List available effects."""
    with create_client(ctx) as client:
        effects = client.get_effects()
    render_list_table(
        "Effects",
        [("Name", "primary bold"), ("Category", ""), ("Source", ""), ("Audio", "")],
        [
            [effect.name, effect.category, effect.source, effect.audio_reactive]
            for effect in effects
        ],
    )


@app.command("show")
def show_effect(ctx: typer.Context, effect_id: str) -> None:
    """Show effect metadata and controls."""
    with create_client(ctx) as client:
        effect = client.get_effect(effect_id)
    render_key_value(
        effect.name,
        {
            "id": effect.id,
            "category": effect.category,
            "author": effect.author,
            "controls": [control.label for control in effect.controls],
        },
    )


@app.command("apply")
def apply_effect(
    ctx: typer.Context,
    effect_id: str,
    control: Annotated[
        list[str] | None, typer.Option("--control", help="Control overrides as key=value")
    ] = None,
) -> None:
    """Apply an effect with optional control overrides."""
    controls = parse_controls(control or [])
    with create_client(ctx) as client:
        result = client.apply_effect(effect_id, controls=controls or None)
    print_success(f"Applied {result.effect.name}")


@app.command("active")
def active_effect(ctx: typer.Context) -> None:
    """Show the active effect."""
    with create_client(ctx) as client:
        effect = client.get_active_effect()
    if effect is None:
        render_key_value("Active Effect", {"status": "none"})
        return
    render_key_value(
        "Active Effect",
        {
            "name": effect.name,
            "id": effect.id,
            "controls": effect.control_values,
            "state": effect.state,
        },
    )


@app.command("stop")
def stop_effect(ctx: typer.Context) -> None:
    """Pause rendering to stop the active effect."""
    with create_client(ctx) as client:
        client.stop_effect()
    print_success("Paused Hypercolor rendering")
