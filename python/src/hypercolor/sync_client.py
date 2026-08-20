"""Synchronous wrapper around :class:`HypercolorClient`."""

from __future__ import annotations

import asyncio
from collections.abc import Mapping
from typing import Any, Self

import httpx

from ._generated.models.activate_scene_response import ActivateSceneResponse
from ._generated.models.apply_effect_response import ApplyEffectResponse
from ._generated.models.delete_scene_response import DeleteSceneResponse
from ._generated.models.effect_detail_response import EffectDetailResponse
from ._generated.models.effect_preset_summary import EffectPresetSummary
from ._generated.models.effect_summary import EffectSummary
from ._generated.models.replace_scene_request import ReplaceSceneRequest
from ._generated.models.scene_document import SceneDocument
from ._generated.models.scene_summary import SceneSummary
from ._generated.models.zone_resource import ZoneResource
from .client import _UNSET_SENTINEL, HypercolorClient, _Unset
from .models.audio import AudioDevices, SpectrumSnapshot
from .models.common import (
    ConfigMutationResult,
    DiscoverResult,
    IdentifyResult,
    MutationResult,
)
from .models.control import ControlActionResult, ControlApplyResult, ControlSurface
from .models.device import Device
from .models.driver import Driver
from .models.effect import EffectCoverImage
from .models.layout import Layout, LayoutSummary
from .models.system import HealthStatus, OutputState, SystemState


