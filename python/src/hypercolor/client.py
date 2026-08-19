"""Async client for the Hypercolor daemon API."""

from __future__ import annotations

import json
from collections.abc import Mapping
from typing import Any, Self, TypeVar
from urllib.parse import quote

import httpx
import msgspec

from ._generated.api.controls import (
    apply_control_surface_values as generated_apply_control_surface_values,
    get_device_control_surface as generated_get_device_control_surface,
    get_driver_control_surface as generated_get_driver_control_surface,
    invoke_control_surface_action as generated_invoke_control_surface_action,
    list_control_surfaces as generated_list_control_surfaces,
)
from ._generated.api.devices import (
    discover_devices as generated_discover_devices,
    get_device as generated_get_device,
    identify_device as generated_identify_device,
    list_devices as generated_list_devices,
    update_device as generated_update_device,
)
from ._generated.api.drivers import list_drivers as generated_list_drivers
from ._generated.api.effects import (
    get_effect as generated_get_effect,
    list_effects as generated_list_effects,
)
from ._generated.api.layouts import (
    apply_layout as generated_apply_layout,
    get_active_layout as generated_get_active_layout,
    list_layouts as generated_list_layouts,
)
from ._generated.api.profiles import (
    apply_profile as generated_apply_profile,
    get_profile as generated_get_profile,
    list_profiles as generated_list_profiles,
)
from ._generated.api.scenes import (
    activate_scene as generated_activate_scene,
    list_scenes as generated_list_scenes,
)
from ._generated.api.system import (
    get_status as generated_get_status,
    health_check as generated_health_check,
    list_audio_devices as generated_list_audio_devices,
)
from ._generated.models.apply_control_changes_request import ApplyControlChangesRequest
from ._generated.models.apply_profile_request import ApplyProfileRequest
from ._generated.models.discover_request import DiscoverRequest
from ._generated.models.identify_request import IdentifyRequest
from ._generated.models.invoke_control_action_request import InvokeControlActionRequest
from ._generated.models.update_device_request import UpdateDeviceRequest
from ._generated.types import UNSET
from .constants import API_PREFIX, DEFAULT_HOST, DEFAULT_PORT, DEFAULT_TIMEOUT, WS_PATH
from .exceptions import (
    ApiErrorDetails,
    HypercolorApiError,
    HypercolorAuthenticationError,
    HypercolorConflictError,
    HypercolorConnectionError,
    HypercolorNotFoundError,
    HypercolorPreconditionError,
    HypercolorRateLimitError,
    HypercolorUnavailableError,
    HypercolorValidationError,
)
from .models.audio import AudioDevices, SpectrumSnapshot
from .models.common import (
    ConfigMutationResult,
    DiscoverResult,
    IdentifyResult,
    MutationResult,
    TransitionSpec,
)
from .models.control import ControlActionResult, ControlApplyResult, ControlSurface
from .models.device import Device
from .models.display import DisplayFaceAssignment, DisplaySummary
from .models.driver import Driver
from .models.effect import (
    ApplyEffectResponse,
    Effect,
    EffectCoverImage,
    EffectPreset,
    EffectSummary,
)
from .models.layout import Layout, LayoutSummary
from .models.library import (
    Favorite,
    Playlist,
    Preset,
)
from .models.profile import ApplyProfileResult, Profile, ProfileSummary
from .models.scene import ActivateSceneResult, Scene, SceneDocument
from .models.system import HealthStatus, OutputState, SystemState
from .models.zone import Zone
from .websocket import HypercolorEventStream

ModelT = TypeVar("ModelT")


class _Unset:
    """Marker type distinguishing "leave unchanged" from an explicit ``None``."""

    __slots__ = ()


_UNSET_SENTINEL = _Unset()


def _if_match_headers(revision: int | None) -> dict[str, str] | None:
    if revision is None:
        return None
    return {"If-Match": f'"{revision}"'}


def _etag_revision(response: httpx.Response) -> int | None:
    etag = response.headers.get("etag")
    if etag is None:
        return None
    try:
        return int(etag.strip().strip('"'))
    except ValueError:
        return None


_DEVICE_FILTERS = {"offset", "limit", "status", "backend", "backend_id", "driver", "q"}
_SCENE_FILTERS: set[str] = set()


