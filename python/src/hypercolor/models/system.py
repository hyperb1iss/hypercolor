"""System state models."""

from __future__ import annotations

import msgspec


class ServerIdentity(msgspec.Struct, kw_only=True):
    """Stable daemon identity."""

    instance_id: str
    instance_name: str
    version: str


class RenderLoopStatus(msgspec.Struct, kw_only=True):
    """Current render loop status."""

    state: str
    fps_tier: str
    total_frames: int


class SystemState(msgspec.Struct, kw_only=True):
    """Current daemon status snapshot."""

    running: bool
    version: str
    server: ServerIdentity
    config_path: str
    data_dir: str
    cache_dir: str
    uptime_seconds: int
    device_count: int
    effect_count: int
    scene_count: int
    global_brightness: int
    audio_available: bool
    capture_available: bool
    render_loop: RenderLoopStatus
    event_bus_subscribers: int
    active_effect: str | None = None

    @property
    def brightness(self) -> int:
        """Backward-compatible alias for the global brightness."""

        return self.global_brightness

    @property
    def paused(self) -> bool:
        """Return whether the render loop is currently paused."""

        return self.render_loop.state == "paused"


class HealthStatus(msgspec.Struct, kw_only=True):
    """Flat health check response."""

    status: str
    version: str
    uptime_seconds: int
    checks: dict[str, str]
