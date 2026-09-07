"""Async client for the Hypercolor daemon API."""

from __future__ import annotations

import math
from collections.abc import Callable, Mapping
from datetime import timedelta
from ipaddress import ip_address
from re import fullmatch
from typing import Any, NoReturn, Self, TypeVar
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
    apply_effect as generated_apply_effect,
    apply_effect_preset as generated_apply_effect_preset,
    get_effect as generated_get_effect,
    list_effect_presets as generated_list_effect_presets,
    list_effects as generated_list_effects,
)
from ._generated.api.layouts import (
    apply_layout as generated_apply_layout,
    get_active_layout as generated_get_active_layout,
    list_layouts as generated_list_layouts,
)
from ._generated.api.scenes import (
    activate_scene as generated_activate_scene,
    assign_live_zone_members as generated_assign_live_zone_members,
    clear_scene as generated_clear_scene,
    create_live_zone as generated_create_live_zone,
    create_scene as generated_create_scene,
    deactivate_scene as generated_deactivate_scene,
    delete_live_zone as generated_delete_live_zone,
    delete_scene as generated_delete_scene,
    get_live_scene as generated_get_live_scene,
    get_live_zone as generated_get_live_zone,
    get_scene as generated_get_scene,
    list_scenes as generated_list_scenes,
    patch_live_layer_controls as generated_patch_live_layer_controls,
    patch_live_scene as generated_patch_live_scene,
    patch_live_zone as generated_patch_live_zone,
    put_live_zone_layout as generated_put_live_zone_layout,
    snapshot_scene as generated_snapshot_scene,
    unassign_live_zone_member as generated_unassign_live_zone_member,
    update_scene as generated_update_scene,
)
from ._generated.api.system import (
    get_system as generated_get_system,
    health_check as generated_health_check,
    list_audio_devices as generated_list_audio_devices,
)
from ._generated.models.activate_scene_request import ActivateSceneRequest
from ._generated.models.activate_scene_response import ActivateSceneResponse
from ._generated.models.apply_effect_request import ApplyEffectRequest
from ._generated.models.apply_effect_response import ApplyEffectResponse
from ._generated.models.assign_members_request import AssignMembersRequest
from ._generated.models.blend_mode import BlendMode
from ._generated.models.clear_scene_request import ClearSceneRequest
from ._generated.models.create_scene_request import CreateSceneRequest
from ._generated.models.create_zone_request import CreateZoneRequest
from ._generated.models.delete_scene_response import DeleteSceneResponse
from ._generated.models.discover_request import DiscoverRequest
from ._generated.models.discovery_completed_response import DiscoveryCompletedResponse
from ._generated.models.discovery_scanning_response import DiscoveryScanningResponse
from ._generated.models.effect_detail_response import EffectDetailResponse
from ._generated.models.effect_preset_summary import EffectPresetSummary
from ._generated.models.effect_preset_summary_list_response import EffectPresetSummaryListResponse
from ._generated.models.effect_summary import EffectSummary
from ._generated.models.effect_summary_list_response import EffectSummaryListResponse
from ._generated.models.get_system_response_200 import GetSystemResponse200
from ._generated.models.health_response import HealthResponse
from ._generated.models.identify_request import IdentifyRequest
from ._generated.models.invoke_control_action_request import InvokeControlActionRequest
from ._generated.models.patch_controls_request import PatchControlsRequest
from ._generated.models.patch_zone_request import PatchZoneRequest
from ._generated.models.replace_scene_request import ReplaceSceneRequest
from ._generated.models.scene_document import SceneDocument
from ._generated.models.scene_patch_request import ScenePatchRequest
from ._generated.models.scene_summary import SceneSummary
from ._generated.models.scene_summary_list_response import SceneSummaryListResponse
from ._generated.models.snapshot_scene_request import SnapshotSceneRequest
from ._generated.models.system_status import SystemStatus
from ._generated.models.update_device_request import UpdateDeviceRequest
from ._generated.models.zone_layout_request import ZoneLayoutRequest
from ._generated.models.zone_resource import ZoneResource
from ._generated.types import UNSET
from ._model_validation import validate_generated_model
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
from .models import (
    ActivatePlaylistResponse,
    AddFavoriteResponse,
    ApplyControlChangesResponse,
    ApplyLayoutResponse,
    AudioDevicesResponse,
    ConfigMutationResponse,
    ControlActionResult,
    ControlSurfaceDocument,
    DeleteFavoriteResponse,
    DeletePresetResponse,
    DeviceSummary,
    DiagnoseResponse,
    DisplayFaceResponse,
    DisplaySummaryListItem,
    DriverSummary,
    EffectCoverImage,
    EffectPlaylist,
    EffectPreset,
    FavoriteSummary,
    IdentifyDeviceResponse,
    LayoutSummary,
    OutputResource,
    SpatialLayout,
)
from .websocket import HypercolorEventStream

ModelT = TypeVar("ModelT")
DiscoverResponse = DiscoveryCompletedResponse | DiscoveryScanningResponse

#: Page size requested from every list route. The daemon defaults to 50 and
#: rejects anything above 200, so asking for the ceiling keeps the number of
#: round trips down without tripping validation.
LIST_PAGE_LIMIT = 200


class _Unset:
    """Marker type distinguishing "leave unchanged" from an explicit ``None``."""

    __slots__ = ()


_UNSET_SENTINEL = _Unset()


def _with_if_match(kwargs: dict[str, Any], revision: int | None) -> dict[str, Any]:
    if revision is None:
        return kwargs
    kwargs["headers"] = {
        **dict(kwargs.get("headers") or {}),
        "If-Match": f'"{revision}"',
    }
    return kwargs


