from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.patch_display_face_controls_response_200 import (
    PatchDisplayFaceControlsResponse200,
)
from ...models.update_display_face_controls_request import (
    UpdateDisplayFaceControlsRequest,
)
from ...types import Response


def _get_kwargs(
    id: str,
    *,
    body: UpdateDisplayFaceControlsRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "patch",
        "url": "/api/v1/displays/{id}/face/controls".format(
            id=quote(str(id), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | PatchDisplayFaceControlsResponse200 | None:
    if response.status_code == 200:
        response_200 = PatchDisplayFaceControlsResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | PatchDisplayFaceControlsResponse200]:
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
    body: UpdateDisplayFaceControlsRequest,
) -> Response[ApiErrorBody | PatchDisplayFaceControlsResponse200]:
    """Patch display face controls

    Args:
        id (str):
        body (UpdateDisplayFaceControlsRequest): Request body for `PATCH
            /api/v1/displays/{id}/face/controls`.

            The payload carries only the overrides the caller wants to change;
            existing control values on the zone are preserved unless their
            key appears in this map. `controls` is typed as raw JSON (rather than
            `HashMap<String, ControlValue>`) so callers can send natural shapes
            like `{"accent": 0.5}` instead of `{"accent": {"float": 0.5}}`, which
            mirrors the effects controls patch endpoint.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | PatchDisplayFaceControlsResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: str,
    *,
    client: AuthenticatedClient | Client,
    body: UpdateDisplayFaceControlsRequest,
) -> ApiErrorBody | PatchDisplayFaceControlsResponse200 | None:
    """Patch display face controls

    Args:
        id (str):
        body (UpdateDisplayFaceControlsRequest): Request body for `PATCH
            /api/v1/displays/{id}/face/controls`.

            The payload carries only the overrides the caller wants to change;
            existing control values on the zone are preserved unless their
            key appears in this map. `controls` is typed as raw JSON (rather than
            `HashMap<String, ControlValue>`) so callers can send natural shapes
            like `{"accent": 0.5}` instead of `{"accent": {"float": 0.5}}`, which
            mirrors the effects controls patch endpoint.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | PatchDisplayFaceControlsResponse200
    """

    return sync_detailed(
        id=id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: str,
    *,
    client: AuthenticatedClient | Client,
    body: UpdateDisplayFaceControlsRequest,
) -> Response[ApiErrorBody | PatchDisplayFaceControlsResponse200]:
    """Patch display face controls

    Args:
        id (str):
        body (UpdateDisplayFaceControlsRequest): Request body for `PATCH
            /api/v1/displays/{id}/face/controls`.

            The payload carries only the overrides the caller wants to change;
            existing control values on the zone are preserved unless their
            key appears in this map. `controls` is typed as raw JSON (rather than
            `HashMap<String, ControlValue>`) so callers can send natural shapes
            like `{"accent": 0.5}` instead of `{"accent": {"float": 0.5}}`, which
            mirrors the effects controls patch endpoint.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | PatchDisplayFaceControlsResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: str,
    *,
    client: AuthenticatedClient | Client,
    body: UpdateDisplayFaceControlsRequest,
) -> ApiErrorBody | PatchDisplayFaceControlsResponse200 | None:
    """Patch display face controls

    Args:
        id (str):
        body (UpdateDisplayFaceControlsRequest): Request body for `PATCH
            /api/v1/displays/{id}/face/controls`.

            The payload carries only the overrides the caller wants to change;
            existing control values on the zone are preserved unless their
            key appears in this map. `controls` is typed as raw JSON (rather than
            `HashMap<String, ControlValue>`) so callers can send natural shapes
            like `{"accent": 0.5}` instead of `{"accent": {"float": 0.5}}`, which
            mirrors the effects controls patch endpoint.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | PatchDisplayFaceControlsResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
        )
    ).parsed
