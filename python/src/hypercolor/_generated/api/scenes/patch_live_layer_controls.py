from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.patch_controls_request import PatchControlsRequest
from ...models.patch_live_layer_controls_response_200 import (
    PatchLiveLayerControlsResponse200,
)
from ...types import Response


def _get_kwargs(
    zone: str,
    layer: str,
    *,
    body: PatchControlsRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "patch",
        "url": "/api/v1/scene/zones/{zone}/layers/{layer}/controls".format(
            zone=quote(str(zone), safe=""),
            layer=quote(str(layer), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | PatchLiveLayerControlsResponse200 | None:
    if response.status_code == 200:
        response_200 = PatchLiveLayerControlsResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = ApiErrorBody.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = ApiErrorBody.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = ApiErrorBody.from_dict(response.json())

        return response_403

    if response.status_code == 404:
        response_404 = ApiErrorBody.from_dict(response.json())

        return response_404

    if response.status_code == 409:
        response_409 = ApiErrorBody.from_dict(response.json())

        return response_409

    if response.status_code == 412:
        response_412 = ApiErrorBody.from_dict(response.json())

        return response_412

    if response.status_code == 422:
        response_422 = ApiErrorBody.from_dict(response.json())

        return response_422

    if response.status_code == 429:
        response_429 = ApiErrorBody.from_dict(response.json())

        return response_429

    if response.status_code == 500:
        response_500 = ApiErrorBody.from_dict(response.json())

        return response_500

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ApiErrorBody | PatchLiveLayerControlsResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    zone: str,
    layer: str,
    *,
    client: AuthenticatedClient | Client,
    body: PatchControlsRequest,
) -> Response[ApiErrorBody | PatchLiveLayerControlsResponse200]:
    """Patch live layer controls

    Args:
        zone (str):
        layer (str):
        body (PatchControlsRequest): The one control-patch shape, used verbatim at every scope:
            layer
            controls, display face controls, control-surface values
            (Spec 78 §5.7).

            `clear_bindings` is meaningful only where bindings exist (layers);
            other scopes reject a non-empty list with a validation error. A
            patch naming a control key with an active input binding is rejected
            409 `control_bound` unless the same request clears that binding —
            removal and the accompanying values land in one atomic commit
            (Spec 78 §1.6).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | PatchLiveLayerControlsResponse200]
    """

    kwargs = _get_kwargs(
        zone=zone,
        layer=layer,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    zone: str,
    layer: str,
    *,
    client: AuthenticatedClient | Client,
    body: PatchControlsRequest,
) -> ApiErrorBody | PatchLiveLayerControlsResponse200 | None:
    """Patch live layer controls

    Args:
        zone (str):
        layer (str):
        body (PatchControlsRequest): The one control-patch shape, used verbatim at every scope:
            layer
            controls, display face controls, control-surface values
            (Spec 78 §5.7).

            `clear_bindings` is meaningful only where bindings exist (layers);
            other scopes reject a non-empty list with a validation error. A
            patch naming a control key with an active input binding is rejected
            409 `control_bound` unless the same request clears that binding —
            removal and the accompanying values land in one atomic commit
            (Spec 78 §1.6).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | PatchLiveLayerControlsResponse200
    """

    return sync_detailed(
        zone=zone,
        layer=layer,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    zone: str,
    layer: str,
    *,
    client: AuthenticatedClient | Client,
    body: PatchControlsRequest,
) -> Response[ApiErrorBody | PatchLiveLayerControlsResponse200]:
    """Patch live layer controls

    Args:
        zone (str):
        layer (str):
        body (PatchControlsRequest): The one control-patch shape, used verbatim at every scope:
            layer
            controls, display face controls, control-surface values
            (Spec 78 §5.7).

            `clear_bindings` is meaningful only where bindings exist (layers);
            other scopes reject a non-empty list with a validation error. A
            patch naming a control key with an active input binding is rejected
            409 `control_bound` unless the same request clears that binding —
            removal and the accompanying values land in one atomic commit
            (Spec 78 §1.6).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | PatchLiveLayerControlsResponse200]
    """

    kwargs = _get_kwargs(
        zone=zone,
        layer=layer,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    zone: str,
    layer: str,
    *,
    client: AuthenticatedClient | Client,
    body: PatchControlsRequest,
) -> ApiErrorBody | PatchLiveLayerControlsResponse200 | None:
    """Patch live layer controls

    Args:
        zone (str):
        layer (str):
        body (PatchControlsRequest): The one control-patch shape, used verbatim at every scope:
            layer
            controls, display face controls, control-surface values
            (Spec 78 §5.7).

            `clear_bindings` is meaningful only where bindings exist (layers);
            other scopes reject a non-empty list with a validation error. A
            patch naming a control key with an active input binding is rejected
            409 `control_bound` unless the same request clears that binding —
            removal and the accompanying values land in one atomic commit
            (Spec 78 §1.6).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | PatchLiveLayerControlsResponse200
    """

    return (
        await asyncio_detailed(
            zone=zone,
            layer=layer,
            client=client,
            body=body,
        )
    ).parsed
