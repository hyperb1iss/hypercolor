from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.clear_scene_request import ClearSceneRequest
from ...models.clear_scene_response_200 import ClearSceneResponse200
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    body: ClearSceneRequest | Unset = UNSET,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/scene/clear",
    }

    if not isinstance(body, Unset):
        _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | ClearSceneResponse200 | None:
    if response.status_code == 200:
        response_200 = ClearSceneResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | ClearSceneResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    body: ClearSceneRequest | Unset = UNSET,
) -> Response[ApiErrorBody | ClearSceneResponse200]:
    """Clear live scene layers

    Args:
        body (ClearSceneRequest | Unset): `POST /scene/clear` — the "stop" gesture (Spec 78 §1.2).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ClearSceneResponse200]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
    body: ClearSceneRequest | Unset = UNSET,
) -> ApiErrorBody | ClearSceneResponse200 | None:
    """Clear live scene layers

    Args:
        body (ClearSceneRequest | Unset): `POST /scene/clear` — the "stop" gesture (Spec 78 §1.2).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ClearSceneResponse200
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    body: ClearSceneRequest | Unset = UNSET,
) -> Response[ApiErrorBody | ClearSceneResponse200]:
    """Clear live scene layers

    Args:
        body (ClearSceneRequest | Unset): `POST /scene/clear` — the "stop" gesture (Spec 78 §1.2).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ClearSceneResponse200]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    body: ClearSceneRequest | Unset = UNSET,
) -> ApiErrorBody | ClearSceneResponse200 | None:
    """Clear live scene layers

    Args:
        body (ClearSceneRequest | Unset): `POST /scene/clear` — the "stop" gesture (Spec 78 §1.2).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ClearSceneResponse200
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
