"""Async client for the Hypercolor daemon API."""

from __future__ import annotations

import json
from collections.abc import Mapping
from typing import Any, Self, TypeVar

import httpx
import msgspec

from ._generated.api.config import set_config_value as generated_set_config_value
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
from ._generated.api.effects import (
    apply_effect as generated_apply_effect,
    get_active_effect as generated_get_active_effect,
    get_effect as generated_get_effect,
    list_effects as generated_list_effects,
    stop_effect as generated_stop_effect,
    update_current_controls as generated_update_current_controls,
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
from ._generated.api.settings import (
    list_audio_devices as generated_list_audio_devices,
    set_brightness as generated_set_brightness,
)
from ._generated.api.system import (
    get_status as generated_get_status,
    health_check as generated_health_check,
)
from ._generated.models.apply_control_changes_request import ApplyControlChangesRequest
from ._generated.models.apply_effect_request import ApplyEffectRequest
from ._generated.models.apply_profile_request import ApplyProfileRequest
from ._generated.models.discover_request import DiscoverRequest
from ._generated.models.identify_request import IdentifyRequest
from ._generated.models.invoke_control_action_request import InvokeControlActionRequest
from ._generated.models.set_brightness_request import SetBrightnessRequest
from ._generated.models.set_config_request import SetConfigRequest
from ._generated.models.update_current_controls_request import UpdateCurrentControlsRequest
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
    HypercolorRateLimitError,
    HypercolorUnavailableError,
    HypercolorValidationError,
)
from .models.audio import AudioDevices, SpectrumSnapshot
from .models.common import (
    BrightnessUpdate,
    ConfigMutationResult,
    DiscoverResult,
    IdentifyResult,
    MutationResult,
    TransitionSpec,
)
from .models.device import Device
from .models.effect import (
    ActiveEffect,
    ApplyEffectResult,
    ControlUpdateResult,
    Effect,
    EffectSummary,
)
from .models.layout import Layout, LayoutSummary
from .models.profile import ApplyProfileResult, Profile, ProfileSummary
from .models.scene import ActivateSceneResult, Scene
from .models.system import HealthStatus, SystemState
from .websocket import HypercolorEventStream

