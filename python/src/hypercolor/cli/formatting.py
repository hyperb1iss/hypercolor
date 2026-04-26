"""Rich formatting helpers for the Hypercolor CLI."""

from __future__ import annotations

from typing import Any

from rich.console import Console
from rich.table import Table
from rich.theme import Theme

from ..constants import SILKCIRCUIT_PALETTE

console = Console(
    theme=Theme(
        {
            "accent": SILKCIRCUIT_PALETTE["electric_purple"],
            "primary": SILKCIRCUIT_PALETTE["neon_cyan"],
            "number": SILKCIRCUIT_PALETTE["coral"],
            "warning": SILKCIRCUIT_PALETTE["electric_yellow"],
            "success": SILKCIRCUIT_PALETTE["success_green"],
            "error": SILKCIRCUIT_PALETTE["error_red"],
        }
    )
)


def render_key_value(title: str, values: dict[str, Any]) -> None:
    """Render a simple key/value table."""
    table = Table(title=f"[primary bold]{title}[/]", border_style="accent")
    table.add_column("Key", style="primary")
    table.add_column("Value")
    for key, value in values.items():
        table.add_row(key, _format_value(value))
    console.print(table)


def render_list_table(title: str, columns: list[tuple[str, str]], rows: list[list[Any]]) -> None:
    """Render a list view table."""
    table = Table(title=f"[primary bold]{title}[/]", border_style="accent")
    for header, style in columns:
        table.add_column(header, style=style)
    for row in rows:
        table.add_row(*[_format_value(cell) for cell in row])
    console.print(table)


def print_success(message: str) -> None:
    """Print a success message."""
    console.print(f"[success]ok[/] {message}")


def print_error(message: str) -> None:
    """Print an error message."""
    console.print(f"[error]error[/] {message}")


def _format_value(value: Any) -> str:
    if isinstance(value, bool):
        return "yes" if value else "no"
    if value is None:
        return "-"
    if isinstance(value, list):
        return ", ".join(_format_value(item) for item in value)
    if isinstance(value, dict):
        return ", ".join(f"{key}={_format_value(item)}" for key, item in value.items())
    return str(value)
