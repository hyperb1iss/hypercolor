from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.unassign_live_zone_member_response_200 import (
    UnassignLiveZoneMemberResponse200,
)
from ...types import Response


def _get_kwargs(
    zone: str,
    member: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/api/v1/scene/zones/{zone}/members/{member}".format(
            zone=quote(str(zone), safe=""),
            member=quote(str(member), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | UnassignLiveZoneMemberResponse200 | None:
    if response.status_code == 200:
        response_200 = UnassignLiveZoneMemberResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | UnassignLiveZoneMemberResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    zone: str,
    member: str,
    *,
    client: AuthenticatedClient | Client,
) -> Response[ApiErrorBody | UnassignLiveZoneMemberResponse200]:
    """Unassign a live zone member

    Args:
        zone (str):
        member (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | UnassignLiveZoneMemberResponse200]
    """

    kwargs = _get_kwargs(
        zone=zone,
        member=member,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    zone: str,
    member: str,
    *,
    client: AuthenticatedClient | Client,
) -> ApiErrorBody | UnassignLiveZoneMemberResponse200 | None:
    """Unassign a live zone member

    Args:
        zone (str):
        member (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | UnassignLiveZoneMemberResponse200
    """

    return sync_detailed(
        zone=zone,
        member=member,
        client=client,
    ).parsed


async def asyncio_detailed(
    zone: str,
    member: str,
    *,
    client: AuthenticatedClient | Client,
) -> Response[ApiErrorBody | UnassignLiveZoneMemberResponse200]:
    """Unassign a live zone member

    Args:
        zone (str):
        member (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | UnassignLiveZoneMemberResponse200]
    """

    kwargs = _get_kwargs(
        zone=zone,
        member=member,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    zone: str,
    member: str,
    *,
    client: AuthenticatedClient | Client,
) -> ApiErrorBody | UnassignLiveZoneMemberResponse200 | None:
    """Unassign a live zone member

    Args:
        zone (str):
        member (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | UnassignLiveZoneMemberResponse200
    """

    return (
        await asyncio_detailed(
            zone=zone,
            member=member,
            client=client,
        )
    ).parsed
