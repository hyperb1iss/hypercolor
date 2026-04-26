"""Async client for the Hypercolor daemon API."""

from __future__ import annotations

import json
from collections.abc import Mapping
from typing import Any, Self, TypeVar

import httpx
import msgspec

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
            base_url=self.base_url,
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
        response = await self._raw_request("GET", f"{self.root_url}/health")
        return self._convert(response, HealthStatus)

    async def get_status(self) -> SystemState:
        """Return the current daemon status snapshot."""

        return await self._request_model("GET", "/status", SystemState)

    async def get_state(self) -> SystemState:
        """Backward-compatible alias for :meth:`get_status`."""

        return await self.get_status()

    async def set_brightness(self, brightness: int) -> BrightnessUpdate:
        """Set the global daemon brightness."""
        return await self._request_model(
            "PUT",
            "/settings/brightness",
            BrightnessUpdate,
            body={"brightness": brightness},
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
        return await self._request_items("GET", "/devices", Device, params=filters)

    async def get_device(self, device_id: str) -> Device:
        """Fetch a single device."""
        return await self._request_model("GET", f"/devices/{device_id}", Device)

    async def update_device(self, device_id: str, **fields: Any) -> Device:
        """Update device configuration."""
        return await self._request_model("PUT", f"/devices/{device_id}", Device, body=fields)

    async def discover_devices(
        self,
        backends: list[str] | None = None,
        timeout_ms: int | None = None,
    ) -> DiscoverResult:
        """Trigger a device discovery scan."""
        body = _drop_none({"backends": backends, "timeout_ms": timeout_ms})
        return await self._request_model(
            "POST", "/devices/discover", DiscoverResult, body=body or None
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
        return await self._request_model(
            "POST",
            f"/devices/{device_id}/identify",
            IdentifyResult,
            body=body or None,
        )

    async def get_effects(self, **filters: Any) -> list[EffectSummary]:
        """List available effects."""
        return await self._request_items("GET", "/effects", EffectSummary, params=filters)

    async def get_effect(self, effect_id: str) -> Effect:
        """Fetch a single effect with controls."""
        return await self._request_model("GET", f"/effects/{effect_id}", Effect)

    async def get_active_effect(self) -> ActiveEffect | None:
        """Return the currently active effect if one exists."""
        try:
            return await self._request_model("GET", "/effects/active", ActiveEffect)
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
        return await self._request_model(
            "POST",
            f"/effects/{effect_id}/apply",
            ApplyEffectResult,
            body=body or None,
        )

    async def update_controls(self, controls: Mapping[str, Any]) -> ControlUpdateResult:
        """Update controls on the active effect."""
        return await self._request_model(
            "PATCH",
            "/effects/current/controls",
            ControlUpdateResult,
            body={"controls": dict(controls)},
        )

    async def stop_effect(self) -> MutationResult:
        """Stop the currently active effect."""

        return await self._request_model("POST", "/effects/stop", MutationResult)

    async def get_layouts(self) -> list[LayoutSummary]:
        """List layouts."""
        return await self._request_items("GET", "/layouts", LayoutSummary)

    async def get_active_layout(self) -> Layout | None:
        """Return the active layout if one exists."""
        try:
            return await self._request_model("GET", "/layouts/active", Layout)
        except HypercolorNotFoundError:
            return None

    async def apply_layout(self, layout_id: str) -> MutationResult:
        """Apply a layout."""
        return await self._request_model("POST", f"/layouts/{layout_id}/apply", MutationResult)

    async def get_profiles(self) -> list[ProfileSummary]:
        """List saved profiles."""
        return await self._request_items("GET", "/profiles", ProfileSummary)

    async def get_profile(self, profile_id: str) -> Profile:
        """Fetch a single profile."""
        return await self._request_model("GET", f"/profiles/{profile_id}", Profile)

    async def apply_profile(
        self,
        profile_id: str,
        *,
        transition: TransitionSpec | Mapping[str, Any] | None = None,
    ) -> ApplyProfileResult:
        """Apply a saved profile."""
        body = _drop_none({"transition": _to_json_mapping(transition)})
        return await self._request_model(
            "POST",
            f"/profiles/{profile_id}/apply",
            ApplyProfileResult,
            body=body or None,
        )

    async def get_scenes(self, **filters: Any) -> list[Scene]:
        """List available scenes."""
        return await self._request_items("GET", "/scenes", Scene, params=filters)

    async def activate_scene(self, scene_id: str) -> ActivateSceneResult:
        """Trigger a scene manually."""
        return await self._request_model(
            "POST", f"/scenes/{scene_id}/activate", ActivateSceneResult
        )

    async def get_audio_spectrum(self) -> SpectrumSnapshot:
        """Return the current audio spectrum snapshot."""

        message = (
            "Audio spectrum snapshots are only available over the Hypercolor WebSocket stream"
        )
        raise HypercolorNotFoundError(message, status_code=404)

    async def get_audio_devices(self) -> AudioDevices:
        """Return the available audio capture devices."""

        return await self._request_model("GET", "/audio/devices", AudioDevices)

    async def set_audio_device(
        self,
        device_id: str,
        *,
        live: bool = True,
    ) -> ConfigMutationResult:
        """Persist the selected audio input device."""

        return await self._request_model(
            "POST",
            "/config/set",
            ConfigMutationResult,
            body={
                "key": "audio.device",
                "value": json.dumps(device_id),
                "live": live,
            },
        )

    async def _request_model(
        self,
        method: str,
        path: str,
        model_type: type[ModelT],
        *,
        body: Mapping[str, Any] | None = None,
        params: Mapping[str, Any] | None = None,
    ) -> ModelT:
        response = await self._raw_request(method, path, body=body, params=params)
        data = self._unwrap_data(response)
        return self._convert(data, model_type)

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
            response = await self._client.request(
                method,
                path,
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
        error = _decode_error_details(response)
        message = (
            error.message
            if error is not None
            else f"Hypercolor API request failed with {response.status_code}"
        )

        status = response.status_code
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
        if status == 401:
            error_type = HypercolorAuthenticationError
        elif status == 404:
            error_type = HypercolorNotFoundError
        elif status == 409:
            error_type = HypercolorConflictError
        elif status == 422:
            error_type = HypercolorValidationError
        elif status == 429:
            error_type = HypercolorRateLimitError
        elif status == 503:
            error_type = HypercolorUnavailableError
        else:
            error_type = HypercolorApiError
        return _instantiate_error(error_type, message, error, response.status_code)


def _decode_error_details(response: httpx.Response) -> ApiErrorDetails | None:
    try:
        payload = msgspec.json.decode(response.content)
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
