"""Typer entrypoint for the Hypercolor CLI."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Annotated

import typer

from ..exceptions import HypercolorError
from .formatting import print_error


@dataclass(slots=True)
class ClientOptions:
    """Connection settings shared by all CLI commands."""

    host: str
    port: int
    api_key: str | None
    timeout: float


app = typer.Typer(help="Hypercolor daemon CLI", no_args_is_help=True)


@app.callback()
def main(
    ctx: typer.Context,
    host: Annotated[str, typer.Option("--host", help="Hypercolor daemon host")] = "127.0.0.1",
    port: Annotated[int, typer.Option("--port", help="Hypercolor daemon port")] = 9420,
    api_key: Annotated[str | None, typer.Option("--api-key", help="Optional bearer token")] = None,
    timeout: Annotated[float, typer.Option("--timeout", help="HTTP timeout in seconds")] = 10.0,
) -> None:
    """Store shared connection options in the Typer context."""
    ctx.obj = ClientOptions(host=host, port=port, api_key=api_key, timeout=timeout)


def run() -> None:
    """Run the CLI with basic error handling."""
    try:
        app()
    except HypercolorError as exc:
        print_error(str(exc))
        raise typer.Exit(code=1) from exc


from .commands import device, effect, layout, profile, scene, system  # noqa: E402

app.add_typer(device.app, name="device")
app.add_typer(effect.app, name="effect")
app.add_typer(layout.app, name="layout")
app.add_typer(scene.app, name="scene")
app.add_typer(profile.app, name="profile")
app.add_typer(system.app, name="system")
