"""Synchronous wrapper around :class:`HypercolorClient`."""

from __future__ import annotations

import asyncio
from collections.abc import Mapping
from typing import Any, Self

import httpx

from .client import _UNSET_SENTINEL, HypercolorClient, _Unset
from .models.audio import AudioDevices, SpectrumSnapshot
from .models.common import (
    BrightnessUpdate,
    ConfigMutationResult,
    DiscoverResult,
    IdentifyResult,
    MutationResult,
    TransitionSpec,
)
from .models.control import ControlActionResult, ControlApplyResult, ControlSurface
from .models.device import Device
from .models.driver import Driver
from .models.effect import (
    ActiveEffect,
    ApplyEffectResult,
    ControlUpdateResult,
    Effect,
    EffectCoverImage,
    EffectSummary,
)
from .models.layout import Layout, LayoutSummary
from .models.profile import ApplyProfileResult, Profile, ProfileSummary
from .models.scene import ActivateSceneResult, ActiveScene, DeactivateSceneResult, Scene
from .models.spatial import SpatialLayout
from .models.system import HealthStatus, SystemState
from .models.zone import (
    UnassignedBehaviorResult,
    ZoneDeleteResult,
    ZoneListResult,
    ZoneResult,
)


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

    def get_state(self) -> SystemState:
        return self._run(self._client.get_state())

    def set_brightness(self, brightness: int) -> BrightnessUpdate:
        return self._run(self._client.set_brightness(brightness))

    def pause_rendering(self) -> MutationResult:
        return self._run(self._client.pause_rendering())

    def resume_rendering(self) -> MutationResult:
        return self._run(self._client.resume_rendering())

    def get_devices(self, **filters: Any) -> list[Device]:
        return self._run(self._client.get_devices(**filters))

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

    def get_effects(self, **filters: Any) -> list[EffectSummary]:
        return self._run(self._client.get_effects(**filters))

    def get_effect(self, effect_id: str) -> Effect:
        return self._run(self._client.get_effect(effect_id))

    def get_active_effect(self) -> ActiveEffect | None:
        return self._run(self._client.get_active_effect())

    def effect_cover_image_url(self, effect_id: str) -> str:
        return self._client.effect_cover_image_url(effect_id)

    def active_effect_cover_image_url(self) -> str:
        return self._client.active_effect_cover_image_url()

    def get_effect_cover_image(self, effect_id: str) -> EffectCoverImage:
        return self._run(self._client.get_effect_cover_image(effect_id))

    def get_active_effect_cover_image(self) -> EffectCoverImage | None:
        return self._run(self._client.get_active_effect_cover_image())

    def apply_effect(
        self,
        effect_id: str,
        *,
        controls: Mapping[str, Any] | None = None,
        transition: TransitionSpec | Mapping[str, Any] | None = None,
        preset_id: str | None = None,
        render_group: str | None = None,
    ) -> ApplyEffectResult:
        return self._run(
            self._client.apply_effect(
                effect_id,
                controls=controls,
                transition=transition,
                preset_id=preset_id,
                render_group=render_group,
            )
        )

    def update_effect_controls(
        self,
        effect_id: str,
        controls: Mapping[str, Any],
        *,
        if_match: int | None = None,
    ) -> ControlUpdateResult:
        return self._run(
            self._client.update_effect_controls(effect_id, controls, if_match=if_match)
        )

    def reset_controls(self, *, render_group: str | None = None) -> MutationResult:
        return self._run(self._client.reset_controls(render_group=render_group))

    def update_controls(self, controls: Mapping[str, Any]) -> ControlUpdateResult:
        return self._run(self._client.update_controls(controls))

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

    def stop_effect(self) -> MutationResult:
        return self._run(self._client.stop_effect())

    def get_layouts(self) -> list[LayoutSummary]:
        return self._run(self._client.get_layouts())

    def get_active_layout(self) -> Layout | None:
        return self._run(self._client.get_active_layout())

    def apply_layout(self, layout_id: str) -> MutationResult:
        return self._run(self._client.apply_layout(layout_id))

    def get_profiles(self) -> list[ProfileSummary]:
        return self._run(self._client.get_profiles())

    def get_profile(self, profile_id: str) -> Profile:
        return self._run(self._client.get_profile(profile_id))

    def apply_profile(
        self,
        profile_id: str,
        *,
        transition: TransitionSpec | Mapping[str, Any] | None = None,
    ) -> ApplyProfileResult:
        return self._run(self._client.apply_profile(profile_id, transition=transition))

    def get_scenes(self, **filters: Any) -> list[Scene]:
        return self._run(self._client.get_scenes(**filters))

    def get_scene(self, scene_id: str) -> Scene:
        return self._run(self._client.get_scene(scene_id))

    def get_active_scene(self) -> ActiveScene | None:
        return self._run(self._client.get_active_scene())

    def create_scene(
        self,
        name: str,
        *,
        description: str | None = None,
        enabled: bool | None = None,
        mutation_mode: str | None = None,
    ) -> Scene:
        return self._run(
            self._client.create_scene(
                name, description=description, enabled=enabled, mutation_mode=mutation_mode
            )
        )

    def update_scene(
        self,
        scene_id: str,
        name: str,
        *,
        description: str | None = None,
        enabled: bool | None = None,
        mutation_mode: str | None = None,
    ) -> Scene:
        return self._run(
            self._client.update_scene(
                scene_id,
                name,
                description=description,
                enabled=enabled,
                mutation_mode=mutation_mode,
            )
        )

    def delete_scene(self, scene_id: str) -> MutationResult:
        return self._run(self._client.delete_scene(scene_id))

    def activate_scene(self, scene_id: str) -> ActivateSceneResult:
        return self._run(self._client.activate_scene(scene_id))

    def deactivate_scene(self) -> DeactivateSceneResult:
        return self._run(self._client.deactivate_scene())

    def get_zones(self, scene_id: str) -> ZoneListResult:
        return self._run(self._client.get_zones(scene_id))

    def get_zone(self, scene_id: str, zone_id: str) -> ZoneResult:
        return self._run(self._client.get_zone(scene_id, zone_id))

    def create_zone(
        self,
        scene_id: str,
        name: str,
        *,
        color: str | None = None,
        if_match: int | None = None,
    ) -> ZoneResult:
        return self._run(self._client.create_zone(scene_id, name, color=color, if_match=if_match))

    def update_zone(
        self,
        scene_id: str,
        zone_id: str,
        *,
        name: str | None = None,
        description: str | None | _Unset = _UNSET_SENTINEL,
        color: str | None | _Unset = _UNSET_SENTINEL,
        brightness: float | None = None,
        enabled: bool | None = None,
        make_primary: bool | None = None,
        if_match: int | None = None,
    ) -> ZoneResult:
        return self._run(
            self._client.update_zone(
                scene_id,
                zone_id,
                name=name,
                description=description,
                color=color,
                brightness=brightness,
                enabled=enabled,
                make_primary=make_primary,
                if_match=if_match,
            )
        )

    def delete_zone(
        self,
        scene_id: str,
        zone_id: str,
        *,
        if_match: int | None = None,
    ) -> ZoneDeleteResult:
        return self._run(self._client.delete_zone(scene_id, zone_id, if_match=if_match))

    def assign_devices(
        self,
        scene_id: str,
        zone_id: str,
        device_zones: list[str | Mapping[str, Any]],
        *,
        if_match: int | None = None,
    ) -> ZoneListResult:
        return self._run(
            self._client.assign_devices(scene_id, zone_id, device_zones, if_match=if_match)
        )

    def unassign_device(
        self,
        scene_id: str,
        zone_id: str,
        device_zone_id: str,
        *,
        if_match: int | None = None,
    ) -> ZoneListResult:
        return self._run(
            self._client.unassign_device(scene_id, zone_id, device_zone_id, if_match=if_match)
        )

    def set_zone_layout(
        self,
        scene_id: str,
        zone_id: str,
        layout: SpatialLayout | Mapping[str, Any],
        *,
        if_match: int | None = None,
    ) -> ZoneResult:
        return self._run(
            self._client.set_zone_layout(scene_id, zone_id, layout, if_match=if_match)
        )

    def set_unassigned_behavior(
        self,
        scene_id: str,
        behavior: str | Mapping[str, Any],
        *,
        if_match: int | None = None,
    ) -> UnassignedBehaviorResult:
        return self._run(
            self._client.set_unassigned_behavior(scene_id, behavior, if_match=if_match)
        )

    def get_brightness(self) -> BrightnessUpdate:
        return self._run(self._client.get_brightness())

    def get_audio_spectrum(self) -> SpectrumSnapshot:
        return self._run(self._client.get_audio_spectrum())

    def get_audio_devices(self) -> AudioDevices:
        return self._run(self._client.get_audio_devices())

    def set_audio_device(self, device_id: str, *, live: bool = True) -> ConfigMutationResult:
        return self._run(self._client.set_audio_device(device_id, live=live))
