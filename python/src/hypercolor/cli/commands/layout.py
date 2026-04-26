"""Layout subcommands."""

from __future__ import annotations

import typer

from ..formatting import print_success, render_list_table
from ._shared import create_client

app = typer.Typer(help="Inspect and apply layouts")


@app.command("list")
def list_layouts(ctx: typer.Context) -> None:
    """List layouts."""
    with create_client(ctx) as client:
        layouts = client.get_layouts()
    render_list_table(
        "Layouts",
        [("Name", "primary bold"), ("Canvas", ""), ("Active", "")],
        [
            [layout.name, f"{layout.canvas_width}x{layout.canvas_height}", layout.is_active]
            for layout in layouts
        ],
    )


@app.command("apply")
def apply_layout(ctx: typer.Context, layout_id: str) -> None:
    """Apply a layout by id."""
    with create_client(ctx) as client:
        client.apply_layout(layout_id)
    print_success(f"Applied layout {layout_id}")
