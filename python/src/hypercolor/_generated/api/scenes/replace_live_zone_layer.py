from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.replace_layer_request import ReplaceLayerRequest
from ...models.replace_live_zone_layer_response_200 import (
    ReplaceLiveZoneLayerResponse200,
)
from ...types import Response


def _get_kwargs(
    zone: str,
    layer: str,
    *,
    body: ReplaceLayerRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/api/v1/scene/zones/{zone}/layers/{layer}".format(
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
) -> ApiErrorBody | ReplaceLiveZoneLayerResponse200 | None:
    if response.status_code == 200:
        response_200 = ReplaceLiveZoneLayerResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | ReplaceLiveZoneLayerResponse200]:
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
    body: ReplaceLayerRequest,
) -> Response[ApiErrorBody | ReplaceLiveZoneLayerResponse200]:
    """Replace a live zone layer

    Args:
        zone (str):
        layer (str):
        body (ReplaceLayerRequest): `PUT /scene/zones/{zone}/layers/{layer}`: whole-layer replace.

            Replacement is creation: every successful `PUT` mints a fresh layer
            id, same effect or not (Spec 78 §1.4). The request shape is the
            creation shape; the path names the layer being replaced.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ReplaceLiveZoneLayerResponse200]
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
    body: ReplaceLayerRequest,
) -> ApiErrorBody | ReplaceLiveZoneLayerResponse200 | None:
    """Replace a live zone layer

    Args:
        zone (str):
        layer (str):
        body (ReplaceLayerRequest): `PUT /scene/zones/{zone}/layers/{layer}`: whole-layer replace.

            Replacement is creation: every successful `PUT` mints a fresh layer
            id, same effect or not (Spec 78 §1.4). The request shape is the
            creation shape; the path names the layer being replaced.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ReplaceLiveZoneLayerResponse200
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
    body: ReplaceLayerRequest,
) -> Response[ApiErrorBody | ReplaceLiveZoneLayerResponse200]:
    """Replace a live zone layer

    Args:
        zone (str):
        layer (str):
        body (ReplaceLayerRequest): `PUT /scene/zones/{zone}/layers/{layer}`: whole-layer replace.

            Replacement is creation: every successful `PUT` mints a fresh layer
            id, same effect or not (Spec 78 §1.4). The request shape is the
            creation shape; the path names the layer being replaced.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ReplaceLiveZoneLayerResponse200]
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
    body: ReplaceLayerRequest,
) -> ApiErrorBody | ReplaceLiveZoneLayerResponse200 | None:
    """Replace a live zone layer

    Args:
        zone (str):
        layer (str):
        body (ReplaceLayerRequest): `PUT /scene/zones/{zone}/layers/{layer}`: whole-layer replace.

            Replacement is creation: every successful `PUT` mints a fresh layer
            id, same effect or not (Spec 78 §1.4). The request shape is the
            creation shape; the path names the layer being replaced.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ReplaceLiveZoneLayerResponse200
    """

    return (
        await asyncio_detailed(
            zone=zone,
            layer=layer,
            client=client,
            body=body,
        )
    ).parsed
