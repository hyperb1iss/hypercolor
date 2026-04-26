"""Scene subcommands."""

from __future__ import annotations

import typer

from ..formatting import print_success, render_list_table
from ._shared import create_client

app = typer.Typer(help="Inspect and trigger scenes")


@app.command("list")
def list_scenes(ctx: typer.Context) -> None:
    """List scenes."""
    with create_client(ctx) as client:
        scenes = client.get_scenes()
    render_list_table(
        "Scenes",
        [("Name", "primary bold"), ("Priority", "number"), ("Enabled", ""), ("Description", "")],
        [[scene.name, scene.priority, scene.enabled, scene.description] for scene in scenes],
    )


@app.command("activate")
def activate_scene(ctx: typer.Context, scene_id: str) -> None:
    """Trigger a scene manually."""
    with create_client(ctx) as client:
        result = client.activate_scene(scene_id)
    print_success(f"Activated scene {result.scene.name}")