def _etag_revision(response: httpx.Response) -> int | None:
    etag = response.headers.get("etag")
    if etag is None:
        return None
    try:
        return int(etag.strip().strip('"'))
    except ValueError:
        return None


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

    async def health(self) -> HealthResponse:
        """Run the daemon health check."""
        payload = await self._generated_payload(
            generated_health_check._get_kwargs(), envelope=False
        )
        try:
            health = HealthResponse.from_dict(_mapping(payload))
            validate_generated_model(health)
        except (KeyError, TypeError, ValueError, AttributeError) as error:
            raise HypercolorApiError("Malformed Hypercolor health response") from error
        return health

    async def get_status(self) -> SystemStatus:
        """Return the current daemon status snapshot."""
        payload = await self._generated_request(generated_get_system._get_kwargs())
        self._unwrap_data(payload)
        try:
            system = GetSystemResponse200.from_dict(_mapping(payload)).data
        except (KeyError, TypeError, ValueError, AttributeError) as error:
            raise HypercolorApiError("Malformed Hypercolor system resource") from error
        if system.status is None or system.status is UNSET:
            raise HypercolorAuthenticationError("System status requires daemon read access")
        if not isinstance(system.status, SystemStatus):
            raise HypercolorApiError("Malformed Hypercolor system status")
        try:
            validate_generated_model(system.status)
        except TypeError as error:
            raise HypercolorApiError("Malformed Hypercolor system status") from error
        return system.status

    async def get_output(self) -> OutputResource:
        """Return global output power and brightness."""

        return await self._request_model("GET", "/output", OutputResource.from_dict)

    async def set_output(
        self,
        *,
        power: str | None = None,
        brightness: float | None = None,
    ) -> OutputResource:
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
        return await self._request_model("PATCH", "/output", OutputResource.from_dict, body=body)

    async def get_brightness(self) -> float:
        """Return the global daemon brightness as a `0.0` to `1.0` float."""

        return (await self.get_output()).brightness

    async def set_brightness(self, brightness: float) -> OutputResource:
        """Set the global daemon brightness as a `0.0` to `1.0` float."""

        return await self.set_output(brightness=brightness)

    async def set_output_power(self, *, paused: bool) -> OutputResource:
        """Set global output power without discarding live scene state."""

        return await self.set_output(power="paused" if paused else "running")

    async def pause_rendering(self) -> OutputResource:
        """Pause all output while preserving live scene state."""

        return await self.set_output_power(paused=True)

    async def resume_rendering(self) -> OutputResource:
        """Resume output from the preserved live scene state."""

        return await self.set_output_power(paused=False)

    async def get_devices(
        self,
        *,
        offset: int | None = None,
        limit: int | None = None,
        status: str | None = None,
        backend_id: str | None = None,
        driver: str | None = None,
        q: str | None = None,
        include: str | None = None,
    ) -> list[DeviceSummary]:
        """List devices.

        With neither ``offset`` nor ``limit`` set, this follows
        ``page.has_more`` until the daemon reports the listing complete.
        Passing either one requests exactly that page and returns it.
        """

        def page_kwargs(page_offset: int, page_limit: int) -> Mapping[str, Any]:
            return generated_list_devices._get_kwargs(
                offset=page_offset,
                limit=page_limit,
                status=_generated_param(status),
                backend_id=_generated_param(backend_id),
                driver=_generated_param(driver),
                q=_generated_param(q),
                include=_generated_param(include),
            )

        if offset is not None or limit is not None:
            return await self._generated_items(
                page_kwargs(offset or 0, limit or LIST_PAGE_LIMIT),
                DeviceSummary.from_dict,
            )
        return await self._all_pages(page_kwargs, DeviceSummary.from_dict)

    async def get_device(self, device_id: str) -> DeviceSummary:
        """Fetch a single device."""
        return await self._generated_model(
            generated_get_device._get_kwargs(device_id),
            DeviceSummary.from_dict,
        )

    async def update_device(self, device_id: str, **fields: Any) -> DeviceSummary:
        """Update device configuration."""
        return await self._generated_model(
            generated_update_device._get_kwargs(
                device_id,
                body=UpdateDeviceRequest.from_dict(fields),
            ),
            DeviceSummary.from_dict,
        )

    async def discover_devices(
        self,
        targets: list[str] | None = None,
        timeout_ms: int | None = None,
        *,
        wait: bool | None = None,
    ) -> DiscoverResponse:
        """Trigger a device discovery scan.

        ``targets`` selects which discovery targets to scan; omitting it
        scans every enabled target. ``wait`` blocks until the scan
        finishes, so the daemon answers with a
        :class:`DiscoveryCompletedResponse` carrying the full scan result
        instead of a :class:`DiscoveryScanningResponse` acknowledgement.
        """
        body = _drop_none({"targets": targets, "timeout_ms": timeout_ms, "wait": wait})
        kwargs = (
            generated_discover_devices._get_kwargs(body=DiscoverRequest.from_dict(body))
            if body
            else generated_discover_devices._get_kwargs()
        )
        payload = await self._generated_payload(kwargs)
        return _discover_response(_mapping(payload))

    async def identify_device(
        self,
        device_id: str,
        *,
        duration_ms: int | None = None,
        color: str | None = None,
    ) -> IdentifyDeviceResponse:
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
            IdentifyDeviceResponse.from_dict,
        )

    async def get_drivers(self) -> list[DriverSummary]:
        """List registered driver modules."""
        return await self._generated_items(
            generated_list_drivers._get_kwargs(),
            DriverSummary.from_dict,
        )

    async def get_effects(
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
        """List available effects."""
        payload = await self._generated_payload(
            generated_list_effects._get_kwargs(
                category=_generated_param(category),
                audio_reactive=_generated_param(audio_reactive),
                screen_reactive=_generated_param(screen_reactive),
                input_reactive=_generated_param(input_reactive),
                source=_generated_param(source),
                q=_generated_param(q),
                include=_generated_param(include),
            )
        )
        return EffectSummaryListResponse.from_dict(_mapping(payload)).items

    async def get_effect(self, effect_id: str) -> EffectDetailResponse:
        """Fetch a single effect with controls."""
        return await self._generated_contract(
            generated_get_effect._get_kwargs(effect_id),
            EffectDetailResponse.from_dict,
        )

    async def get_effect_presets(self, effect_id: str) -> list[EffectPresetSummary]:
        """List bundled and saved presets for one effect."""
        payload = await self._generated_payload(
            generated_list_effect_presets._get_kwargs(effect_id)
        )
        return EffectPresetSummaryListResponse.from_dict(_mapping(payload)).items

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
        data = _drop_none(
            {
                "controls": (
                    {
                        str(name): _canonical_control_value(value)
                        for name, value in controls.items()
                    }
                    if controls is not None
                    else None
                ),
                "transition": _transition_value(transition),
                "preset_id": preset_id,
                "zone": zone,
            }
        )
        kwargs = generated_apply_effect._get_kwargs(
            effect_id,
            **({"body": ApplyEffectRequest.from_dict(data)} if data else {}),
        )
        return await self._generated_contract(
            _with_if_match(kwargs, if_match),
            ApplyEffectResponse.from_dict,
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
        data = _drop_none(
            {
                "controls": (
                    {
                        str(name): _canonical_control_value(value)
                        for name, value in controls.items()
                    }
                    if controls is not None
                    else None
                ),
                "transition": _transition_value(transition),
                "zone": zone,
            }
        )
        kwargs = generated_apply_effect_preset._get_kwargs(
            effect_id,
            preset_id,
            **({"body": ApplyEffectRequest.from_dict(data)} if data else {}),
        )
        return await self._generated_contract(
            _with_if_match(kwargs, if_match),
            ApplyEffectResponse.from_dict,
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
    ) -> ZoneResource:
        """Patch values on one live scene layer."""
        body: dict[str, Any] = {
            "values": {
                str(name): _canonical_control_value(value) for name, value in values.items()
            }
        }
        if clear_bindings:
            body["clear_bindings"] = clear_bindings
        return await self._generated_contract(
            generated_patch_live_layer_controls._get_kwargs(
                zone,
                layer,
                body=PatchControlsRequest.from_dict(body),
            ),
            ZoneResource.from_dict,
        )

    async def get_control_surfaces(
        self,
        *,
        device_id: str | None = None,
        driver_id: str | None = None,
        include_driver: bool = False,
    ) -> list[ControlSurfaceDocument]:
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
            self._decode(surface, ControlSurfaceDocument.from_dict)
            for surface in surfaces
            if isinstance(surface, Mapping)
        ]

    async def get_device_controls(self, device_id: str) -> ControlSurfaceDocument:
        """Return a device control surface."""
        return await self._generated_model(
            generated_get_device_control_surface._get_kwargs(device_id),
            ControlSurfaceDocument.from_dict,
        )

    async def get_driver_controls(self, driver_id: str) -> ControlSurfaceDocument:
        """Return a driver control surface."""
        return await self._generated_model(
            generated_get_driver_control_surface._get_kwargs(driver_id),
            ControlSurfaceDocument.from_dict,
        )

    async def set_control_values(
        self,
        surface_id: str,
        values: Mapping[str, Any],
    ) -> ApplyControlChangesResponse:
        """Apply one or more control values to a control surface."""
        body = _patch_controls_request(values)
        return await self._generated_model(
            generated_apply_control_surface_values._get_kwargs(
                surface_id,
                body=body,
            ),
            ApplyControlChangesResponse.from_dict,
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
            body["input"] = {
                str(key): _canonical_control_value(value) for key, value in input.items()
            }
        return await self._generated_model(
            generated_invoke_control_surface_action._get_kwargs(
                surface_id,
                action_id,
                body=body,
            ),
            ControlActionResult.from_dict,
        )

    async def get_layouts(self) -> list[LayoutSummary]:
        """List layouts, following `page.has_more` until the listing ends."""

        def page_kwargs(page_offset: int, page_limit: int) -> Mapping[str, Any]:
            return generated_list_layouts._get_kwargs(
                offset=page_offset,
                limit=page_limit,
            )

        return await self._all_pages(page_kwargs, LayoutSummary.from_dict)

    async def get_active_layout(self) -> SpatialLayout | None:
        """Return the active layout if one exists."""
        try:
            return await self._generated_model(
                generated_get_active_layout._get_kwargs(),
                SpatialLayout.from_dict,
            )
        except HypercolorNotFoundError:
            return None

    async def apply_layout(self, layout_id: str) -> ApplyLayoutResponse:
        """Apply a layout."""
        return await self._generated_model(
            generated_apply_layout._get_kwargs(layout_id),
            ApplyLayoutResponse.from_dict,
        )

    async def get_scenes(self) -> list[SceneSummary]:
        """List available scenes."""
        payload = await self._generated_payload(generated_list_scenes._get_kwargs())
        return SceneSummaryListResponse.from_dict(_mapping(payload)).items

    async def get_scene(self, scene_id: str) -> SceneDocument:
        """Fetch a complete stored scene document."""
        return await self._generated_contract(
            generated_get_scene._get_kwargs(scene_id),
            SceneDocument.from_dict,
        )

    async def get_live_scene(self) -> SceneDocument:
        """Return the full live scene tree."""
        return await self._generated_contract(
            generated_get_live_scene._get_kwargs(),
            SceneDocument.from_dict,
        )

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
        body = ScenePatchRequest.from_dict(
            _drop_none({"name": name, "unassigned_behavior": behavior})
        )
        return await self._generated_contract(
            _with_if_match(generated_patch_live_scene._get_kwargs(body=body), if_match),
            SceneDocument.from_dict,
        )

    async def deactivate_scene(self) -> SceneDocument:
        """Return to the auto-managed default scene."""
        return await self._generated_contract(
            generated_deactivate_scene._get_kwargs(),
            SceneDocument.from_dict,
        )

    async def clear_scene(
        self,
        *,
        zone: str | None = None,
        if_match: int | None = None,
    ) -> SceneDocument:
        """Clear one zone's layer stack, or every non-display zone."""
        data = _drop_none({"zone": zone})
        kwargs = generated_clear_scene._get_kwargs(
            **({"body": ClearSceneRequest.from_dict(data)} if data else {})
        )
        return await self._generated_contract(
            _with_if_match(kwargs, if_match),
            SceneDocument.from_dict,
        )

    async def create_scene(
        self,
        name: str,
        *,
        description: str | None = None,
        enabled: bool | None = None,
        mutation_mode: str | None = None,
    ) -> SceneSummary:
        """Create a scene."""
        body = _drop_none(
            {
                "name": name,
                "description": description,
                "enabled": enabled,
                "mutation_mode": mutation_mode,
            }
        )
        return await self._generated_contract(
            generated_create_scene._get_kwargs(body=CreateSceneRequest.from_dict(body)),
            SceneSummary.from_dict,
        )

    async def snapshot_scene(
        self,
        name: str,
        *,
        description: str | None = None,
    ) -> SceneSummary:
        """Save the current runtime scene as a snapshot-locked scene."""
        body = _drop_none({"name": name, "description": description})
        return await self._generated_contract(
            generated_snapshot_scene._get_kwargs(body=SnapshotSceneRequest.from_dict(body)),
            SceneSummary.from_dict,
        )

    async def activate_scene(
        self,
        scene_id: str,
        *,
        transition_ms: int | None = None,
    ) -> ActivateSceneResponse:
        """Trigger a scene manually."""
        body = ActivateSceneRequest.from_dict(_drop_none({"transition_ms": transition_ms}))
        return await self._generated_contract(
            generated_activate_scene._get_kwargs(scene_id, body=body),
            ActivateSceneResponse.from_dict,
        )

    async def update_scene(
        self,
        scene_id: str,
        document: SceneDocument | ReplaceSceneRequest,
        *,
        if_match: int | None = None,
    ) -> SceneDocument:
        """Replace a complete stored scene document."""
        replacement = (
            ReplaceSceneRequest.from_dict(document.to_dict())
            if isinstance(document, SceneDocument)
            else document
        )
        return await self._generated_contract(
            _with_if_match(
                generated_update_scene._get_kwargs(scene_id, body=replacement),
                if_match,
            ),
            SceneDocument.from_dict,
        )

    async def delete_scene(self, scene_id: str) -> DeleteSceneResponse:
        """Delete a scene."""
        return await self._generated_contract(
            generated_delete_scene._get_kwargs(scene_id),
            DeleteSceneResponse.from_dict,
        )

    async def get_zone(self, zone: str) -> ZoneResource:
        """Fetch one zone from the live scene tree."""
        return await self._generated_contract(
            generated_get_live_zone._get_kwargs(zone),
            ZoneResource.from_dict,
        )

    async def create_zone(
        self,
        name: str,
        *,
        role: str | None = None,
        color: str | None = None,
        if_match: int | None = None,
    ) -> ZoneResource:
        """Create a zone in the live scene tree."""
        body = _drop_none({"name": name, "role": role, "color": color})
        return await self._generated_contract(
            _with_if_match(
                generated_create_live_zone._get_kwargs(body=CreateZoneRequest.from_dict(body)),
                if_match,
            ),
            ZoneResource.from_dict,
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
    ) -> ZoneResource:
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
        return await self._generated_contract(
            _with_if_match(
                generated_patch_live_zone._get_kwargs(
                    zone,
                    body=PatchZoneRequest.from_dict(body),
                ),
                if_match,
            ),
            ZoneResource.from_dict,
        )

    async def delete_zone(
        self,
        zone: str,
        *,
        if_match: int | None = None,
    ) -> SceneDocument:
        """Delete one zone from the live scene tree."""
        return await self._generated_contract(
            _with_if_match(generated_delete_live_zone._get_kwargs(zone), if_match),
            SceneDocument.from_dict,
        )

    async def assign_members(
        self,
        zone: str,
        device_id: str,
        *,
        segments: list[str] | None = None,
        if_match: int | None = None,
    ) -> ZoneResource:
        """Assign a device and selected segments to one live zone."""
        body = AssignMembersRequest.from_dict({"device_id": device_id, "segments": segments or []})
        return await self._generated_contract(
            _with_if_match(
                generated_assign_live_zone_members._get_kwargs(zone, body=body),
                if_match,
            ),
            ZoneResource.from_dict,
        )

    async def unassign_member(
        self,
        zone: str,
        member: str,
        *,
        if_match: int | None = None,
    ) -> ZoneResource:
        """Remove one membership from a live zone."""
        return await self._generated_contract(
            _with_if_match(
                generated_unassign_live_zone_member._get_kwargs(zone, member),
                if_match,
            ),
            ZoneResource.from_dict,
        )

    async def set_zone_layout(
        self,
        zone: str,
        layout: Mapping[str, Any],
        *,
        if_match: int | None = None,
    ) -> ZoneResource:
        """Replace a live zone's compact member-placement layout."""
        body = ZoneLayoutRequest.from_dict({str(key): value for key, value in layout.items()})
        return await self._generated_contract(
            _with_if_match(
                generated_put_live_zone_layout._get_kwargs(zone, body=body),
                if_match,
            ),
            ZoneResource.from_dict,
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

    async def get_favorites(self) -> list[FavoriteSummary]:
        """List favorite effects."""
        return await self._request_items("GET", "/library/favorites", FavoriteSummary.from_dict)

    async def add_favorite(self, effect_id: str) -> AddFavoriteResponse:
        """Add or update a favorite effect."""
        return await self._request_model(
            "POST",
            "/library/favorites",
            AddFavoriteResponse.from_dict,
            body={"effect": effect_id},
        )

    async def remove_favorite(self, effect_id: str) -> DeleteFavoriteResponse:
        """Remove a favorite effect."""
        return await self._request_model(
            "DELETE",
            f"/library/favorites/{_quote_path(effect_id)}",
            DeleteFavoriteResponse.from_dict,
        )

    async def get_presets(self) -> list[EffectPreset]:
        """List saved presets."""
        return await self._request_items("GET", "/library/presets", EffectPreset.from_dict)

    async def get_preset(self, preset_id: str) -> EffectPreset:
        """Fetch a saved preset."""
        return await self._request_model(
            "GET",
            f"/library/presets/{_quote_path(preset_id)}",
            EffectPreset.from_dict,
        )

    async def save_preset(
        self,
        name: str,
        effect_id: str,
        *,
        description: str | None = None,
        controls: Mapping[str, Any] | None = None,
        tags: list[str] | None = None,
    ) -> EffectPreset:
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
        return await self._request_model(
            "POST", "/library/presets", EffectPreset.from_dict, body=body
        )

    async def delete_preset(self, preset_id: str) -> DeletePresetResponse:
        """Delete a saved preset."""
        return await self._request_model(
            "DELETE",
            f"/library/presets/{_quote_path(preset_id)}",
            DeletePresetResponse.from_dict,
        )

    async def get_playlists(self) -> list[EffectPlaylist]:
        """List saved playlists."""
        return await self._request_items("GET", "/library/playlists", EffectPlaylist.from_dict)

    async def get_playlist(self, playlist_id: str) -> EffectPlaylist:
        """Fetch a saved playlist."""
        return await self._request_model(
            "GET",
            f"/library/playlists/{_quote_path(playlist_id)}",
            EffectPlaylist.from_dict,
        )

    async def activate_playlist(self, playlist_id: str) -> ActivatePlaylistResponse:
        """Start playlist playback."""
        return await self._request_model(
            "POST",
            f"/library/playlists/{_quote_path(playlist_id)}/activate",
            ActivatePlaylistResponse.from_dict,
        )

    async def list_displays(self) -> list[DisplaySummaryListItem]:
        """List devices that expose display faces."""
        return await self._request_list("GET", "/displays", DisplaySummaryListItem.from_dict)

    async def set_display_face(
        self,
        display_id: str,
        effect_id: str,
        *,
        controls: Mapping[str, Any] | None = None,
        blend_mode: BlendMode | str | None = None,
        opacity: float | None = None,
    ) -> DisplayFaceResponse:
        """Assign an effect to a display face."""
        body = _drop_none(
            {
                "effect_id": effect_id,
                "controls": (
                    {str(key): _canonical_control_value(value) for key, value in controls.items()}
                    if controls is not None
                    else None
                ),
                "blend_mode": blend_mode.value
                if isinstance(blend_mode, BlendMode)
                else blend_mode,
                "opacity": opacity,
            }
        )
        return await self._request_model(
            "PUT",
            f"/displays/{_quote_path(display_id)}/face",
            DisplayFaceResponse.from_dict,
            body=body,
        )

    async def run_diagnostics(
        self,
        *,
        checks: list[str] | None = None,
        system: bool | None = None,
    ) -> DiagnoseResponse:
        """Run daemon diagnostics."""
        body = _drop_none({"checks": checks, "system": system})
        return await self._request_model(
            "POST", "/diagnose", DiagnoseResponse.from_dict, body=body
        )

    async def get_audio_spectrum(self) -> NoReturn:
        """Raise: spectrum snapshots only exist on the WebSocket stream."""

        message = (
            "Audio spectrum snapshots are only available over the Hypercolor WebSocket stream"
        )
        raise HypercolorNotFoundError(message, status_code=404)

    async def get_audio_devices(self) -> AudioDevicesResponse:
        """Return the available audio capture devices."""

        return await self._generated_model(
            generated_list_audio_devices._get_kwargs(),
            AudioDevicesResponse.from_dict,
        )

    async def _generated_model(
        self,
        kwargs: Mapping[str, Any],
        decoder: Callable[[Mapping[str, Any]], ModelT],
        *,
        envelope: bool = True,
    ) -> ModelT:
        payload = await self._generated_payload(kwargs, envelope=envelope)
        return self._decode(payload, decoder)

    async def _all_pages(
        self,
        page_kwargs: Callable[[int, int], Mapping[str, Any]],
        decoder: Callable[[Mapping[str, Any]], ModelT],
    ) -> list[ModelT]:
        items: list[ModelT] = []
        offset = 0
        while True:
            data = await self._generated_payload(page_kwargs(offset, LIST_PAGE_LIMIT))
            page = data if isinstance(data, dict) else {}
            raw_items = page.get("items")
            fetched: list[Any] = raw_items if isinstance(raw_items, list) else []
            items.extend(self._decode(item, decoder) for item in fetched)
            paging = page.get("page")
            has_more = isinstance(paging, dict) and bool(paging.get("has_more"))
            if not has_more or not fetched:
                return items
            offset += len(fetched)

    async def _generated_items(
        self,
        kwargs: Mapping[str, Any],
        decoder: Callable[[Mapping[str, Any]], ModelT],
    ) -> list[ModelT]:
        data = await self._generated_payload(kwargs)
        items = data["items"] if isinstance(data, dict) else []
        return [self._decode(item, decoder) for item in items]

    async def _generated_contract(
        self,
        kwargs: Mapping[str, Any],
        decoder: Callable[[Mapping[str, Any]], ModelT],
    ) -> ModelT:
        payload = await self._generated_payload(kwargs)
        return decoder(_mapping(payload))

    async def _generated_payload(
        self,
        kwargs: Mapping[str, Any],
        *,
        envelope: bool = True,
    ) -> Any:
        response = await self._generated_request(kwargs)
        return self._unwrap_data(response) if envelope else response

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
    ) -> ConfigMutationResponse:
        """Persist the selected audio input device.

        The config key resource takes the value as the request body, and
        ``live`` decides whether the daemon re-applies it to the running
        audio pipeline.
        """

        return await self._request_model(
            "PUT",
            "/config/keys/audio.device",
            ConfigMutationResponse.from_dict,
            body=device_id,
            params={"live": live},
        )

    async def _request_items(
        self,
        method: str,
        path: str,
        decoder: Callable[[Mapping[str, Any]], ModelT],
        *,
        body: Mapping[str, Any] | None = None,
        params: Mapping[str, Any] | None = None,
    ) -> list[ModelT]:
        response = await self._raw_request(method, path, body=body, params=params)
        data = self._unwrap_data(response)
        items = data["items"] if isinstance(data, dict) else []
        return [self._decode(item, decoder) for item in items]

    async def _request_list(
        self,
        method: str,
        path: str,
        decoder: Callable[[Mapping[str, Any]], ModelT],
        *,
        body: Mapping[str, Any] | None = None,
        params: Mapping[str, Any] | None = None,
    ) -> list[ModelT]:
        response = await self._raw_request(method, path, body=body, params=params)
        data = self._unwrap_data(response)
        if not isinstance(data, list):
            message = "Unexpected Hypercolor list response"
            raise HypercolorApiError(message)
        return [self._decode(item, decoder) for item in data]

    async def _request_model(
        self,
        method: str,
        path: str,
        decoder: Callable[[Mapping[str, Any]], ModelT],
        *,
        body: Any = None,
        params: Mapping[str, Any] | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> ModelT:
        payload = await self._request_payload(
            method, path, body=body, params=params, headers=headers
        )
        return self._decode(payload, decoder)

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
        if not isinstance(response, dict) or set(response) != {"data", "meta"}:
            message = "Unexpected Hypercolor response envelope"
            raise HypercolorApiError(message)
        meta = response["meta"]
        required_meta = {"api_version", "request_id", "timestamp"}
        if (
            not isinstance(meta, dict)
            or set(meta) != required_meta
            or any(not isinstance(meta[field], str) for field in required_meta)
        ):
            message = "Unexpected Hypercolor response metadata"
            raise HypercolorApiError(message)
        return response["data"]

    @staticmethod
    def _decode(payload: Any, decoder: Callable[[Mapping[str, Any]], ModelT]) -> ModelT:
        try:
            return decoder(_mapping(payload))
        except (KeyError, TypeError, ValueError, AttributeError) as error:
            message = "Malformed Hypercolor resource payload"
            raise HypercolorApiError(message) from error

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


def _discover_response(payload: Mapping[str, Any]) -> DiscoverResponse:
    status = payload.get("status")
    if status == "completed":
        return DiscoveryCompletedResponse.from_dict(payload)
    if status == "scanning":
        return DiscoveryScanningResponse.from_dict(payload)
    message = "Unexpected Hypercolor discovery response status"
    raise HypercolorApiError(message)


def _cover_image(response: httpx.Response, url: str) -> EffectCoverImage:
    content_type = response.headers.get("content-type", "application/octet-stream")
    content_type = content_type.split(";", maxsplit=1)[0].strip() or "application/octet-stream"
    return EffectCoverImage(data=response.content, content_type=content_type, url=url)


def _drop_none(data: Mapping[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in data.items() if value is not None}


def _mapping(value: Any) -> Mapping[str, Any]:
    if isinstance(value, Mapping):
        return value
    message = "Unexpected Hypercolor resource payload"
    raise HypercolorApiError(message)


def _generated_param(value: Any) -> Any:
    return UNSET if value is None else value


def _drop_unset_json_body(kwargs: Mapping[str, Any]) -> dict[str, Any]:
    request = dict(kwargs)
    if "json" in request and request["json"] is not UNSET:
        return request

    request.pop("json", None)
    headers = dict(request.get("headers") or {})
    if headers.get("Content-Type") == "application/json":
        headers.pop("Content-Type")
    if headers:
        request["headers"] = headers
    else:
        request.pop("headers", None)
    return request


def _patch_controls_request(values: Mapping[str, Any]) -> PatchControlsRequest:
    body = {
        "values": {
            str(field_id): _canonical_control_value(value) for field_id, value in values.items()
        }
    }
    return PatchControlsRequest.from_dict(body)


def _canonical_control_value(value: Any) -> dict[str, Any]:
    if isinstance(value, Mapping):
        if _is_canonical_control_envelope(value):
            return _canonical_tagged_control_value(value)
        if set(value) == {"x", "y", "width", "height"}:
            return {"kind": "rect", "value": _canonical_rect_payload(value)}
        return {
            "kind": "map",
            "value": {str(key): _canonical_control_value(item) for key, item in value.items()},
        }
    return _canonical_non_mapping_control_value(value)


_CANONICAL_UNIT_KINDS = {"null", "unknown"}
_CANONICAL_VALUE_KINDS = {
    "bool",
    "int",
    "float",
    "text",
    "secret_ref",
    "ip",
    "mac",
    "duration",
    "color_rgb",
    "color_rgba",
    "color_linear",
    "gradient",
    "rect",
    "enum",
    "flags",
    "list",
    "map",
}


def _is_canonical_control_envelope(value: Mapping[Any, Any]) -> bool:
    if "kind" not in value:
        return False
    kind = value["kind"]
    return set(value) <= {"kind", "value"} or (
        isinstance(kind, str) and kind in _CANONICAL_UNIT_KINDS | _CANONICAL_VALUE_KINDS
    )


def _canonical_non_mapping_control_value(value: Any) -> dict[str, Any]:
    if value is None:
        result = {"kind": "null"}
    elif isinstance(value, timedelta):
        microseconds = (value.days * 24 * 60 * 60 + value.seconds) * 1_000_000 + value.microseconds
        if microseconds < 0 or microseconds % 1_000 != 0:
            message = "control durations must be non-negative whole milliseconds"
            raise ValueError(message)
        result = {"kind": "duration", "value": microseconds // 1_000}
    elif isinstance(value, bool):
        result = {"kind": "bool", "value": value}
    elif isinstance(value, int):
        _require_integer(value, "int", -(2**63), 2**63 - 1)
        result = {"kind": "int", "value": value}
    elif isinstance(value, float):
        _require_finite_number(value, "float")
        result = {"kind": "float", "value": value}
    elif isinstance(value, str):
        color = _hex_color_value(value)
        result = (
            {
                "kind": "color_linear",
                "value": dict(zip(("r", "g", "b", "a"), color, strict=True)),
            }
            if color is not None
            else {"kind": "text", "value": value}
        )
    elif isinstance(value, list):
        result = {"kind": "list", "value": [_canonical_control_value(item) for item in value]}
    else:
        message = "unsupported control value"
        raise ValueError(message)
    return result


def _canonical_tagged_control_value(value: Mapping[Any, Any]) -> dict[str, Any]:
    kind = value.get("kind")
    if not isinstance(kind, str):
        message = "control value kind must be text"
        raise TypeError(message)

    if kind not in _CANONICAL_UNIT_KINDS | _CANONICAL_VALUE_KINDS:
        message = f"unknown control value kind: {kind}"
        raise ValueError(message)
    expected_keys = {"kind"} if kind in _CANONICAL_UNIT_KINDS else {"kind", "value"}
    _require_exact_keys(value, expected_keys, kind)
    if kind in _CANONICAL_UNIT_KINDS:
        return {"kind": kind}

    return {"kind": kind, "value": _canonical_tagged_payload(kind, value["value"])}


def _canonical_tagged_payload(kind: str, payload: Any) -> Any:
    if kind in {"bool", "int", "float", "text", "secret_ref", "ip", "mac", "duration", "enum"}:
        canonical = _canonical_scalar_payload(kind, payload)
    elif kind in {"color_rgb", "color_rgba", "color_linear"}:
        canonical = _canonical_color_payload(kind, payload)
    elif kind == "gradient":
        canonical = _canonical_gradient_payload(payload)
    elif kind == "rect":
        canonical = _canonical_rect_payload(payload)
    elif kind == "flags":
        if not isinstance(payload, list) or not all(isinstance(item, str) for item in payload):
            message = "flags control value must contain a list of text values"
            raise TypeError(message)
        canonical = list(payload)
    elif kind == "list":
        if not isinstance(payload, list):
            message = "list control value must contain a list"
            raise TypeError(message)
        canonical = [_canonical_control_value(item) for item in payload]
    elif kind == "map":
        if not isinstance(payload, Mapping) or not all(isinstance(key, str) for key in payload):
            message = "map control value must contain a string-keyed map"
            raise TypeError(message)
        canonical = {key: _canonical_control_value(item) for key, item in payload.items()}
    else:
        message = f"unknown control value kind: {kind}"
        raise ValueError(message)
    return canonical


def _canonical_scalar_payload(kind: str, payload: Any) -> bool | int | float | str:
    if kind == "bool":
        if not isinstance(payload, bool):
            message = "bool control value must contain a boolean"
            raise TypeError(message)
        canonical = payload
    elif kind == "int":
        _require_integer(payload, kind, -(2**63), 2**63 - 1)
        canonical = payload
    elif kind == "float":
        canonical = _require_finite_number(payload, kind)
    elif kind in {"text", "secret_ref", "enum"}:
        _require_text(payload, kind)
        canonical = payload
    elif kind == "ip":
        _require_text(payload, kind)
        try:
            ip_address(payload)
        except ValueError as error:
            message = "ip control value must contain an IP address"
            raise ValueError(message) from error
        canonical = payload
    elif kind == "mac":
        _require_text(payload, kind)
        mac_pattern = (
            r"(?:[0-9A-Fa-f]{12}|(?:[0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}|"
            r"(?:[0-9A-Fa-f]{2}-){5}[0-9A-Fa-f]{2}|"
            r"[0-9A-Fa-f]{4}(?:\.[0-9A-Fa-f]{4}){2})"
        )
        if fullmatch(mac_pattern, payload) is None:
            message = "mac control value must contain six hexadecimal octets"
            raise ValueError(message)
        canonical = payload
    elif kind == "duration":
        _require_integer(payload, kind, 0, 2**64 - 1)
        canonical = payload
    else:
        message = f"unknown scalar control value kind: {kind}"
        raise ValueError(message)
    return canonical


def _require_exact_keys(value: Mapping[Any, Any], expected: set[str], kind: str) -> None:
    if set(value) != expected:
        message = f"{kind} control value must contain exactly {sorted(expected)}"
        raise ValueError(message)


def _require_text(value: Any, kind: str) -> None:
    if not isinstance(value, str):
        message = f"{kind} control value must contain text"
        raise TypeError(message)


def _require_integer(value: Any, kind: str, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        message = f"{kind} control value must contain an integer"
        raise TypeError(message)
    if not minimum <= value <= maximum:
        message = f"{kind} control value integer is outside the canonical range"
        raise ValueError(message)
    return value


def _require_finite_number(value: Any, kind: str) -> float:
    if isinstance(value, bool) or not isinstance(value, int | float):
        message = f"{kind} control value must contain a number"
        raise TypeError(message)
    number = float(value)
    if not math.isfinite(number):
        message = f"{kind} control value must contain a finite number"
        raise ValueError(message)
    return number


def _canonical_color_payload(kind: str, payload: Any) -> dict[str, int | float]:
    channels = {
        "color_rgb": ("r", "g", "b"),
        "color_rgba": ("r", "g", "b", "a"),
        "color_linear": ("r", "g", "b", "a"),
    }[kind]
    if isinstance(payload, list) and len(payload) == len(channels):
        payload = dict(zip(channels, payload, strict=True))
    if not isinstance(payload, Mapping):
        message = f"{kind} control value must contain color channels"
        raise TypeError(message)
    _require_exact_keys(payload, set(channels), kind)
    canonical: dict[str, int | float] = {}
    for channel in channels:
        component = payload[channel]
        if kind == "color_linear":
            canonical[channel] = _require_finite_number(component, kind)
        else:
            canonical[channel] = _require_integer(component, kind, 0, 255)
    return canonical


def _canonical_gradient_payload(payload: Any) -> list[dict[str, Any]]:
    if not isinstance(payload, list) or not 2 <= len(payload) <= 8:
        message = "gradient control value must contain two to eight stops"
        raise ValueError(message)
    canonical = []
    previous_position = -math.inf
    for stop in payload:
        if not isinstance(stop, Mapping):
            message = "gradient stop must be an object"
            raise TypeError(message)
        _require_exact_keys(stop, {"position", "color"}, "gradient stop")
        position = _require_finite_number(stop["position"], "gradient position")
        if not 0.0 <= position <= 1.0 or position < previous_position:
            message = "gradient positions must be ordered within 0.0 through 1.0"
            raise ValueError(message)
        color = stop["color"]
        if not isinstance(color, list) or len(color) != 4:
            message = "gradient stop color must contain four channels"
            raise TypeError(message)
        canonical_color = [_require_finite_number(channel, "gradient color") for channel in color]
        if any(not 0.0 <= channel <= 1.0 for channel in canonical_color):
            message = "gradient color channels must be within 0.0 through 1.0"
            raise ValueError(message)
        canonical.append({"position": position, "color": canonical_color})
        previous_position = position
    return canonical


def _canonical_rect_payload(payload: Any) -> dict[str, float]:
    if not isinstance(payload, Mapping):
        message = "rect control value must contain an object"
        raise TypeError(message)
    channels = ("x", "y", "width", "height")
    _require_exact_keys(payload, set(channels), "rect")
    return {channel: _require_finite_number(payload[channel], "rect") for channel in channels}


def _hex_color_value(value: str) -> list[float] | None:
    color = value.strip().removeprefix("#")
    if len(color) not in {6, 8} or any(ch not in "0123456789abcdefABCDEF" for ch in color):
        return None
    red = _srgb_to_linear(int(color[0:2], 16) / 255)
    green = _srgb_to_linear(int(color[2:4], 16) / 255)
    blue = _srgb_to_linear(int(color[4:6], 16) / 255)
    alpha = int(color[6:8], 16) / 255 if len(color) == 8 else 1.0
    return [red, green, blue, alpha]


def _srgb_to_linear(channel: float) -> float:
    if channel <= 0.04045:
        return channel / 12.92
    return ((channel + 0.055) / 1.055) ** 2.4


def _request_path(path: str) -> str:
    if path.startswith(("http://", "https://")) or path.startswith(API_PREFIX):
        return path
    return f"{API_PREFIX}{path}"


def _quote_path(value: str) -> str:
    return quote(str(value), safe="")


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
