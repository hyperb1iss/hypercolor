from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.list_devices_response_200 import ListDevicesResponse200
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    status: None | str | Unset = UNSET,
    backend_id: None | str | Unset = UNSET,
    driver: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    include: None | str | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_offset: int | None | Unset
    if isinstance(offset, Unset):
        json_offset = UNSET
    else:
        json_offset = offset
    params["offset"] = json_offset

    json_limit: int | None | Unset
    if isinstance(limit, Unset):
        json_limit = UNSET
    else:
        json_limit = limit
    params["limit"] = json_limit

    json_status: None | str | Unset
    if isinstance(status, Unset):
        json_status = UNSET
    else:
        json_status = status
    params["status"] = json_status

    json_backend_id: None | str | Unset
    if isinstance(backend_id, Unset):
        json_backend_id = UNSET
    else:
        json_backend_id = backend_id
    params["backend_id"] = json_backend_id

    json_driver: None | str | Unset
    if isinstance(driver, Unset):
        json_driver = UNSET
    else:
        json_driver = driver
    params["driver"] = json_driver

    json_q: None | str | Unset
    if isinstance(q, Unset):
        json_q = UNSET
    else:
        json_q = q
    params["q"] = json_q

    json_include: None | str | Unset
    if isinstance(include, Unset):
        json_include = UNSET
    else:
        json_include = include
    params["include"] = json_include

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/devices",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | ListDevicesResponse200 | None:
    if response.status_code == 200:
        response_200 = ListDevicesResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | ListDevicesResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    status: None | str | Unset = UNSET,
    backend_id: None | str | Unset = UNSET,
    driver: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    include: None | str | Unset = UNSET,
) -> Response[ApiErrorBody | ListDevicesResponse200]:
    """List tracked devices

    Args:
        offset (int | None | Unset):
        limit (int | None | Unset):
        status (None | str | Unset):
        backend_id (None | str | Unset):
        driver (None | str | Unset):
        q (None | str | Unset):
        include (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ListDevicesResponse200]
    """

    kwargs = _get_kwargs(
        offset=offset,
        limit=limit,
        status=status,
        backend_id=backend_id,
        driver=driver,
        q=q,
        include=include,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    status: None | str | Unset = UNSET,
    backend_id: None | str | Unset = UNSET,
    driver: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    include: None | str | Unset = UNSET,
) -> ApiErrorBody | ListDevicesResponse200 | None:
    """List tracked devices

    Args:
        offset (int | None | Unset):
        limit (int | None | Unset):
        status (None | str | Unset):
        backend_id (None | str | Unset):
        driver (None | str | Unset):
        q (None | str | Unset):
        include (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ListDevicesResponse200
    """

    return sync_detailed(
        client=client,
        offset=offset,
        limit=limit,
        status=status,
        backend_id=backend_id,
        driver=driver,
        q=q,
        include=include,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    status: None | str | Unset = UNSET,
    backend_id: None | str | Unset = UNSET,
    driver: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    include: None | str | Unset = UNSET,
) -> Response[ApiErrorBody | ListDevicesResponse200]:
    """List tracked devices

    Args:
        offset (int | None | Unset):
        limit (int | None | Unset):
        status (None | str | Unset):
        backend_id (None | str | Unset):
        driver (None | str | Unset):
        q (None | str | Unset):
        include (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ListDevicesResponse200]
    """

    kwargs = _get_kwargs(
        offset=offset,
        limit=limit,
        status=status,
        backend_id=backend_id,
        driver=driver,
        q=q,
        include=include,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    status: None | str | Unset = UNSET,
    backend_id: None | str | Unset = UNSET,
    driver: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    include: None | str | Unset = UNSET,
) -> ApiErrorBody | ListDevicesResponse200 | None:
    """List tracked devices

    Args:
        offset (int | None | Unset):
        limit (int | None | Unset):
        status (None | str | Unset):
        backend_id (None | str | Unset):
        driver (None | str | Unset):
        q (None | str | Unset):
        include (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ListDevicesResponse200
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
            include=include,
        )
    ).parsed
