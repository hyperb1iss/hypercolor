from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.effect_console_level import EffectConsoleLevel
from ...models.get_effect_console_response_200 import GetEffectConsoleResponse200
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    after: int | Unset = UNSET,
    limit: int | Unset = UNSET,
    effect: str | Unset = UNSET,
    level: EffectConsoleLevel | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["after"] = after

    params["limit"] = limit

    params["effect"] = effect

    json_level: str | Unset = UNSET
    if not isinstance(level, Unset):
        json_level = level.value

    params["level"] = json_level

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/effects/console",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | GetEffectConsoleResponse200 | None:
    if response.status_code == 200:
        response_200 = GetEffectConsoleResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | GetEffectConsoleResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    after: int | Unset = UNSET,
    limit: int | Unset = UNSET,
    effect: str | Unset = UNSET,
    level: EffectConsoleLevel | Unset = UNSET,
) -> Response[ApiErrorBody | GetEffectConsoleResponse200]:
    """Read effect console output

    Args:
        after (int | Unset):
        limit (int | Unset):
        effect (str | Unset):
        level (EffectConsoleLevel | Unset): Severity floor for `GET /api/v1/effects/console`.

            Raw console levels fold onto four floors: `trace` and `debug` are
            `debug`; `log`, `info`, and `dir` are `log`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | GetEffectConsoleResponse200]
    """

    kwargs = _get_kwargs(
        after=after,
        limit=limit,
        effect=effect,
        level=level,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
    after: int | Unset = UNSET,
    limit: int | Unset = UNSET,
    effect: str | Unset = UNSET,
    level: EffectConsoleLevel | Unset = UNSET,
) -> ApiErrorBody | GetEffectConsoleResponse200 | None:
    """Read effect console output

    Args:
        after (int | Unset):
        limit (int | Unset):
        effect (str | Unset):
        level (EffectConsoleLevel | Unset): Severity floor for `GET /api/v1/effects/console`.

            Raw console levels fold onto four floors: `trace` and `debug` are
            `debug`; `log`, `info`, and `dir` are `log`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | GetEffectConsoleResponse200
    """

    return sync_detailed(
        client=client,
        after=after,
        limit=limit,
        effect=effect,
        level=level,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    after: int | Unset = UNSET,
    limit: int | Unset = UNSET,
    effect: str | Unset = UNSET,
    level: EffectConsoleLevel | Unset = UNSET,
) -> Response[ApiErrorBody | GetEffectConsoleResponse200]:
    """Read effect console output

    Args:
        after (int | Unset):
        limit (int | Unset):
        effect (str | Unset):
        level (EffectConsoleLevel | Unset): Severity floor for `GET /api/v1/effects/console`.

            Raw console levels fold onto four floors: `trace` and `debug` are
            `debug`; `log`, `info`, and `dir` are `log`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | GetEffectConsoleResponse200]
    """

    kwargs = _get_kwargs(
        after=after,
        limit=limit,
        effect=effect,
        level=level,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    after: int | Unset = UNSET,
    limit: int | Unset = UNSET,
    effect: str | Unset = UNSET,
    level: EffectConsoleLevel | Unset = UNSET,
) -> ApiErrorBody | GetEffectConsoleResponse200 | None:
    """Read effect console output

    Args:
        after (int | Unset):
        limit (int | Unset):
        effect (str | Unset):
        level (EffectConsoleLevel | Unset): Severity floor for `GET /api/v1/effects/console`.

            Raw console levels fold onto four floors: `trace` and `debug` are
            `debug`; `log`, `info`, and `dir` are `log`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | GetEffectConsoleResponse200
    """

    return (
        await asyncio_detailed(
            client=client,
            after=after,
            limit=limit,
            effect=effect,
            level=level,
        )
    ).parsed
