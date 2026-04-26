"""Profile subcommands."""

from __future__ import annotations

import typer

from ..formatting import print_success, render_list_table
from ._shared import create_client

app = typer.Typer(help="Inspect and apply profiles")


@app.command("list")
def list_profiles(ctx: typer.Context) -> None:
    """List profiles."""
    with create_client(ctx) as client:
        profiles = client.get_profiles()
    render_list_table(
        "Profiles",
        [("Name", "primary bold"), ("Brightness", "number"), ("Effect", ""), ("Layout", "")],
        [
            [profile.name, profile.brightness, profile.effect_name, profile.layout_id]
            for profile in profiles
        ],
    )


@app.command("apply")
def apply_profile(ctx: typer.Context, profile_id: str) -> None:
    """Apply a profile."""
    with create_client(ctx) as client:
        result = client.apply_profile(profile_id)
    print_success(f"Applied profile {result.profile.name}")
