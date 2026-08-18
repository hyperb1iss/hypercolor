from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.api_response_vec_capture_monitor import ApiResponseVecCaptureMonitor
from ...types import Response


def _get_kwargs() -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/capture/monitors",
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | ApiResponseVecCaptureMonitor | None:
    if response.status_code == 200:
        response_200 = ApiResponseVecCaptureMonitor.from_dict(response.json())

        return response_200

    if response.status_code == 403:
        response_403 = ApiErrorBody.from_dict(response.json())

        return response_403

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ApiErrorBody | ApiResponseVecCaptureMonitor]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
) -> Response[ApiErrorBody | ApiResponseVecCaptureMonitor]:
    """`GET /api/v1/capture/monitors` — Display outputs capture can address.

     Empty on platforms where the backend picks its own source (the XDG
    portal on Linux); the UI uses emptiness to decide between a monitor
    dropdown and the portal picker button.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ApiResponseVecCaptureMonitor]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
) -> ApiErrorBody | ApiResponseVecCaptureMonitor | None:
    """`GET /api/v1/capture/monitors` — Display outputs capture can address.

     Empty on platforms where the backend picks its own source (the XDG
    portal on Linux); the UI uses emptiness to decide between a monitor
    dropdown and the portal picker button.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ApiResponseVecCaptureMonitor
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
) -> Response[ApiErrorBody | ApiResponseVecCaptureMonitor]:
    """`GET /api/v1/capture/monitors` — Display outputs capture can address.

     Empty on platforms where the backend picks its own source (the XDG
    portal on Linux); the UI uses emptiness to decide between a monitor
    dropdown and the portal picker button.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ApiResponseVecCaptureMonitor]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
) -> ApiErrorBody | ApiResponseVecCaptureMonitor | None:
    """`GET /api/v1/capture/monitors` — Display outputs capture can address.

     Empty on platforms where the backend picks its own source (the XDG
    portal on Linux); the UI uses emptiness to decide between a monitor
    dropdown and the portal picker button.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ApiResponseVecCaptureMonitor
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
