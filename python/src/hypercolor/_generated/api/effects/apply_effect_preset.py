from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.api_response_apply_effect_response import ApiResponseApplyEffectResponse
from ...models.apply_effect_preset_request import ApplyEffectPresetRequest
from ...types import UNSET, Response, Unset


def _get_kwargs(
    id: str,
    preset_id: str,
    *,
    body: ApplyEffectPresetRequest | None | Unset = UNSET,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/effects/{id}/presets/{preset_id}/apply".format(
            id=quote(str(id), safe=""),
            preset_id=quote(str(preset_id), safe=""),
        ),
    }

    if isinstance(body, ApplyEffectPresetRequest):
        _kwargs["json"] = body.to_dict()
    else:
        _kwargs["json"] = body

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | ApiResponseApplyEffectResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseApplyEffectResponse.from_dict(response.json())

        return response_200

    if response.status_code == 404:
        response_404 = ApiErrorBody.from_dict(response.json())

        return response_404

    if response.status_code == 422:
        response_422 = ApiErrorBody.from_dict(response.json())

        return response_422

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ApiErrorBody | ApiResponseApplyEffectResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: str,
    preset_id: str,
    *,
    client: AuthenticatedClient | Client,
    body: ApplyEffectPresetRequest | None | Unset = UNSET,
) -> Response[ApiErrorBody | ApiResponseApplyEffectResponse]:
    """`POST /api/v1/effects/:id/presets/:preset_id/apply` applies one preset.

    Args:
        id (str):
        preset_id (str):
        body (ApplyEffectPresetRequest | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ApiResponseApplyEffectResponse]
    """

    kwargs = _get_kwargs(
        id=id,
        preset_id=preset_id,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: str,
    preset_id: str,
    *,
    client: AuthenticatedClient | Client,
    body: ApplyEffectPresetRequest | None | Unset = UNSET,
) -> ApiErrorBody | ApiResponseApplyEffectResponse | None:
    """`POST /api/v1/effects/:id/presets/:preset_id/apply` applies one preset.

    Args:
        id (str):
        preset_id (str):
        body (ApplyEffectPresetRequest | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ApiResponseApplyEffectResponse
    """

    return sync_detailed(
        id=id,
        preset_id=preset_id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: str,
    preset_id: str,
    *,
    client: AuthenticatedClient | Client,
    body: ApplyEffectPresetRequest | None | Unset = UNSET,
) -> Response[ApiErrorBody | ApiResponseApplyEffectResponse]:
    """`POST /api/v1/effects/:id/presets/:preset_id/apply` applies one preset.

    Args:
        id (str):
        preset_id (str):
        body (ApplyEffectPresetRequest | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ApiResponseApplyEffectResponse]
    """

    kwargs = _get_kwargs(
        id=id,
        preset_id=preset_id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: str,
    preset_id: str,
    *,
    client: AuthenticatedClient | Client,
    body: ApplyEffectPresetRequest | None | Unset = UNSET,
) -> ApiErrorBody | ApiResponseApplyEffectResponse | None:
    """`POST /api/v1/effects/:id/presets/:preset_id/apply` applies one preset.

    Args:
        id (str):
        preset_id (str):
        body (ApplyEffectPresetRequest | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ApiResponseApplyEffectResponse
    """

    return (
        await asyncio_detailed(
            id=id,
            preset_id=preset_id,
            client=client,
            body=body,
        )
    ).parsed
