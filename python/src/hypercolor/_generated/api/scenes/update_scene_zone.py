from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.update_zone_request import UpdateZoneRequest
from ...types import Response


def _get_kwargs(
    id: str,
    zone_id: str,
    *,
    body: UpdateZoneRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "patch",
        "url": "/api/v1/scenes/{id}/zones/{zone_id}".format(
            id=quote(str(id), safe=""),
            zone_id=quote(str(zone_id), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | None:
    if response.status_code == 200:
        return None

    if response.status_code == 400:
        return None

    if response.status_code == 404:
        return None

    if response.status_code == 409:
        return None

    if response.status_code == 412:
        return None

    if response.status_code == 422:
        return None

    if response.status_code == 500:
        return None

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: str,
    zone_id: str,
    *,
    client: AuthenticatedClient | Client,
    body: UpdateZoneRequest,
) -> Response[Any]:
    """Update scene zone

    Args:
        id (str):
        zone_id (str):
        body (UpdateZoneRequest): Partial zone-metadata patch for `PATCH
            /api/v1/scenes/{id}/zones/{zone_id}`.

            Every field is optional; only supplied ones change. `description` and
            `color` are doubly-optional so clients can distinguish "leave
            unchanged" (`None`, skipped on the wire) from "clear it"
            (`Some(None)`, serialized as `null`).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        id=id,
        zone_id=zone_id,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


async def asyncio_detailed(
    id: str,
    zone_id: str,
    *,
    client: AuthenticatedClient | Client,
    body: UpdateZoneRequest,
) -> Response[Any]:
    """Update scene zone

    Args:
        id (str):
        zone_id (str):
        body (UpdateZoneRequest): Partial zone-metadata patch for `PATCH
            /api/v1/scenes/{id}/zones/{zone_id}`.

            Every field is optional; only supplied ones change. `description` and
            `color` are doubly-optional so clients can distinguish "leave
            unchanged" (`None`, skipped on the wire) from "clear it"
            (`Some(None)`, serialized as `null`).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        id=id,
        zone_id=zone_id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)
