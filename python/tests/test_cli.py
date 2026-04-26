"""CLI smoke tests."""

from __future__ import annotations

from dataclasses import dataclass

from typer.testing import CliRunner

from hypercolor.cli.app import app
from hypercolor.models.system import RenderLoopStatus, ServerIdentity, SystemState

runner = CliRunner()


@dataclass
class DummyClient:
    """Minimal context-manager client for CLI tests."""

    def __enter__(self) -> DummyClient:
        return self

    def __exit__(self, *_exc_info: object) -> None:
        return None

    def get_status(self) -> SystemState:
        return SystemState(
            running=True,
            version="0.1.0",
            server=ServerIdentity(
                instance_id="srv_1",
                instance_name="Hyperia",
                version="0.1.0",
            ),
            config_path="/var/lib/hypercolor/hypercolor.toml",
            data_dir="/var/lib/hypercolor/data",
            cache_dir="/var/cache/hypercolor",
            uptime_seconds=42,
            device_count=2,
            effect_count=12,
            scene_count=3,
            active_effect="Aurora",
            global_brightness=85,
            audio_available=True,
            capture_available=False,
            render_loop=RenderLoopStatus(
                state="running",
                fps_tier="high",
                total_frames=1_024,
            ),
            event_bus_subscribers=4,
        )


def test_system_status_command(monkeypatch) -> None:
    monkeypatch.setattr("hypercolor.cli.commands.system.create_client", lambda _ctx: DummyClient())

    result = runner.invoke(app, ["system", "status"])

    assert result.exit_code == 0
    assert "Hypercolor Status" in result.stdout
    assert "Aurora" in result.stdout
