from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.delete_display_face_response_200 import DeleteDisplayFaceResponse200
from ...models.display_face_scope import DisplayFaceScope
from ...types import UNSET, Response, Unset


def _get_kwargs(
    id: str,
    *,
    scope: DisplayFaceScope | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_scope: str | Unset = UNSET
    if not isinstance(scope, Unset):
        json_scope = scope.value

    params["scope"] = json_scope

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/api/v1/displays/{id}/face".format(
            id=quote(str(id), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | DeleteDisplayFaceResponse200 | None:
    if response.status_code == 200:
        response_200 = DeleteDisplayFaceResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | DeleteDisplayFaceResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: str,
    *,
    client: AuthenticatedClient | Client,
    scope: DisplayFaceScope | Unset = UNSET,
) -> Response[ApiErrorBody | DeleteDisplayFaceResponse200]:
    """Delete display face assignment

    Args:
        id (str):
        scope (DisplayFaceScope | Unset): Which assignment layer a face operation targets (spec 69
            §3.6).

            `default` persists across scenes (the display's own face); `scene`
            writes into the active scene's display zone, which always wins while
            that scene is active.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | DeleteDisplayFaceResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        scope=scope,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: str,
    *,
    client: AuthenticatedClient | Client,
    scope: DisplayFaceScope | Unset = UNSET,
) -> ApiErrorBody | DeleteDisplayFaceResponse200 | None:
    """Delete display face assignment

    Args:
        id (str):
        scope (DisplayFaceScope | Unset): Which assignment layer a face operation targets (spec 69
            §3.6).

            `default` persists across scenes (the display's own face); `scene`
            writes into the active scene's display zone, which always wins while
            that scene is active.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | DeleteDisplayFaceResponse200
    """

    return sync_detailed(
        id=id,
        client=client,
        scope=scope,
    ).parsed


async def asyncio_detailed(
    id: str,
    *,
    client: AuthenticatedClient | Client,
    scope: DisplayFaceScope | Unset = UNSET,
) -> Response[ApiErrorBody | DeleteDisplayFaceResponse200]:
    """Delete display face assignment

    Args:
        id (str):
        scope (DisplayFaceScope | Unset): Which assignment layer a face operation targets (spec 69
            §3.6).

            `default` persists across scenes (the display's own face); `scene`
            writes into the active scene's display zone, which always wins while
            that scene is active.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | DeleteDisplayFaceResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        scope=scope,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: str,
    *,
    client: AuthenticatedClient | Client,
    scope: DisplayFaceScope | Unset = UNSET,
) -> ApiErrorBody | DeleteDisplayFaceResponse200 | None:
    """Delete display face assignment

    Args:
        id (str):
        scope (DisplayFaceScope | Unset): Which assignment layer a face operation targets (spec 69
            §3.6).

            `default` persists across scenes (the display's own face); `scene`
            writes into the active scene's display zone, which always wins while
            that scene is active.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | DeleteDisplayFaceResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            scope=scope,
        )
    ).parsed
