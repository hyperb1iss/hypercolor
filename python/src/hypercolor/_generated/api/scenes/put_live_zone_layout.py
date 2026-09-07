from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.put_live_zone_layout_response_200 import PutLiveZoneLayoutResponse200
from ...models.zone_layout_request import ZoneLayoutRequest
from ...types import Response


def _get_kwargs(
    zone: str,
    *,
    body: ZoneLayoutRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/api/v1/scene/zones/{zone}/layout".format(
            zone=quote(str(zone), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | PutLiveZoneLayoutResponse200 | None:
    if response.status_code == 200:
        response_200 = PutLiveZoneLayoutResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | PutLiveZoneLayoutResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    zone: str,
    *,
    client: AuthenticatedClient | Client,
    body: ZoneLayoutRequest,
) -> Response[ApiErrorBody | PutLiveZoneLayoutResponse200]:
    """Replace a live zone layout

    Args:
        zone (str):
        body (ZoneLayoutRequest): `PUT /scene/zones/{zone}/layout` — zone-scoped spatial override,
            in the same compact shape the zone resource reads back.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | PutLiveZoneLayoutResponse200]
    """

    kwargs = _get_kwargs(
        zone=zone,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    zone: str,
    *,
    client: AuthenticatedClient | Client,
    body: ZoneLayoutRequest,
) -> ApiErrorBody | PutLiveZoneLayoutResponse200 | None:
    """Replace a live zone layout

    Args:
        zone (str):
        body (ZoneLayoutRequest): `PUT /scene/zones/{zone}/layout` — zone-scoped spatial override,
            in the same compact shape the zone resource reads back.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | PutLiveZoneLayoutResponse200
    """

    return sync_detailed(
        zone=zone,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    zone: str,
    *,
    client: AuthenticatedClient | Client,
    body: ZoneLayoutRequest,
) -> Response[ApiErrorBody | PutLiveZoneLayoutResponse200]:
    """Replace a live zone layout

    Args:
        zone (str):
        body (ZoneLayoutRequest): `PUT /scene/zones/{zone}/layout` — zone-scoped spatial override,
            in the same compact shape the zone resource reads back.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | PutLiveZoneLayoutResponse200]
    """

    kwargs = _get_kwargs(
        zone=zone,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    zone: str,
    *,
    client: AuthenticatedClient | Client,
    body: ZoneLayoutRequest,
) -> ApiErrorBody | PutLiveZoneLayoutResponse200 | None:
    """Replace a live zone layout

    Args:
        zone (str):
        body (ZoneLayoutRequest): `PUT /scene/zones/{zone}/layout` — zone-scoped spatial override,
            in the same compact shape the zone resource reads back.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | PutLiveZoneLayoutResponse200
    """

    return (
        await asyncio_detailed(
            zone=zone,
            client=client,
            body=body,
        )
    ).parsed
