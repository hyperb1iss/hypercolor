from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.upload_asset_response_200 import UploadAssetResponse200
from ...models.upload_asset_response_201 import UploadAssetResponse201
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    rename_duplicate: bool | Unset = UNSET,
    type_: None | str | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["rename_duplicate"] = rename_duplicate

    json_type_: None | str | Unset
    if isinstance(type_, Unset):
        json_type_ = UNSET
    else:
        json_type_ = type_
    params["type"] = json_type_

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/assets",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | UploadAssetResponse200 | UploadAssetResponse201 | None:
    if response.status_code == 200:
        response_200 = UploadAssetResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 201:
        response_201 = UploadAssetResponse201.from_dict(response.json())

        return response_201

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
) -> Response[ApiErrorBody | UploadAssetResponse200 | UploadAssetResponse201]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    rename_duplicate: bool | Unset = UNSET,
    type_: None | str | Unset = UNSET,
) -> Response[ApiErrorBody | UploadAssetResponse200 | UploadAssetResponse201]:
    """Upload a media asset

    Args:
        rename_duplicate (bool | Unset):
        type_ (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | UploadAssetResponse200 | UploadAssetResponse201]
    """

    kwargs = _get_kwargs(
        rename_duplicate=rename_duplicate,
        type_=type_,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
    rename_duplicate: bool | Unset = UNSET,
    type_: None | str | Unset = UNSET,
) -> ApiErrorBody | UploadAssetResponse200 | UploadAssetResponse201 | None:
    """Upload a media asset

    Args:
        rename_duplicate (bool | Unset):
        type_ (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | UploadAssetResponse200 | UploadAssetResponse201
    """

    return sync_detailed(
        client=client,
        rename_duplicate=rename_duplicate,
        type_=type_,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    rename_duplicate: bool | Unset = UNSET,
    type_: None | str | Unset = UNSET,
) -> Response[ApiErrorBody | UploadAssetResponse200 | UploadAssetResponse201]:
    """Upload a media asset

    Args:
        rename_duplicate (bool | Unset):
        type_ (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | UploadAssetResponse200 | UploadAssetResponse201]
    """

    kwargs = _get_kwargs(
        rename_duplicate=rename_duplicate,
        type_=type_,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    rename_duplicate: bool | Unset = UNSET,
    type_: None | str | Unset = UNSET,
) -> ApiErrorBody | UploadAssetResponse200 | UploadAssetResponse201 | None:
    """Upload a media asset

    Args:
        rename_duplicate (bool | Unset):
        type_ (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | UploadAssetResponse200 | UploadAssetResponse201
    """

    return (
        await asyncio_detailed(
            client=client,
            rename_duplicate=rename_duplicate,
            type_=type_,
        )
    ).parsed
