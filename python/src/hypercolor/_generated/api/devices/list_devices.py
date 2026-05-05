from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_response import ApiErrorResponse
from ...models.api_response_device_list_response import ApiResponseDeviceListResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    offset: int | Unset = UNSET,
    limit: int | Unset = UNSET,
    status: str | Unset = UNSET,
    backend_id: str | Unset = UNSET,
    driver: str | Unset = UNSET,
    q: str | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["offset"] = offset

    params["limit"] = limit

    params["status"] = status

    params["backend_id"] = backend_id

    params["driver"] = driver

    params["q"] = q

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/devices",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorResponse | ApiResponseDeviceListResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseDeviceListResponse.from_dict(response.json())

        return response_200

    if response.status_code == 422:
        response_422 = ApiErrorResponse.from_dict(response.json())

        return response_422

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ApiErrorResponse | ApiResponseDeviceListResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    offset: int | Unset = UNSET,
    limit: int | Unset = UNSET,
    status: str | Unset = UNSET,
    backend_id: str | Unset = UNSET,
    driver: str | Unset = UNSET,
    q: str | Unset = UNSET,
) -> Response[ApiErrorResponse | ApiResponseDeviceListResponse]:
    """`GET /api/v1/devices` — List all tracked devices.

    Args:
        offset (int | Unset):
        limit (int | Unset):
        status (str | Unset):
        backend_id (str | Unset):
        driver (str | Unset):
        q (str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorResponse | ApiResponseDeviceListResponse]
    """

    kwargs = _get_kwargs(
        offset=offset,
        limit=limit,
        status=status,
        backend_id=backend_id,
        driver=driver,
        q=q,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
    offset: int | Unset = UNSET,
    limit: int | Unset = UNSET,
    status: str | Unset = UNSET,
    backend_id: str | Unset = UNSET,
    driver: str | Unset = UNSET,
    q: str | Unset = UNSET,
) -> ApiErrorResponse | ApiResponseDeviceListResponse | None:
    """`GET /api/v1/devices` — List all tracked devices.

    Args:
        offset (int | Unset):
        limit (int | Unset):
        status (str | Unset):
        backend_id (str | Unset):
        driver (str | Unset):
        q (str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorResponse | ApiResponseDeviceListResponse
    """

    return sync_detailed(
        client=client,
        offset=offset,
        limit=limit,
        status=status,
        backend_id=backend_id,
        driver=driver,
        q=q,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    offset: int | Unset = UNSET,
    limit: int | Unset = UNSET,
    status: str | Unset = UNSET,
    backend_id: str | Unset = UNSET,
    driver: str | Unset = UNSET,
    q: str | Unset = UNSET,
) -> Response[ApiErrorResponse | ApiResponseDeviceListResponse]:
    """`GET /api/v1/devices` — List all tracked devices.

    Args:
        offset (int | Unset):
        limit (int | Unset):
        status (str | Unset):
        backend_id (str | Unset):
        driver (str | Unset):
        q (str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorResponse | ApiResponseDeviceListResponse]
    """

    kwargs = _get_kwargs(
        offset=offset,
        limit=limit,
        status=status,
        backend_id=backend_id,
        driver=driver,
        q=q,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    offset: int | Unset = UNSET,
    limit: int | Unset = UNSET,
    status: str | Unset = UNSET,
    backend_id: str | Unset = UNSET,
    driver: str | Unset = UNSET,
    q: str | Unset = UNSET,
) -> ApiErrorResponse | ApiResponseDeviceListResponse | None:
    """`GET /api/v1/devices` — List all tracked devices.

    Args:
        offset (int | Unset):
        limit (int | Unset):
        status (str | Unset):
        backend_id (str | Unset):
        driver (str | Unset):
        q (str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorResponse | ApiResponseDeviceListResponse
    """

    return (
        await asyncio_detailed(
            client=client,
            offset=offset,
            limit=limit,
            status=status,
            backend_id=backend_id,
            driver=driver,
            q=q,
        )
    ).parsed