class SyncHypercolorClient:
    """Sync adapter around :class:`HypercolorClient` for scripts."""

    def __init__(
        self, *args: Any, transport: httpx.AsyncBaseTransport | None = None, **kwargs: Any
    ) -> None:
        self._loop: asyncio.AbstractEventLoop | None = asyncio.new_event_loop()
        self._client = HypercolorClient(*args, transport=transport, **kwargs)
        self._closed = False

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_exc_info: object) -> None:
        self.close()

    def __del__(self) -> None:
        if getattr(self, "_closed", True) or getattr(self, "_loop", None) is None:
            return
        try:
            self.close()
        except (AttributeError, RuntimeError):
            return

    def close(self) -> None:
        """Close the underlying async client and runner."""
        if self._closed or self._loop is None:
            return
        self._loop.run_until_complete(self._client.aclose())
        self._loop.close()
        self._loop = None
        self._closed = True

    def _run(self, awaitable: Any) -> Any:
        if self._loop is None or self._closed:
            msg = "SyncHypercolorClient is closed"
            raise RuntimeError(msg)
        return self._loop.run_until_complete(awaitable)

    def health(self) -> HealthStatus:
        return self._run(self._client.health())

    def get_status(self) -> SystemState:
        return self._run(self._client.get_status())

    def get_output(self) -> OutputState:
        return self._run(self._client.get_output())

    def set_output(
        self,
        *,
        power: str | None = None,
        brightness: float | None = None,
    ) -> OutputState:
        return self._run(self._client.set_output(power=power, brightness=brightness))

    def set_brightness(self, brightness: float) -> OutputState:
        return self._run(self._client.set_brightness(brightness))

    def set_output_power(self, *, paused: bool) -> OutputState:
        return self._run(self._client.set_output_power(paused=paused))

    def pause_rendering(self) -> OutputState:
        return self._run(self._client.pause_rendering())

    def resume_rendering(self) -> OutputState:
        return self._run(self._client.resume_rendering())

    def get_devices(
        self,
        *,
        offset: int | None = None,
        limit: int | None = None,
        status: str | None = None,
        backend_id: str | None = None,
        driver: str | None = None,
        q: str | None = None,
        include: str | None = None,
    ) -> list[Device]:
        return self._run(
            self._client.get_devices(
                offset=offset,
                limit=limit,
                status=status,
                backend_id=backend_id,
                driver=driver,
                q=q,
                include=include,
            )
        )

    def get_device(self, device_id: str) -> Device:
        return self._run(self._client.get_device(device_id))

    def update_device(self, device_id: str, **fields: Any) -> Device:
        return self._run(self._client.update_device(device_id, **fields))

    def discover_devices(
        self,
        backends: list[str] | None = None,
        timeout_ms: int | None = None,
    ) -> DiscoverResult:
        return self._run(self._client.discover_devices(backends=backends, timeout_ms=timeout_ms))

    def identify_device(
        self,
        device_id: str,
        *,
        duration_ms: int | None = None,
        color: str | None = None,
    ) -> IdentifyResult:
        return self._run(
            self._client.identify_device(device_id, duration_ms=duration_ms, color=color)
        )

    def get_drivers(self) -> list[Driver]:
        return self._run(self._client.get_drivers())

    def get_effects(
        self,
        *,
        category: str | None = None,
        audio_reactive: bool | None = None,
        screen_reactive: bool | None = None,
        input_reactive: bool | None = None,
        source: str | None = None,
        q: str | None = None,
        include: str | None = None,
    ) -> list[EffectSummary]:
        return self._run(
            self._client.get_effects(
                category=category,
                audio_reactive=audio_reactive,
                screen_reactive=screen_reactive,
                input_reactive=input_reactive,
                source=source,
                q=q,
                include=include,
            )
        )

    def get_effect(self, effect_id: str) -> EffectDetailResponse:
        return self._run(self._client.get_effect(effect_id))

    def get_effect_presets(self, effect_id: str) -> list[EffectPresetSummary]:
        return self._run(self._client.get_effect_presets(effect_id))

    def effect_cover_image_url(self, effect_id: str) -> str:
        return self._client.effect_cover_image_url(effect_id)

    def get_effect_cover_image(self, effect_id: str) -> EffectCoverImage:
        return self._run(self._client.get_effect_cover_image(effect_id))

    def apply_effect(
        self,
        effect_id: str,
        *,
        controls: Mapping[str, Any] | None = None,
        transition: str | Mapping[str, Any] | None = None,
        preset_id: str | None = None,
        zone: str | None = None,
        if_match: int | None = None,
    ) -> ApplyEffectResponse:
        return self._run(
            self._client.apply_effect(
                effect_id,
                controls=controls,
                transition=transition,
                preset_id=preset_id,
                zone=zone,
                if_match=if_match,
            )
        )

    def apply_effect_preset(
        self,
        effect_id: str,
        preset_id: str,
        *,
        controls: Mapping[str, Any] | None = None,
        transition: str | Mapping[str, Any] | None = None,
        zone: str | None = None,
        if_match: int | None = None,
    ) -> ApplyEffectResponse:
        return self._run(
            self._client.apply_effect_preset(
                effect_id,
                preset_id,
                controls=controls,
                transition=transition,
                zone=zone,
                if_match=if_match,
            )
        )

    def patch_layer_controls(
        self,
        zone: str,
        layer: str,
        values: Mapping[str, Any],
        *,
        clear_bindings: list[str] | None = None,
    ) -> ZoneResource:
        return self._run(
            self._client.patch_layer_controls(
                zone,
                layer,
                values,
                clear_bindings=clear_bindings,
            )
        )

    def get_control_surfaces(
        self,
        *,
        device_id: str | None = None,
        driver_id: str | None = None,
        include_driver: bool = False,
    ) -> list[ControlSurface]:
        return self._run(
            self._client.get_control_surfaces(
                device_id=device_id,
                driver_id=driver_id,
                include_driver=include_driver,
            )
        )

    def get_device_controls(self, device_id: str) -> ControlSurface:
        return self._run(self._client.get_device_controls(device_id))

    def get_driver_controls(self, driver_id: str) -> ControlSurface:
        return self._run(self._client.get_driver_controls(driver_id))

    def set_control_values(
        self,
        surface_id: str,
        values: Mapping[str, Any],
        *,
        dry_run: bool = False,
        expected_revision: int | None = None,
    ) -> ControlApplyResult:
        return self._run(
            self._client.set_control_values(
                surface_id,
                values,
                dry_run=dry_run,
                expected_revision=expected_revision,
            )
        )

    def invoke_control_action(
        self,
        surface_id: str,
        action_id: str,
        input: Mapping[str, Any] | None = None,
    ) -> ControlActionResult:
        return self._run(self._client.invoke_control_action(surface_id, action_id, input))

    def get_layouts(self) -> list[LayoutSummary]:
        return self._run(self._client.get_layouts())

    def get_active_layout(self) -> Layout | None:
        return self._run(self._client.get_active_layout())

    def apply_layout(self, layout_id: str) -> MutationResult:
        return self._run(self._client.apply_layout(layout_id))

    def get_scenes(self) -> list[SceneSummary]:
        return self._run(self._client.get_scenes())

    def get_scene(self, scene_id: str) -> SceneDocument:
        return self._run(self._client.get_scene(scene_id))

    def get_live_scene(self) -> SceneDocument:
        return self._run(self._client.get_live_scene())

    def patch_live_scene(
        self,
        *,
        name: str | None = None,
        unassigned_behavior: str | Mapping[str, Any] | None = None,
        if_match: int | None = None,
    ) -> SceneDocument:
        return self._run(
            self._client.patch_live_scene(
                name=name,
                unassigned_behavior=unassigned_behavior,
                if_match=if_match,
            )
        )

    def clear_scene(
        self,
        *,
        zone: str | None = None,
        if_match: int | None = None,
    ) -> SceneDocument:
        return self._run(self._client.clear_scene(zone=zone, if_match=if_match))

    def create_scene(
        self,
        name: str,
        *,
        description: str | None = None,
        enabled: bool | None = None,
        mutation_mode: str | None = None,
    ) -> SceneSummary:
        return self._run(
            self._client.create_scene(
                name, description=description, enabled=enabled, mutation_mode=mutation_mode
            )
        )

    def snapshot_scene(
        self,
        name: str,
        *,
        description: str | None = None,
    ) -> SceneSummary:
        return self._run(self._client.snapshot_scene(name, description=description))

    def update_scene(
        self,
        scene_id: str,
        document: SceneDocument | ReplaceSceneRequest,
        *,
        if_match: int | None = None,
    ) -> SceneDocument:
        return self._run(
            self._client.update_scene(
                scene_id,
                document,
                if_match=if_match,
            )
        )

    def delete_scene(self, scene_id: str) -> DeleteSceneResponse:
        return self._run(self._client.delete_scene(scene_id))

    def activate_scene(self, scene_id: str) -> ActivateSceneResponse:
        return self._run(self._client.activate_scene(scene_id))

    def deactivate_scene(self) -> SceneDocument:
        return self._run(self._client.deactivate_scene())

    def get_zone(self, zone: str) -> ZoneResource:
        return self._run(self._client.get_zone(zone))

    def create_zone(
        self,
        name: str,
        *,
        role: str | None = None,
        color: str | None = None,
        if_match: int | None = None,
    ) -> ZoneResource:
        return self._run(
            self._client.create_zone(
                name,
                role=role,
                color=color,
                if_match=if_match,
            )
        )

    def update_zone(
        self,
        zone: str,
        *,
        name: str | None = None,
        color: str | None | _Unset = _UNSET_SENTINEL,
        brightness: float | None = None,
        enabled: bool | None = None,
        if_match: int | None = None,
    ) -> ZoneResource:
        return self._run(
            self._client.update_zone(
                zone,
                name=name,
                color=color,
                brightness=brightness,
                enabled=enabled,
                if_match=if_match,
            )
        )

    def delete_zone(
        self,
        zone: str,
        *,
        if_match: int | None = None,
    ) -> SceneDocument:
        return self._run(self._client.delete_zone(zone, if_match=if_match))

    def assign_members(
        self,
        zone: str,
        device_id: str,
        *,
        segments: list[str] | None = None,
        if_match: int | None = None,
    ) -> ZoneResource:
        return self._run(
            self._client.assign_members(
                zone,
                device_id,
                segments=segments,
                if_match=if_match,
            )
        )

    def unassign_member(
        self,
        zone: str,
        member: str,
        *,
        if_match: int | None = None,
    ) -> ZoneResource:
        return self._run(self._client.unassign_member(zone, member, if_match=if_match))

    def set_zone_layout(
        self,
        zone: str,
        layout: Mapping[str, Any],
        *,
        if_match: int | None = None,
    ) -> ZoneResource:
        return self._run(self._client.set_zone_layout(zone, layout, if_match=if_match))

    def set_unassigned_behavior(
        self,
        behavior: str | Mapping[str, Any],
        *,
        if_match: int | None = None,
    ) -> SceneDocument:
        return self._run(self._client.set_unassigned_behavior(behavior, if_match=if_match))

    def get_brightness(self) -> float:
        return self._run(self._client.get_brightness())

    def get_audio_spectrum(self) -> SpectrumSnapshot:
        return self._run(self._client.get_audio_spectrum())

    def get_audio_devices(self) -> AudioDevices:
        return self._run(self._client.get_audio_devices())

    def set_audio_device(self, device_id: str, *, live: bool = True) -> ConfigMutationResult:
        return self._run(self._client.set_audio_device(device_id, live=live))