ModelT = TypeVar("ModelT")
_DEVICE_FILTERS = {"offset", "limit", "status", "backend", "q"}
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
    ) -> None:
        self.host = host
        self.port = port
        self.api_key = api_key
        self.timeout = timeout
        self.root_url = f"http://{host}:{port}"
        self.base_url = f"http://{host}:{port}{API_PREFIX}"
        self.ws_url = f"ws://{host}:{port}{WS_PATH}"
        self._client = httpx.AsyncClient(
            base_url=self.root_url,
            timeout=timeout,
            headers=self._auth_headers(),
            transport=transport,
        )

    async def __aenter__(self) -> Self:
        """Return self for async context-manager usage."""
        return self

    async def __aexit__(self, *_exc_info: object) -> None:
        """Close the underlying HTTP client."""
        await self.aclose()

    async def aclose(self) -> None:
        """Close the underlying HTTP client."""
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

    async def set_brightness(self, brightness: int) -> BrightnessUpdate:
        """Set the global daemon brightness."""
        return await self._generated_model(
            generated_set_brightness._get_kwargs(
                body=SetBrightnessRequest(brightness=brightness),
            ),
            BrightnessUpdate,
        )

    async def pause_rendering(self) -> MutationResult:
        """Backward-compatible alias that stops the active effect."""

        return await self.stop_effect()

    async def resume_rendering(self) -> MutationResult:
        """The daemon does not expose a resume endpoint."""

        message = "Hypercolor cannot resume rendering directly; apply an effect or profile instead"
        raise HypercolorApiError(message)

    async def get_devices(self, **filters: Any) -> list[Device]:
        """List devices."""
        if any(key not in _DEVICE_FILTERS for key in filters):
            return await self._request_items("GET", "/devices", Device, params=filters)
        return await self._generated_items(
            generated_list_devices._get_kwargs(
                offset=_generated_param(filters.get("offset")),
                limit=_generated_param(filters.get("limit")),
                status=_generated_param(filters.get("status")),
                backend=_generated_param(filters.get("backend")),
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

    async def get_active_effect(self) -> ActiveEffect | None:
        """Return the currently active effect if one exists."""
        try:
            return await self._generated_model(
                generated_get_active_effect._get_kwargs(),
                ActiveEffect,
            )
        except HypercolorNotFoundError:
            return None

    async def apply_effect(
        self,
        effect_id: str,
        *,
        controls: Mapping[str, Any] | None = None,
        transition: TransitionSpec | Mapping[str, Any] | None = None,
    ) -> ApplyEffectResult:
        """Apply an effect with optional control overrides."""
        body = _drop_none(
            {
                "controls": dict(controls) if controls is not None else None,
                "transition": _to_json_mapping(transition),
            }
        )
        kwargs = (
            generated_apply_effect._get_kwargs(
                effect_id,
                body=ApplyEffectRequest.from_dict(body),
            )
            if body
            else generated_apply_effect._get_kwargs(effect_id)
        )
        return await self._generated_model(
            kwargs,
            ApplyEffectResult,
        )

    async def update_controls(self, controls: Mapping[str, Any]) -> ControlUpdateResult:
        """Update controls on the active effect."""
        return await self._generated_model(
            generated_update_current_controls._get_kwargs(
                body=UpdateCurrentControlsRequest.from_dict({"controls": dict(controls)}),
            ),
            ControlUpdateResult,
        )

    async def get_control_surfaces(
        self,
        *,
        device_id: str | None = None,
        driver_id: str | None = None,
        include_driver: bool = False,
    ) -> list[dict[str, Any]]:
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
        return [dict(surface) for surface in surfaces if isinstance(surface, Mapping)]

    async def get_device_controls(self, device_id: str) -> dict[str, Any]:
        """Return a device control surface."""
        return await self._generated_payload(
            generated_get_device_control_surface._get_kwargs(device_id),
        )

    async def get_driver_controls(self, driver_id: str) -> dict[str, Any]:
        """Return a driver control surface."""
        return await self._generated_payload(
            generated_get_driver_control_surface._get_kwargs(driver_id),
        )

    async def set_control_values(
        self,
        surface_id: str,
        values: Mapping[str, Any],
        *,
        dry_run: bool = False,
        expected_revision: int | None = None,
    ) -> dict[str, Any]:
        """Apply one or more control values to a control surface."""
        body = _control_changes_request(
            surface_id,
            values,
            dry_run=dry_run,
            expected_revision=expected_revision,
        )
        return await self._generated_payload(
            generated_apply_control_surface_values._get_kwargs(
                surface_id,
                body=body,
            ),
        )

    async def invoke_control_action(
        self,
        surface_id: str,
        action_id: str,
        input: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Invoke a control-surface action."""
        body = InvokeControlActionRequest()
        if input is not None:
            body["input"] = {str(key): _control_api_value(value) for key, value in input.items()}
        return await self._generated_payload(
            generated_invoke_control_surface_action._get_kwargs(
                surface_id,
                action_id,
                body=body,
            ),
        )

    async def stop_effect(self) -> MutationResult:
        """Stop the currently active effect."""

        return await self._generated_model(
            generated_stop_effect._get_kwargs(),
            MutationResult,
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

    async def get_scenes(self, **filters: Any) -> list[Scene]:
        """List available scenes."""
        if any(key not in _SCENE_FILTERS for key in filters):
            return await self._request_items("GET", "/scenes", Scene, params=filters)
        return await self._generated_items(
            generated_list_scenes._get_kwargs(),
            Scene,
        )

    async def activate_scene(self, scene_id: str) -> ActivateSceneResult:
        """Trigger a scene manually."""
        return await self._generated_model(
            generated_activate_scene._get_kwargs(scene_id),
            ActivateSceneResult,
        )

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
            response = await self._client.request(**_drop_unset_json_body(kwargs))
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
        """Persist the selected audio input device."""

        return await self._generated_model(
            generated_set_config_value._get_kwargs(
                body=SetConfigRequest(
                    key="audio.device",
                    value=json.dumps(device_id),
                    live=live,
                ),
            ),
            ConfigMutationResult,
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

    async def _raw_request(
        self,
        method: str,
        path: str,
        *,
        body: Mapping[str, Any] | None = None,
        params: Mapping[str, Any] | None = None,
    ) -> Any:
        try:
            request_path = _request_path(path)
            response = await self._client.request(
                method,
                request_path,
                json=body,
                params=_drop_none(params or {}),
            )
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

    def _auth_headers(self) -> dict[str, str]:
        if self.api_key is None:
            return {}
        return {"Authorization": f"Bearer {self.api_key}"}

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


def _request_path(path: str) -> str:
    if path.startswith(("http://", "https://")) or path.startswith(API_PREFIX):
        return path
    return f"{API_PREFIX}{path}"


def _normalize_payload(value: Any) -> Any:
    if isinstance(value, list):
        return [_normalize_payload(item) for item in value]
    if not isinstance(value, dict):
        return value

    normalized = {key: _normalize_payload(item) for key, item in value.items()}
    if "control_type" in normalized and "name" in normalized and "default_value" in normalized:
        normalized.setdefault("label", normalized["name"])
        normalized.setdefault("type", _legacy_control_type(normalized["control_type"]))
        normalized.setdefault("default", _control_value(normalized["default_value"]))
    for key in ("control_values", "active_control_values", "applied_controls", "applied"):
        if isinstance(normalized.get(key), dict):
            normalized[key] = {
                str(item_key): _control_value(item_value)
                for item_key, item_value in normalized[key].items()
            }
    return normalized


def _control_value(value: Any) -> Any:
    if not isinstance(value, dict) or len(value) != 1:
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
