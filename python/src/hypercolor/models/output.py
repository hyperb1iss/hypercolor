"""Ergonomic output resource projection."""

from __future__ import annotations

import msgspec


class OutputState(msgspec.Struct, kw_only=True):
    """Global power and brightness from the `/output` resource."""

    power: str
    brightness: float

    @property
    def paused(self) -> bool:
        """Return whether output is paused."""

        return self.power == "paused"

    @property
    def brightness_percent(self) -> int:
        """Return brightness as a 0-100 percentage."""

        return round(max(0.0, min(1.0, self.brightness)) * 100)
