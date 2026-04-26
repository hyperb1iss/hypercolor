from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.effect_health_status import EffectHealthStatus
    from ..models.latest_frame_status import LatestFrameStatus
    from ..models.preview_runtime_status import PreviewRuntimeStatus
    from ..models.render_acceleration_status import RenderAccelerationStatus
    from ..models.render_loop_status import RenderLoopStatus
    from ..models.server_identity import ServerIdentity


T = TypeVar("T", bound="ApiResponseSystemStatusData")


@_attrs_define
class ApiResponseSystemStatusData:
    """
    Attributes:
        active_scene_snapshot_locked (bool):
        audio_available (bool):
        cache_dir (str):
        capture_available (bool):
        compositor_acceleration (RenderAccelerationStatus):
        config_path (str):
        data_dir (str):
        device_count (int):
        effect_count (int):
        effect_health (EffectHealthStatus):
        event_bus_subscribers (int):
        global_brightness (int):
        preview_runtime (PreviewRuntimeStatus):
        render_loop (RenderLoopStatus):
        running (bool):
        scene_count (int):
        server (ServerIdentity): Stable identity exposed by each Hypercolor daemon instance.
        uptime_seconds (int):
        version (str):
        active_effect (None | str | Unset):
        active_scene (None | str | Unset):
        latest_frame (LatestFrameStatus | None | Unset):
    """

    active_scene_snapshot_locked: bool
    audio_available: bool
    cache_dir: str
    capture_available: bool
    compositor_acceleration: RenderAccelerationStatus
    config_path: str
    data_dir: str
    device_count: int
    effect_count: int
    effect_health: EffectHealthStatus
    event_bus_subscribers: int
    global_brightness: int
    preview_runtime: PreviewRuntimeStatus
    render_loop: RenderLoopStatus
    running: bool
    scene_count: int
    server: ServerIdentity
    uptime_seconds: int
    version: str
    active_effect: None | str | Unset = UNSET
    active_scene: None | str | Unset = UNSET
    latest_frame: LatestFrameStatus | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.latest_frame_status import LatestFrameStatus

        active_scene_snapshot_locked = self.active_scene_snapshot_locked

        audio_available = self.audio_available

        cache_dir = self.cache_dir

        capture_available = self.capture_available

        compositor_acceleration = self.compositor_acceleration.to_dict()

        config_path = self.config_path

        data_dir = self.data_dir

        device_count = self.device_count

        effect_count = self.effect_count

        effect_health = self.effect_health.to_dict()

        event_bus_subscribers = self.event_bus_subscribers

        global_brightness = self.global_brightness

        preview_runtime = self.preview_runtime.to_dict()

        render_loop = self.render_loop.to_dict()

        running = self.running

        scene_count = self.scene_count

        server = self.server.to_dict()

        uptime_seconds = self.uptime_seconds

        version = self.version

        active_effect: None | str | Unset
        if isinstance(self.active_effect, Unset):
            active_effect = UNSET
        else:
            active_effect = self.active_effect

        active_scene: None | str | Unset
        if isinstance(self.active_scene, Unset):
            active_scene = UNSET
        else:
            active_scene = self.active_scene

        latest_frame: dict[str, Any] | None | Unset
        if isinstance(self.latest_frame, Unset):
            latest_frame = UNSET
        elif isinstance(self.latest_frame, LatestFrameStatus):
            latest_frame = self.latest_frame.to_dict()
        else:
            latest_frame = self.latest_frame

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "active_scene_snapshot_locked": active_scene_snapshot_locked,
                "audio_available": audio_available,
                "cache_dir": cache_dir,
                "capture_available": capture_available,
                "compositor_acceleration": compositor_acceleration,
                "config_path": config_path,
                "data_dir": data_dir,
                "device_count": device_count,
                "effect_count": effect_count,
                "effect_health": effect_health,
                "event_bus_subscribers": event_bus_subscribers,
                "global_brightness": global_brightness,
                "preview_runtime": preview_runtime,
                "render_loop": render_loop,
                "running": running,
                "scene_count": scene_count,
                "server": server,
                "uptime_seconds": uptime_seconds,
                "version": version,
            }
        )
        if active_effect is not UNSET:
            field_dict["active_effect"] = active_effect
        if active_scene is not UNSET:
            field_dict["active_scene"] = active_scene
        if latest_frame is not UNSET:
            field_dict["latest_frame"] = latest_frame

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.effect_health_status import EffectHealthStatus
        from ..models.latest_frame_status import LatestFrameStatus
        from ..models.preview_runtime_status import PreviewRuntimeStatus
        from ..models.render_acceleration_status import RenderAccelerationStatus
        from ..models.render_loop_status import RenderLoopStatus
        from ..models.server_identity import ServerIdentity

        d = dict(src_dict)
        active_scene_snapshot_locked = d.pop("active_scene_snapshot_locked")

        audio_available = d.pop("audio_available")

        cache_dir = d.pop("cache_dir")

        capture_available = d.pop("capture_available")

        compositor_acceleration = RenderAccelerationStatus.from_dict(
            d.pop("compositor_acceleration")
        )

        config_path = d.pop("config_path")

        data_dir = d.pop("data_dir")

        device_count = d.pop("device_count")

        effect_count = d.pop("effect_count")

        effect_health = EffectHealthStatus.from_dict(d.pop("effect_health"))

        event_bus_subscribers = d.pop("event_bus_subscribers")

        global_brightness = d.pop("global_brightness")

        preview_runtime = PreviewRuntimeStatus.from_dict(d.pop("preview_runtime"))

        render_loop = RenderLoopStatus.from_dict(d.pop("render_loop"))

        running = d.pop("running")

        scene_count = d.pop("scene_count")

        server = ServerIdentity.from_dict(d.pop("server"))

        uptime_seconds = d.pop("uptime_seconds")

        version = d.pop("version")

        def _parse_active_effect(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        active_effect = _parse_active_effect(d.pop("active_effect", UNSET))

        def _parse_active_scene(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        active_scene = _parse_active_scene(d.pop("active_scene", UNSET))

        def _parse_latest_frame(data: object) -> LatestFrameStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                latest_frame_type_1 = LatestFrameStatus.from_dict(data)

                return latest_frame_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(LatestFrameStatus | None | Unset, data)

        latest_frame = _parse_latest_frame(d.pop("latest_frame", UNSET))

        api_response_system_status_data = cls(
            active_scene_snapshot_locked=active_scene_snapshot_locked,
            audio_available=audio_available,
            cache_dir=cache_dir,
            capture_available=capture_available,
            compositor_acceleration=compositor_acceleration,
            config_path=config_path,
            data_dir=data_dir,
            device_count=device_count,
            effect_count=effect_count,
            effect_health=effect_health,
            event_bus_subscribers=event_bus_subscribers,
            global_brightness=global_brightness,
            preview_runtime=preview_runtime,
            render_loop=render_loop,
            running=running,
            scene_count=scene_count,
            server=server,
            uptime_seconds=uptime_seconds,
            version=version,
            active_effect=active_effect,
            active_scene=active_scene,
            latest_frame=latest_frame,
        )

        api_response_system_status_data.additional_properties = d
        return api_response_system_status_data

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
