from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.apply_effect_preset_response_200 import ApplyEffectPresetResponse200
from ...models.apply_effect_request import ApplyEffectRequest
from ...types import UNSET, Response, Unset


def _get_kwargs(
    id: str,
    preset: str,
    *,
    body: ApplyEffectRequest | Unset = UNSET,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/effects/{id}/presets/{preset}/apply".format(
            id=quote(str(id), safe=""),
            preset=quote(str(preset), safe=""),
        ),
    }

    if not isinstance(body, Unset):
        _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | ApplyEffectPresetResponse200 | None:
    if response.status_code == 200:
        response_200 = ApplyEffectPresetResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | ApplyEffectPresetResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: str,
    preset: str,
    *,
    client: AuthenticatedClient | Client,
    body: ApplyEffectRequest | Unset = UNSET,
) -> Response[ApiErrorBody | ApplyEffectPresetResponse200]:
    """Apply effect preset

    Args:
        id (str):
        preset (str):
        body (ApplyEffectRequest | Unset): `POST /effects/{id}/apply` — the sugar request (Spec 78
            §2.3).

            Replaces the target zone's layer stack with a single new layer
            running this effect; a projection of the same `SceneMutation` a
            layer-stack replacement performs, never a second code path.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ApplyEffectPresetResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        preset=preset,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: str,
    preset: str,
    *,
    client: AuthenticatedClient | Client,
    body: ApplyEffectRequest | Unset = UNSET,
) -> ApiErrorBody | ApplyEffectPresetResponse200 | None:
    """Apply effect preset

    Args:
        id (str):
        preset (str):
        body (ApplyEffectRequest | Unset): `POST /effects/{id}/apply` — the sugar request (Spec 78
            §2.3).

            Replaces the target zone's layer stack with a single new layer
            running this effect; a projection of the same `SceneMutation` a
            layer-stack replacement performs, never a second code path.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ApplyEffectPresetResponse200
    """

    return sync_detailed(
        id=id,
        preset=preset,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: str,
    preset: str,
    *,
    client: AuthenticatedClient | Client,
    body: ApplyEffectRequest | Unset = UNSET,
) -> Response[ApiErrorBody | ApplyEffectPresetResponse200]:
    """Apply effect preset

    Args:
        id (str):
        preset (str):
        body (ApplyEffectRequest | Unset): `POST /effects/{id}/apply` — the sugar request (Spec 78
            §2.3).

            Replaces the target zone's layer stack with a single new layer
            running this effect; a projection of the same `SceneMutation` a
            layer-stack replacement performs, never a second code path.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ApplyEffectPresetResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        preset=preset,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: str,
    preset: str,
    *,
    client: AuthenticatedClient | Client,
    body: ApplyEffectRequest | Unset = UNSET,
) -> ApiErrorBody | ApplyEffectPresetResponse200 | None:
    """Apply effect preset

    Args:
        id (str):
        preset (str):
        body (ApplyEffectRequest | Unset): `POST /effects/{id}/apply` — the sugar request (Spec 78
            §2.3).

            Replaces the target zone's layer stack with a single new layer
            running this effect; a projection of the same `SceneMutation` a
            layer-stack replacement performs, never a second code path.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ApplyEffectPresetResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            preset=preset,
            client=client,
            body=body,
        )
    ).parsed