class HypercolorClient:
    """Async client for the Hypercolor daemon API."""

    def __init__(
        self,
        host: str = DEFAULT_HOST,
        port: int = DEFAULT_PORT,
        api_key: str | None = None,
        timeout: float = DEFAULT_TIMEOUT,
        *,
        transport: httpx.AsyncBaseTransport | None = None,
        httpx_client: httpx.AsyncClient | None = None,
    ) -> None:
        if transport is not None and httpx_client is not None:
            message = "transport and httpx_client are mutually exclusive"
            raise ValueError(message)
        self.host = host
        self.port = port
        self.api_key = api_key
        self.timeout = timeout
        self.root_url = f"http://{host}:{port}"
        self.base_url = f"http://{host}:{port}{API_PREFIX}"
        self.ws_url = f"ws://{host}:{port}{WS_PATH}"
        self._client = httpx_client or httpx.AsyncClient(timeout=timeout, transport=transport)
        self._owns_client = httpx_client is None

    async def __aenter__(self) -> Self:
        """Return self for async context-manager usage."""
        return self

    async def __aexit__(self, *_exc_info: object) -> None:
        """Close the underlying HTTP client."""
        await self.aclose()

    async def aclose(self) -> None:
        """Close the underlying HTTP client."""
        if self._owns_client:
            await self._client.aclose()

    def events(self) -> HypercolorEventStream:
        """Create a WebSocket event stream bound to this client."""
        return HypercolorEventStream(self)

    async def health(self) -> HealthStatus:
        """Run the daemon health check."""
        return await self._generated_model(
            generated_health_check._get_kwargs(),
            HealthStatus,
            envelope=False,
        )

    async def get_status(self) -> SystemState:
        """Return the current daemon status snapshot."""

        return await self._generated_model(
            generated_get_status._get_kwargs(),
            SystemState,
        )

    async def get_state(self) -> SystemState:
        """Backward-compatible alias for :meth:`get_status`."""

        return await self.get_status()

    async def get_output(self) -> OutputState:
        """Return global output power and brightness."""

        return await self._request_model("GET", "/output", OutputState)

    async def set_output(
        self,
        *,
        power: str | None = None,
        brightness: float | None = None,
    ) -> OutputState:
        """Patch global output power, brightness, or both.

        The daemon refuses a patch that sets neither field, so at least
        one argument is required. Brightness is the wire's `0.0` to
        `1.0` float, not a percentage.
        """

        body: dict[str, Any] = {}
        if power is not None:
            body["power"] = power
        if brightness is not None:
            body["brightness"] = brightness
        if not body:
            message = "set_output requires power, brightness, or both"
            raise ValueError(message)
        return await self._request_model("PATCH", "/output", OutputState, body=body)

    async def get_brightness(self) -> float:
        """Return the global daemon brightness as a `0.0` to `1.0` float."""

        return (await self.get_output()).brightness

    async def set_brightness(self, brightness: float) -> OutputState:
        """Set the global daemon brightness as a `0.0` to `1.0` float."""

        return await self.set_output(brightness=brightness)

    async def set_output_power(self, *, paused: bool) -> OutputState:
        """Set global output power without discarding live scene state."""

        return await self.set_output(power="paused" if paused else "running")

    async def pause_rendering(self) -> OutputState:
        """Pause all output while preserving live scene state."""

        return await self.set_output_power(paused=True)

    async def resume_rendering(self) -> OutputState:
        """Resume output from the preserved live scene state."""

        return await self.set_output_power(paused=False)

    async def get_devices(self, **filters: Any) -> list[Device]:
        """List devices."""
        if any(key not in _DEVICE_FILTERS for key in filters):
            return await self._request_items("GET", "/devices", Device, params=filters)
        backend_id = filters.get("backend_id", filters.get("backend"))
        return await self._generated_items(
            generated_list_devices._get_kwargs(
                offset=_generated_param(filters.get("offset")),
                limit=_generated_param(filters.get("limit")),
                status=_generated_param(filters.get("status")),
                backend_id=_generated_param(backend_id),
                driver=_generated_param(filters.get("driver")),
                q=_generated_param(filters.get("q")),
            ),
            Device,
        )

    async def get_device(self, device_id: str) -> Device:
        """Fetch a single device."""
        return await self._generated_model(
            generated_get_device._get_kwargs(device_id),
            Device,
        )

    async def update_device(self, device_id: str, **fields: Any) -> Device:
        """Update device configuration."""
        return await self._generated_model(
            generated_update_device._get_kwargs(
                device_id,
                body=UpdateDeviceRequest.from_dict(fields),
            ),
            Device,
        )

    async def discover_devices(
        self,
        backends: list[str] | None = None,
        timeout_ms: int | None = None,
    ) -> DiscoverResult:
        """Trigger a device discovery scan."""
        body = _drop_none({"backends": backends, "timeout_ms": timeout_ms})
        kwargs = (
            generated_discover_devices._get_kwargs(body=DiscoverRequest.from_dict(body))
            if body
            else generated_discover_devices._get_kwargs()
        )
        return await self._generated_model(
            kwargs,
            DiscoverResult,
        )

    async def identify_device(
        self,
        device_id: str,
        *,
        duration_ms: int | None = None,
        color: str | None = None,
    ) -> IdentifyResult:
        """Flash a device for identification."""
        body = _drop_none({"duration_ms": duration_ms, "color": color})
        kwargs = (
            generated_identify_device._get_kwargs(
                device_id,
                body=IdentifyRequest.from_dict(body),
            )
            if body
            else generated_identify_device._get_kwargs(device_id)
        )
        return await self._generated_model(
            kwargs,
            IdentifyResult,
        )

    async def get_drivers(self) -> list[Driver]:
        """List registered driver modules."""
        return await self._generated_items(
            generated_list_drivers._get_kwargs(),
            Driver,
        )

    async def get_effects(self, **filters: Any) -> list[EffectSummary]:
        """List available effects."""
        if filters:
            return await self._request_items("GET", "/effects", EffectSummary, params=filters)
        return await self._generated_items(
            generated_list_effects._get_kwargs(),
            EffectSummary,
        )

    async def get_effect(self, effect_id: str) -> Effect:
        """Fetch a single effect with controls."""
        return await self._generated_model(
            generated_get_effect._get_kwargs(effect_id),
            Effect,
        )

    async def get_effect_presets(self, effect_id: str) -> list[EffectPreset]:
        """List bundled and saved presets for one effect."""
        return await self._request_items(
            "GET",
            f"/effects/{_quote_path(effect_id)}/presets",
            EffectPreset,
        )

    def effect_cover_image_url(self, effect_id: str) -> str:
        """Return the absolute cover image URL for an effect."""
        return self._request_url(f"/effects/{_quote_path(effect_id)}/cover")

    async def get_effect_cover_image(self, effect_id: str) -> EffectCoverImage:
        """Fetch an effect cover image."""
        response = await self._response_request(
            "GET",
            f"/effects/{_quote_path(effect_id)}/cover",
        )
        return _cover_image(response, self.effect_cover_image_url(effect_id))

    async def apply_effect(
        self,
        effect_id: str,
        *,
        controls: Mapping[str, Any] | None = None,
        transition: str | Mapping[str, Any] | None = None,
        preset_id: str | None = None,
        zone: str | None = None,
        if_match: int | None = None,
    ) -> ApplyEffectResponse:
        """Apply an effect with optional control overrides.

        ``zone`` targets a specific zone by id; omitted applies to
        the scene's primary zone.
        """
        body = _drop_none(
            {
                "controls": (
                    {str(name): _effect_control_value(value) for name, value in controls.items()}
                    if controls is not None
                    else None
                ),
                "transition": _transition_value(transition),
                "preset_id": preset_id,
                "zone": zone,
            }
        )
        return await self._request_model(
            "POST",
            f"/effects/{_quote_path(effect_id)}/apply",
            ApplyEffectResponse,
            body=body or None,
            headers=_if_match_headers(if_match),
        )

    async def apply_effect_preset(
        self,
        effect_id: str,
        preset_id: str,
        *,
        controls: Mapping[str, Any] | None = None,
        transition: str | Mapping[str, Any] | None = None,
        zone: str | None = None,
        if_match: int | None = None,
    ) -> ApplyEffectResponse:
        """Apply a bundled or saved preset to an effect and optional zone."""
        body = _drop_none(
            {
                "controls": (
                    {str(name): _effect_control_value(value) for name, value in controls.items()}
                    if controls is not None
                    else None
                ),
                "transition": _transition_value(transition),
                "zone": zone,
            }
        )
        return await self._request_model(
            "POST",
            f"/effects/{_quote_path(effect_id)}/presets/{_quote_path(preset_id)}/apply",
            ApplyEffectResponse,
            body=body or None,
            headers=_if_match_headers(if_match),
        )

    async def upload_effect(
        self,
        file_name: str,
        content: bytes | str,
    ) -> dict[str, Any]:
        """Upload and install an HTML effect."""
        data = content.encode() if isinstance(content, str) else content
        files = {"file": (file_name, data, "text/html")}
        try:
            response = await self._client.post(
                self._request_url("/effects/install"),
                files=files,
                headers=self._auth_headers(),
            )
            response.raise_for_status()
        except httpx.ConnectError as exc:
            raise HypercolorConnectionError("Failed to connect to the Hypercolor daemon") from exc
        except httpx.TimeoutException as exc:
            raise HypercolorConnectionError("Hypercolor request timed out") from exc
        except httpx.HTTPStatusError as exc:
            raise self._map_http_error(exc) from exc

        try:
            decoded = msgspec.json.decode(response.content)
        except msgspec.DecodeError:
            decoded = response.text
        return self._unwrap_data(decoded)

    async def patch_layer_controls(
        self,
        zone: str,
        layer: str,
        values: Mapping[str, Any],
        *,
        clear_bindings: list[str] | None = None,
    ) -> Zone:
        """Patch values on one live scene layer."""
        body: dict[str, Any] = {
            "values": {str(name): _effect_control_value(value) for name, value in values.items()}
        }
        if clear_bindings:
            body["clear_bindings"] = clear_bindings
        return await self._request_model(
            "PATCH",
            f"/scene/zones/{_quote_path(zone)}/layers/{_quote_path(layer)}/controls",
            Zone,
            body=body,
        )

    async def get_control_surfaces(
        self,
        *,
        device_id: str | None = None,
        driver_id: str | None = None,
        include_driver: bool = False,
    ) -> list[ControlSurface]:
        """List control surfaces for a selected device or driver."""
        kwargs = generated_list_control_surfaces._get_kwargs()
        params = _drop_none(
            {
                "device_id": device_id,
                "driver_id": driver_id,
                "include_driver": include_driver if include_driver else None,
            }
        )
        if params:
            kwargs["params"] = params
        payload = await self._generated_payload(kwargs)
        surfaces = payload.get("surfaces") if isinstance(payload, dict) else None
        if not isinstance(surfaces, list):
            return []
        return [
            self._convert(surface, ControlSurface)
            for surface in surfaces
            if isinstance(surface, Mapping)
        ]

    async def get_device_controls(self, device_id: str) -> ControlSurface:
        """Return a device control surface."""
        return await self._generated_model(
            generated_get_device_control_surface._get_kwargs(device_id),
            ControlSurface,
        )

    async def get_driver_controls(self, driver_id: str) -> ControlSurface:
        """Return a driver control surface."""
        return await self._generated_model(
            generated_get_driver_control_surface._get_kwargs(driver_id),
            ControlSurface,
        )

    async def set_control_values(
        self,
        surface_id: str,
        values: Mapping[str, Any],
        *,
        dry_run: bool = False,
        expected_revision: int | None = None,
    ) -> ControlApplyResult:
        """Apply one or more control values to a control surface."""
        body = _control_changes_request(
            surface_id,
            values,
            dry_run=dry_run,
            expected_revision=expected_revision,
        )
        return await self._generated_model(
            generated_apply_control_surface_values._get_kwargs(
                surface_id,
                body=body,
            ),
            ControlApplyResult,
        )

    async def invoke_control_action(
        self,
        surface_id: str,
        action_id: str,
        input: Mapping[str, Any] | None = None,
    ) -> ControlActionResult:
        """Invoke a control-surface action."""
        body = InvokeControlActionRequest()
        if input is not None:
            body["input"] = {str(key): _control_api_value(value) for key, value in input.items()}
        return await self._generated_model(
            generated_invoke_control_surface_action._get_kwargs(
                surface_id,
                action_id,
                body=body,
            ),
            ControlActionResult,
        )

    async def get_layouts(self) -> list[LayoutSummary]:
        """List layouts."""
        return await self._generated_items(
            generated_list_layouts._get_kwargs(),
            LayoutSummary,
        )

    async def get_active_layout(self) -> Layout | None:
        """Return the active layout if one exists."""
        try:
            return await self._generated_model(
                generated_get_active_layout._get_kwargs(),
                Layout,
            )
        except HypercolorNotFoundError:
            return None

    async def apply_layout(self, layout_id: str) -> MutationResult:
        """Apply a layout."""
        return await self._generated_model(
            generated_apply_layout._get_kwargs(layout_id),
            MutationResult,
        )

    async def get_profiles(self) -> list[ProfileSummary]:
        """List saved profiles."""
        return await self._generated_items(
            generated_list_profiles._get_kwargs(),
            ProfileSummary,
        )

    async def get_profile(self, profile_id: str) -> Profile:
        """Fetch a single profile."""
        return await self._generated_model(
            generated_get_profile._get_kwargs(profile_id),
            Profile,
        )

    async def apply_profile(
        self,
        profile_id: str,
        *,
        transition: TransitionSpec | Mapping[str, Any] | None = None,
    ) -> ApplyProfileResult:
        """Apply a saved profile."""
        body = _drop_none({"transition": _to_json_mapping(transition)})
        kwargs = (
            generated_apply_profile._get_kwargs(
                profile_id,
                body=ApplyProfileRequest.from_dict(body),
            )
            if body
            else generated_apply_profile._get_kwargs(profile_id)
        )
        return await self._generated_model(
            kwargs,
            ApplyProfileResult,
        )

    async def save_profile(
        self,
        name: str,
        *,
        description: str | None = None,
        brightness: int | None = None,
        force: bool = False,
    ) -> Profile:
        """Save a profile from the current runtime state."""
        body = _drop_none(
            {
                "name": name,
                "description": description,
                "brightness": brightness,
                "force": force,
            }
        )
        return await self._request_model("POST", "/profiles", Profile, body=body)

    async def get_scenes(self, **filters: Any) -> list[Scene]:
        """List available scenes."""
        if any(key not in _SCENE_FILTERS for key in filters):
            return await self._request_items("GET", "/scenes", Scene, params=filters)
        return await self._generated_items(
            generated_list_scenes._get_kwargs(),
            Scene,
        )

    async def get_scene(self, scene_id: str) -> Scene:
        """Fetch a single scene."""
        return await self._request_model("GET", f"/scenes/{_quote_path(scene_id)}", Scene)

    async def get_live_scene(self) -> SceneDocument:
        """Return the full live scene tree."""
        return await self._request_model("GET", "/scene", SceneDocument)

    async def patch_live_scene(
        self,
        *,
        name: str | None = None,
        unassigned_behavior: str | Mapping[str, Any] | None = None,
        if_match: int | None = None,
    ) -> SceneDocument:
        """Patch scene-level fields on the live tree."""
        behavior = (
            dict(unassigned_behavior)
            if isinstance(unassigned_behavior, Mapping)
            else unassigned_behavior
        )
        body = _drop_none({"name": name, "unassigned_behavior": behavior})
        return await self._request_model(
            "PATCH",
            "/scene",
            SceneDocument,
            body=body,
            headers=_if_match_headers(if_match),
        )

    async def deactivate_scene(self) -> SceneDocument:
        """Return to the auto-managed default scene."""
        return await self._request_model("POST", "/scene/deactivate", SceneDocument)

    async def clear_scene(
        self,
        *,
        zone: str | None = None,
        if_match: int | None = None,
    ) -> SceneDocument:
        """Clear one zone's layer stack, or every non-display zone."""
        body = _drop_none({"zone": zone})
        return await self._request_model(
            "POST",
            "/scene/clear",
            SceneDocument,
            body=body or None,
            headers=_if_match_headers(if_match),
        )

    async def create_scene(
        self,
        name: str,
        *,
        description: str | None = None,
        enabled: bool | None = None,
        mutation_mode: str | None = None,
    ) -> Scene:
        """Create a scene."""
        body = _drop_none(
            {
                "name": name,
                "description": description,
                "enabled": enabled,
                "mutation_mode": mutation_mode,
            }
        )
        return await self._request_model("POST", "/scenes", Scene, body=body)

    async def activate_scene(self, scene_id: str) -> ActivateSceneResult:
        """Trigger a scene manually."""
        return await self._generated_model(
            generated_activate_scene._get_kwargs(scene_id),
            ActivateSceneResult,
        )

    async def update_scene(
        self,
        scene_id: str,
        name: str,
        *,
        description: str | None = None,
        enabled: bool | None = None,
        mutation_mode: str | None = None,
    ) -> Scene:
        """Update a scene.

        The daemon replaces ``name`` and ``description`` wholesale — echo
        the existing description back when renaming or it is cleared.
        """
        body = _drop_none(
            {
                "name": name,
                "description": description,
                "enabled": enabled,
                "mutation_mode": mutation_mode,
            }
        )
        return await self._request_model(
            "PUT", f"/scenes/{_quote_path(scene_id)}", Scene, body=body
        )

    async def delete_scene(self, scene_id: str) -> MutationResult:
        """Delete a scene."""
        return await self._request_model(
            "DELETE", f"/scenes/{_quote_path(scene_id)}", MutationResult
        )

    async def get_zone(self, zone: str) -> Zone:
        """Fetch one zone from the live scene tree."""
        return await self._request_model("GET", f"/scene/zones/{_quote_path(zone)}", Zone)

    async def create_zone(
        self,
        name: str,
        *,
        role: str | None = None,
        color: str | None = None,
        if_match: int | None = None,
    ) -> Zone:
        """Create a zone in the live scene tree."""
        body = _drop_none({"name": name, "role": role, "color": color})
        return await self._request_model(
            "POST",
            "/scene/zones",
            Zone,
            body=body,
            headers=_if_match_headers(if_match),
        )

    async def update_zone(
        self,
        zone: str,
        *,
        name: str | None = None,
        color: str | None | _Unset = _UNSET_SENTINEL,
        brightness: float | None = None,
        enabled: bool | None = None,
        if_match: int | None = None,
    ) -> Zone:
        """Patch one live zone; an explicit ``None`` clears its color."""
        body: dict[str, Any] = _drop_none(
            {
                "name": name,
                "brightness": brightness,
                "enabled": enabled,
            }
        )
        if not isinstance(color, _Unset):
            body["color"] = color
        return await self._request_model(
            "PATCH",
            f"/scene/zones/{_quote_path(zone)}",
            Zone,
            body=body,
            headers=_if_match_headers(if_match),
        )

    async def delete_zone(
        self,
        zone: str,
        *,
        if_match: int | None = None,
    ) -> SceneDocument:
        """Delete one zone from the live scene tree."""
        return await self._request_model(
            "DELETE",
            f"/scene/zones/{_quote_path(zone)}",
            SceneDocument,
            headers=_if_match_headers(if_match),
        )

    async def assign_members(
        self,
        zone: str,
        device_id: str,
        *,
        segments: list[str] | None = None,
        if_match: int | None = None,
    ) -> Zone:
        """Assign a device and selected segments to one live zone."""
        return await self._request_model(
            "POST",
            f"/scene/zones/{_quote_path(zone)}/members",
            Zone,
            body={"device_id": device_id, "segments": segments or []},
            headers=_if_match_headers(if_match),
        )

    async def unassign_member(
        self,
        zone: str,
        member: str,
        *,
        if_match: int | None = None,
    ) -> Zone:
        """Remove one membership from a live zone."""
        return await self._request_model(
            "DELETE",
            f"/scene/zones/{_quote_path(zone)}/members/{_quote_path(member)}",
            Zone,
            headers=_if_match_headers(if_match),
        )

    async def set_zone_layout(
        self,
        zone: str,
        layout: Mapping[str, Any],
        *,
        if_match: int | None = None,
    ) -> Zone:
        """Replace a live zone's compact member-placement layout."""
        return await self._request_model(
            "PUT",
            f"/scene/zones/{_quote_path(zone)}/layout",
            Zone,
            body={str(key): value for key, value in layout.items()},
            headers=_if_match_headers(if_match),
        )

    async def set_unassigned_behavior(
        self,
        behavior: str | Mapping[str, Any],
        *,
        if_match: int | None = None,
    ) -> SceneDocument:
        """Set the unassigned-output policy on the live scene."""
        return await self.patch_live_scene(
            unassigned_behavior=behavior,
            if_match=if_match,
        )

    async def get_favorites(self) -> list[Favorite]:
        """List favorite effects."""
        return await self._request_items("GET", "/library/favorites", Favorite)

    async def add_favorite(self, effect_id: str) -> dict[str, Any]:
        """Add or update a favorite effect."""
        return await self._request_payload(
            "POST",
            "/library/favorites",
            body={"effect": effect_id},
        )

    async def remove_favorite(self, effect_id: str) -> dict[str, Any]:
        """Remove a favorite effect."""
        return await self._request_payload(
            "DELETE",
            f"/library/favorites/{_quote_path(effect_id)}",
        )

    async def get_presets(self) -> list[Preset]:
        """List saved presets."""
        return await self._request_items("GET", "/library/presets", Preset)

    async def get_preset(self, preset_id: str) -> Preset:
        """Fetch a saved preset."""
        return await self._request_model(
            "GET",
            f"/library/presets/{_quote_path(preset_id)}",
            Preset,
        )

    async def save_preset(
        self,
        name: str,
        effect_id: str,
        *,
        description: str | None = None,
        controls: Mapping[str, Any] | None = None,
        tags: list[str] | None = None,
    ) -> Preset:
        """Save an effect preset."""
        body = _drop_none(
            {
                "name": name,
                "description": description,
                "effect": effect_id,
                "controls": dict(controls) if controls is not None else None,
                "tags": tags,
            }
        )
        return await self._request_model("POST", "/library/presets", Preset, body=body)

    async def delete_preset(self, preset_id: str) -> dict[str, Any]:
        """Delete a saved preset."""
        return await self._request_payload(
            "DELETE",
            f"/library/presets/{_quote_path(preset_id)}",
        )

    async def get_playlists(self) -> list[Playlist]:
        """List saved playlists."""
        return await self._request_items("GET", "/library/playlists", Playlist)

    async def get_playlist(self, playlist_id: str) -> Playlist:
        """Fetch a saved playlist."""
        return await self._request_model(
            "GET",
            f"/library/playlists/{_quote_path(playlist_id)}",
            Playlist,
        )

    async def activate_playlist(self, playlist_id: str) -> dict[str, Any]:
        """Start playlist playback."""
        return await self._request_payload(
            "POST",
            f"/library/playlists/{_quote_path(playlist_id)}/activate",
        )

    async def list_displays(self) -> list[DisplaySummary]:
        """List devices that expose display faces."""
        return await self._request_list("GET", "/displays", DisplaySummary)

    async def set_display_face(
        self,
        display_id: str,
        effect_id: str,
        *,
        controls: Mapping[str, Any] | None = None,
        blend_mode: str | None = None,
        opacity: float | None = None,
    ) -> DisplayFaceAssignment:
        """Assign an effect to a display face."""
        body = _drop_none(
            {
                "effect_id": effect_id,
                "controls": (
                    {str(key): _display_control_value(value) for key, value in controls.items()}
                    if controls is not None
                    else None
                ),
                "blend_mode": blend_mode,
                "opacity": opacity,
            }
        )
        return await self._request_model(
            "PUT",
            f"/displays/{_quote_path(display_id)}/face",
            DisplayFaceAssignment,
            body=body,
        )

    async def run_diagnostics(
        self,
        *,
        checks: list[str] | None = None,
        system: bool | None = None,
    ) -> dict[str, Any]:
        """Run daemon diagnostics."""
        body = _drop_none({"checks": checks, "system": system})
        return await self._request_payload("POST", "/diagnose", body=body)

    async def get_audio_spectrum(self) -> SpectrumSnapshot:
        """Return the current audio spectrum snapshot."""

        message = (
            "Audio spectrum snapshots are only available over the Hypercolor WebSocket stream"
        )
        raise HypercolorNotFoundError(message, status_code=404)

    async def get_audio_devices(self) -> AudioDevices:
        """Return the available audio capture devices."""

        return await self._generated_model(
            generated_list_audio_devices._get_kwargs(),
            AudioDevices,
        )

    async def _generated_model(
        self,
        kwargs: Mapping[str, Any],
        model_type: type[ModelT],
        *,
        envelope: bool = True,
    ) -> ModelT:
        payload = await self._generated_payload(kwargs, envelope=envelope)
        return self._convert(payload, model_type)

    async def _generated_items(
        self,
        kwargs: Mapping[str, Any],
        item_type: type[ModelT],
    ) -> list[ModelT]:
        data = await self._generated_payload(kwargs)
        items = data["items"] if isinstance(data, dict) else []
        return [self._convert(item, item_type) for item in items]

    async def _generated_payload(
        self,
        kwargs: Mapping[str, Any],
        *,
        envelope: bool = True,
    ) -> Any:
        response = await self._generated_request(kwargs)
        payload = self._unwrap_data(response) if envelope else response
        return _normalize_payload(payload)

    async def _generated_request(self, kwargs: Mapping[str, Any]) -> Any:
        try:
            request_kwargs = _drop_unset_json_body(kwargs)
            request_kwargs["url"] = self._absolute_url(str(request_kwargs["url"]))
            headers = {
                **dict(request_kwargs.get("headers") or {}),
                **self._auth_headers(),
            }
            if headers:
                request_kwargs["headers"] = headers
            response = await self._client.request(**request_kwargs)
            response.raise_for_status()
        except httpx.ConnectError as exc:
            raise HypercolorConnectionError("Failed to connect to the Hypercolor daemon") from exc
        except httpx.TimeoutException as exc:
            raise HypercolorConnectionError("Hypercolor request timed out") from exc
        except httpx.HTTPStatusError as exc:
            raise self._map_http_error(exc) from exc

        try:
            return msgspec.json.decode(response.content)
        except msgspec.DecodeError:
            return response.text

    async def set_audio_device(
        self,
        device_id: str,
        *,
        live: bool = True,
    ) -> ConfigMutationResult:
        """Persist the selected audio input device.

        The config key resource takes the value as the request body, and
        ``live`` decides whether the daemon re-applies it to the running
        audio pipeline.
        """

        return await self._request_model(
            "PUT",
            "/config/keys/audio.device",
            ConfigMutationResult,
            body=device_id,
            params={"live": live},
        )

    async def _request_items(
        self,
        method: str,
        path: str,
        item_type: type[ModelT],
        *,
        body: Mapping[str, Any] | None = None,
        params: Mapping[str, Any] | None = None,
    ) -> list[ModelT]:
        response = await self._raw_request(method, path, body=body, params=params)
        data = self._unwrap_data(response)
        items = data["items"] if isinstance(data, dict) else []
        return [self._convert(item, item_type) for item in items]

    async def _request_list(
        self,
        method: str,
        path: str,
        item_type: type[ModelT],
        *,
        body: Mapping[str, Any] | None = None,
        params: Mapping[str, Any] | None = None,
    ) -> list[ModelT]:
        response = await self._raw_request(method, path, body=body, params=params)
        data = self._unwrap_data(response)
        if not isinstance(data, list):
            message = "Unexpected Hypercolor list response"
            raise HypercolorApiError(message)
        return [self._convert(item, item_type) for item in data]

    async def _request_model(
        self,
        method: str,
        path: str,
        model_type: type[ModelT],
        *,
        body: Any = None,
        params: Mapping[str, Any] | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> ModelT:
        payload = await self._request_payload(
            method, path, body=body, params=params, headers=headers
        )
        return self._convert(payload, model_type)

    async def _request_payload(
        self,
        method: str,
        path: str,
        *,
        body: Any = None,
        params: Mapping[str, Any] | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> Any:
        response = await self._raw_request(method, path, body=body, params=params, headers=headers)
        return self._unwrap_data(response)

    async def _raw_request(
        self,
        method: str,
        path: str,
        *,
        body: Any = None,
        params: Mapping[str, Any] | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> Any:
        response = await self._response_request(
            method, path, body=body, params=params, headers=headers
        )

        try:
            return msgspec.json.decode(response.content)
        except msgspec.DecodeError:
            return response.text

    async def _response_request(
        self,
        method: str,
        path: str,
        *,
        body: Any = None,
        params: Mapping[str, Any] | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> httpx.Response:
        try:
            request_path = _request_path(path)
            request_headers = self._auth_headers()
            if headers:
                request_headers.update(headers)
            response = await self._client.request(
                method,
                self._request_url(request_path),
                json=body,
                params=_drop_none(params or {}),
                headers=request_headers,
            )
            response.raise_for_status()
        except httpx.ConnectError as exc:
            raise HypercolorConnectionError("Failed to connect to the Hypercolor daemon") from exc
        except httpx.TimeoutException as exc:
            raise HypercolorConnectionError("Hypercolor request timed out") from exc
        except httpx.HTTPStatusError as exc:
            raise self._map_http_error(exc) from exc

        return response

    def _auth_headers(self) -> dict[str, str]:
        if self.api_key is None:
            return {}
        return {"Authorization": f"Bearer {self.api_key}"}

    def _request_url(self, path: str) -> str:
        request_path = _request_path(path)
        return self._absolute_url(request_path)

    def _absolute_url(self, path: str) -> str:
        if path.startswith(("http://", "https://")):
            return path
        request_path = path if path.startswith("/") else f"/{path}"
        return f"{self.root_url}{request_path}"

    @staticmethod
    def _unwrap_data(response: Any) -> Any:
        if not isinstance(response, dict) or "data" not in response:
            message = "Unexpected Hypercolor response envelope"
            raise HypercolorApiError(message)
        return response["data"]

    @staticmethod
    def _convert(payload: Any, model_type: type[ModelT]) -> ModelT:
        return msgspec.convert(payload, type=model_type)

    @staticmethod
    def _map_http_error(exc: httpx.HTTPStatusError) -> Exception:
        response = exc.response
        if response.status_code == 412:
            error = _decode_error_details(response.content)
            message = error.message if error else "Hypercolor precondition failed"
            return HypercolorPreconditionError(
                message,
                error=error,
                status_code=response.status_code,
                current_revision=_etag_revision(response),
            )
        return HypercolorClient._map_response_error(response.status_code, response.content)

    @staticmethod
    def _map_response_error(status_code: int, content: bytes) -> Exception:
        error = _decode_error_details(content)
        message = (
            error.message
            if error is not None
            else f"Hypercolor API request failed with {status_code}"
        )

        error_type: type[
            HypercolorApiError
            | HypercolorAuthenticationError
            | HypercolorConflictError
            | HypercolorConnectionError
            | HypercolorNotFoundError
            | HypercolorRateLimitError
            | HypercolorUnavailableError
            | HypercolorValidationError
        ]
        if status_code == 401:
            error_type = HypercolorAuthenticationError
        elif status_code == 404:
            error_type = HypercolorNotFoundError
        elif status_code == 409:
            error_type = HypercolorConflictError
        elif status_code == 422:
            error_type = HypercolorValidationError
        elif status_code == 429:
            error_type = HypercolorRateLimitError
        elif status_code == 503:
            error_type = HypercolorUnavailableError
        else:
            error_type = HypercolorApiError
        return _instantiate_error(error_type, message, error, status_code)


def _decode_error_details(content: bytes) -> ApiErrorDetails | None:
    try:
        payload = msgspec.json.decode(content)
    except msgspec.DecodeError:
        return None

    if not isinstance(payload, dict):
        return None
    error = payload.get("error")
    if not isinstance(error, dict):
        return None
    code = error.get("code")
    message = error.get("message")
    if not isinstance(code, str) or not isinstance(message, str):
        return None
    details = error.get("details")
    if details is not None and not isinstance(details, dict):
        details = None
    return ApiErrorDetails(code=code, message=message, details=details)


def _cover_image(response: httpx.Response, url: str) -> EffectCoverImage:
    content_type = response.headers.get("content-type", "application/octet-stream")
    content_type = content_type.split(";", maxsplit=1)[0].strip() or "application/octet-stream"
    return EffectCoverImage(data=response.content, content_type=content_type, url=url)


def _drop_none(data: Mapping[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in data.items() if value is not None}


def _generated_param(value: Any) -> Any:
    return UNSET if value is None else value


def _drop_unset_json_body(kwargs: Mapping[str, Any]) -> dict[str, Any]:
    request = dict(kwargs)
    if request.get("json") is not UNSET:
        return request

    request.pop("json")
    headers = dict(request.get("headers") or {})
    if headers.get("Content-Type") == "application/json":
        headers.pop("Content-Type")
    if headers:
        request["headers"] = headers
    else:
        request.pop("headers", None)
    return request


def _control_changes_request(
    surface_id: str,
    values: Mapping[str, Any],
    *,
    dry_run: bool,
    expected_revision: int | None,
) -> ApplyControlChangesRequest:
    body: dict[str, Any] = {
        "surface_id": surface_id,
        "changes": [
            {"field_id": str(field_id), "value": _control_api_value(value)}
            for field_id, value in values.items()
        ],
    }
    if dry_run:
        body["dry_run"] = True
    if expected_revision is not None:
        body["expected_revision"] = expected_revision
    return ApplyControlChangesRequest.from_dict(body)


def _control_api_value(value: Any) -> dict[str, Any]:
    if isinstance(value, Mapping):
        if "kind" in value:
            result = {str(key): item for key, item in value.items()}
        else:
            result = {
                "kind": "object",
                "value": {str(key): _control_api_value(item) for key, item in value.items()},
            }
    elif isinstance(value, list):
        result = {"kind": "list", "value": [_control_api_value(item) for item in value]}
    elif value is None:
        result = {"kind": "null"}
    elif isinstance(value, bool):
        result = {"kind": "bool", "value": value}
    elif isinstance(value, int):
        result = {"kind": "integer", "value": value}
    elif isinstance(value, float):
        result = {"kind": "float", "value": value}
    else:
        result = {"kind": "string", "value": str(value)}
    return result


def _display_control_value(value: Any) -> dict[str, Any]:
    if isinstance(value, Mapping):
        if set(value) & {"float", "integer", "boolean", "color", "text", "enum", "rect"}:
            result = {str(key): item for key, item in value.items()}
        else:
            result = {"text": json.dumps({str(key): item for key, item in value.items()})}
    elif isinstance(value, bool):
        result = {"boolean": value}
    elif isinstance(value, int):
        result = {"integer": value}
    elif isinstance(value, float):
        result = {"float": value}
    elif isinstance(value, str):
        result = {"color": color} if (color := _hex_color_value(value)) else {"text": value}
    else:
        result = {"text": str(value)}
    return result


def _effect_control_value(value: Any) -> dict[str, Any]:
    if isinstance(value, Mapping):
        tags = {"float", "integer", "boolean", "color", "gradient", "enum", "text", "rect"}
        if len(value) == 1 and set(value) <= tags:
            result = {str(key): item for key, item in value.items()}
        elif {"x", "y", "width", "height"} <= set(value):
            result = {"rect": {str(key): item for key, item in value.items()}}
        else:
            message = "effect control mappings must be tagged values or rectangles"
            raise ValueError(message)
    elif isinstance(value, bool):
        result = {"boolean": value}
    elif isinstance(value, int):
        result = {"integer": value}
    elif isinstance(value, float):
        result = {"float": value}
    elif isinstance(value, str):
        color = _hex_color_value(value)
        result = {"color": color} if color is not None else {"text": value}
    elif isinstance(value, list) and len(value) == 4:
        result = {"color": value}
    else:
        message = "unsupported effect control value"
        raise ValueError(message)
    return result


def _hex_color_value(value: str) -> list[float] | None:
    color = value.strip().removeprefix("#")
    if len(color) not in {6, 8} or any(ch not in "0123456789abcdefABCDEF" for ch in color):
        return None
    red = int(color[0:2], 16) / 255
    green = int(color[2:4], 16) / 255
    blue = int(color[4:6], 16) / 255
    alpha = int(color[6:8], 16) / 255 if len(color) == 8 else 1.0
    return [red, green, blue, alpha]


def _request_path(path: str) -> str:
    if path.startswith(("http://", "https://")) or path.startswith(API_PREFIX):
        return path
    return f"{API_PREFIX}{path}"


def _quote_path(value: str) -> str:
    return quote(str(value), safe="")


def _normalize_payload(value: Any) -> Any:
    if isinstance(value, list):
        return [_normalize_payload(item) for item in value]
    if not isinstance(value, dict):
        return value

    normalized = {key: _normalize_payload(item) for key, item in value.items()}
    if "field_id" in normalized and isinstance(normalized.get("value"), dict):
        normalized["value"] = _control_value(normalized["value"])
    if "field_id" in normalized and isinstance(normalized.get("attempted_value"), dict):
        normalized["attempted_value"] = _control_value(normalized["attempted_value"])
    if isinstance(normalized.get("result"), dict):
        normalized["result"] = _control_value(normalized["result"])
    if "control_type" in normalized and "name" in normalized and "default_value" in normalized:
        normalized.setdefault("label", normalized["name"])
        normalized.setdefault("type", _legacy_control_type(normalized["control_type"]))
        normalized.setdefault("default", _control_value(normalized["default_value"]))
        # Dropdown/enum choices ship under `labels`; expose them as the
        # `options` field the ControlDefinition model (and downstream cards)
        # read, so select controls carry their selectable values.
        if isinstance(normalized.get("labels"), list):
            normalized.setdefault("options", normalized["labels"])
    for key in (
        "controls",
        "control_values",
        "active_control_values",
        "applied_controls",
        "applied",
        "values",
    ):
        if isinstance(normalized.get(key), dict):
            normalized[key] = {
                str(item_key): _control_value(item_value)
                for item_key, item_value in normalized[key].items()
            }
    return normalized


def _control_value(value: Any) -> Any:
    if not isinstance(value, dict) or len(value) != 1:
        if isinstance(value, dict) and isinstance(value.get("kind"), str):
            return _normalize_payload(value.get("value"))
        return _normalize_payload(value)
    key, item = next(iter(value.items()))
    if key not in {"float", "integer", "boolean", "color", "gradient", "enum", "text", "rect"}:
        return _normalize_payload(value)
    return _normalize_payload(item)


def _legacy_control_type(control_type: Any) -> str:
    return {
        "color_picker": "color",
        "dropdown": "select",
        "gradient_editor": "gradient",
        "rect": "rect",
        "slider": "number",
        "text_input": "text",
        "toggle": "boolean",
    }.get(str(control_type), str(control_type))


def _to_json_mapping(value: TransitionSpec | Mapping[str, Any] | None) -> dict[str, Any] | None:
    if value is None:
        return None
    if isinstance(value, Mapping):
        return {str(key): item for key, item in value.items()}
    return msgspec.to_builtins(value)


def _transition_value(value: str | Mapping[str, Any] | None) -> dict[str, Any] | None:
    if value is None:
        return None
    if isinstance(value, str):
        return {"type": value}
    return {str(key): item for key, item in value.items()}


def _instantiate_error(
    error_type: type[
        HypercolorApiError
        | HypercolorAuthenticationError
        | HypercolorConflictError
        | HypercolorNotFoundError
        | HypercolorRateLimitError
        | HypercolorUnavailableError
        | HypercolorValidationError
    ],
    message: str,
    error: ApiErrorDetails | None,
    status_code: int,
) -> (
    HypercolorApiError
    | HypercolorAuthenticationError
    | HypercolorConflictError
    | HypercolorNotFoundError
    | HypercolorRateLimitError
    | HypercolorUnavailableError
    | HypercolorValidationError
):
    return error_type(message, error=error, status_code=status_code)
