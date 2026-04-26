"""System subcommands."""

from __future__ import annotations

import typer

from ..formatting import print_success, render_key_value
from ._shared import create_client

app = typer.Typer(help="Inspect daemon state")


@app.command("status")
def status(ctx: typer.Context) -> None:
    """Show a high-level daemon state summary."""
    with create_client(ctx) as client:
        state = client.get_status()
    render_key_value(
        "Hypercolor Status",
        {
            "running": state.running,
            "paused": state.paused,
            "brightness": state.global_brightness,
            "render_state": state.render_loop.state,
            "fps_tier": state.render_loop.fps_tier,
            "effect": state.active_effect,
            "devices": state.device_count,
            "effects": state.effect_count,
            "scenes": state.scene_count,
            "version": state.version,
        },
    )


@app.command("health")
def health(ctx: typer.Context) -> None:
    """Run the daemon health check."""
    with create_client(ctx) as client:
        result = client.health()
    render_key_value(
        "Health", {"status": result.status, "version": result.version, **result.checks}
    )


@app.command("pause")
def pause(ctx: typer.Context) -> None:
    """Stop the currently active effect."""
    with create_client(ctx) as client:
        client.stop_effect()
    print_success("Stopped the active Hypercolor effect")


@app.command("resume")
def resume(ctx: typer.Context) -> None:
    """Explain that Hypercolor requires reapplying an effect or profile."""
    with create_client(ctx) as client:
        client.resume_rendering()
