from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.preview_layout_response_200 import PreviewLayoutResponse200
from ...models.spatial_layout import SpatialLayout
from ...types import Response


def _get_kwargs(
    *,
    body: SpatialLayout,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/api/v1/layouts/active/preview",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | PreviewLayoutResponse200 | None:
    if response.status_code == 200:
        response_200 = PreviewLayoutResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | PreviewLayoutResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    body: SpatialLayout,
) -> Response[ApiErrorBody | PreviewLayoutResponse200]:
    """Preview active layout

    Args:
        body (SpatialLayout): Top-level spatial layout container.

            Defines the complete mapping from a 2D effect canvas to the physical LED
            positions of every connected device. All coordinates use normalized
            `[0.0, 1.0]` space where `(0,0)` is top-left and `(1,1)` is bottom-right.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | PreviewLayoutResponse200]
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
    body: SpatialLayout,
) -> ApiErrorBody | PreviewLayoutResponse200 | None:
    """Preview active layout

    Args:
        body (SpatialLayout): Top-level spatial layout container.

            Defines the complete mapping from a 2D effect canvas to the physical LED
            positions of every connected device. All coordinates use normalized
            `[0.0, 1.0]` space where `(0,0)` is top-left and `(1,1)` is bottom-right.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | PreviewLayoutResponse200
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    body: SpatialLayout,
) -> Response[ApiErrorBody | PreviewLayoutResponse200]:
    """Preview active layout

    Args:
        body (SpatialLayout): Top-level spatial layout container.

            Defines the complete mapping from a 2D effect canvas to the physical LED
            positions of every connected device. All coordinates use normalized
            `[0.0, 1.0]` space where `(0,0)` is top-left and `(1,1)` is bottom-right.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | PreviewLayoutResponse200]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    body: SpatialLayout,
) -> ApiErrorBody | PreviewLayoutResponse200 | None:
    """Preview active layout

    Args:
        body (SpatialLayout): Top-level spatial layout container.

            Defines the complete mapping from a 2D effect canvas to the physical LED
            positions of every connected device. All coordinates use normalized
            `[0.0, 1.0]` space where `(0,0)` is top-left and `(1,1)` is bottom-right.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | PreviewLayoutResponse200
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
