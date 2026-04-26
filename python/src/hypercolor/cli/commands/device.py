"""Device subcommands."""

from __future__ import annotations

from typing import Annotated

import typer

from ..formatting import print_success, render_key_value, render_list_table
from ._shared import create_client

app = typer.Typer(help="Inspect and manage Hypercolor devices")


@app.command("list")
def list_devices(ctx: typer.Context) -> None:
    """List known devices."""
    with create_client(ctx) as client:
        devices = client.get_devices()
    render_list_table(
        "Devices",
        [("Name", "primary bold"), ("Backend", ""), ("Status", ""), ("LEDs", "number")],
        [[device.name, device.backend, device.status, device.total_leds] for device in devices],
    )


@app.command("show")
def show_device(ctx: typer.Context, device_id: str) -> None:
    """Show detailed information for one device."""
    with create_client(ctx) as client:
        device = client.get_device(device_id)
    render_key_value(
        device.name,
        {
            "id": device.id,
            "backend": device.backend,
            "status": device.status,
            "brightness": device.brightness,
            "total_leds": device.total_leds,
            "zones": [zone.name for zone in device.zones],
            "connection": device.connection_label,
            "network_ip": device.network_ip,
        },
    )


@app.command("discover")
def discover_devices(
    ctx: typer.Context,
    backend: Annotated[
        list[str] | None, typer.Option("--backend", help="Backend(s) to scan")
    ] = None,
    timeout_ms: Annotated[
        int | None, typer.Option("--timeout-ms", help="Optional scan timeout")
    ] = None,
) -> None:
    """Trigger a discovery scan."""
    with create_client(ctx) as client:
        result = client.discover_devices(backends=backend, timeout_ms=timeout_ms)
    print_success(f"Started discovery {result.scan_id} ({result.status})")


@app.command("identify")
def identify_device(
    ctx: typer.Context,
    device_id: str,
    duration_ms: Annotated[int, typer.Option("--duration-ms")] = 3000,
    color: Annotated[str, typer.Option("--color")] = "#ffffff",
) -> None:
    """Flash a device for identification."""
    with create_client(ctx) as client:
        result = client.identify_device(device_id, duration_ms=duration_ms, color=color)
    print_success(f"Identifying {result.device_id} for {result.duration_ms} ms")
