from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.list_control_surfaces_response_200 import ListControlSurfacesResponse200
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    device_id: None | str | Unset = UNSET,
    driver_id: None | str | Unset = UNSET,
    include_driver: bool | None | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_device_id: None | str | Unset
    if isinstance(device_id, Unset):
        json_device_id = UNSET
    else:
        json_device_id = device_id
    params["device_id"] = json_device_id

    json_driver_id: None | str | Unset
    if isinstance(driver_id, Unset):
        json_driver_id = UNSET
    else:
        json_driver_id = driver_id
    params["driver_id"] = json_driver_id

    json_include_driver: bool | None | Unset
    if isinstance(include_driver, Unset):
        json_include_driver = UNSET
    else:
        json_include_driver = include_driver
    params["include_driver"] = json_include_driver

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/control-surfaces",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | ListControlSurfacesResponse200 | None:
    if response.status_code == 200:
        response_200 = ListControlSurfacesResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | ListControlSurfacesResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    device_id: None | str | Unset = UNSET,
    driver_id: None | str | Unset = UNSET,
    include_driver: bool | None | Unset = UNSET,
) -> Response[ApiErrorBody | ListControlSurfacesResponse200]:
    """List control surfaces

    Args:
        device_id (None | str | Unset):
        driver_id (None | str | Unset):
        include_driver (bool | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ListControlSurfacesResponse200]
    """

    kwargs = _get_kwargs(
        device_id=device_id,
        driver_id=driver_id,
        include_driver=include_driver,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
    device_id: None | str | Unset = UNSET,
    driver_id: None | str | Unset = UNSET,
    include_driver: bool | None | Unset = UNSET,
) -> ApiErrorBody | ListControlSurfacesResponse200 | None:
    """List control surfaces

    Args:
        device_id (None | str | Unset):
        driver_id (None | str | Unset):
        include_driver (bool | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ListControlSurfacesResponse200
    """

    return sync_detailed(
        client=client,
        device_id=device_id,
        driver_id=driver_id,
        include_driver=include_driver,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    device_id: None | str | Unset = UNSET,
    driver_id: None | str | Unset = UNSET,
    include_driver: bool | None | Unset = UNSET,
) -> Response[ApiErrorBody | ListControlSurfacesResponse200]:
    """List control surfaces

    Args:
        device_id (None | str | Unset):
        driver_id (None | str | Unset):
        include_driver (bool | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ListControlSurfacesResponse200]
    """

    kwargs = _get_kwargs(
        device_id=device_id,
        driver_id=driver_id,
        include_driver=include_driver,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    device_id: None | str | Unset = UNSET,
    driver_id: None | str | Unset = UNSET,
    include_driver: bool | None | Unset = UNSET,
) -> ApiErrorBody | ListControlSurfacesResponse200 | None:
    """List control surfaces

    Args:
        device_id (None | str | Unset):
        driver_id (None | str | Unset):
        include_driver (bool | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ListControlSurfacesResponse200
    """

    return (
        await asyncio_detailed(
            client=client,
            device_id=device_id,
            driver_id=driver_id,
            include_driver=include_driver,
        )
    ).parsed
