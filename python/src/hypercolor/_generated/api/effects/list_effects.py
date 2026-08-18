from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.api_response_effect_list_response import ApiResponseEffectListResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    category: None | str | Unset = UNSET,
    audio_reactive: bool | None | Unset = UNSET,
    screen_reactive: bool | None | Unset = UNSET,
    input_reactive: bool | None | Unset = UNSET,
    source: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    include: None | str | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_category: None | str | Unset
    if isinstance(category, Unset):
        json_category = UNSET
    else:
        json_category = category
    params["category"] = json_category

    json_audio_reactive: bool | None | Unset
    if isinstance(audio_reactive, Unset):
        json_audio_reactive = UNSET
    else:
        json_audio_reactive = audio_reactive
    params["audio_reactive"] = json_audio_reactive

    json_screen_reactive: bool | None | Unset
    if isinstance(screen_reactive, Unset):
        json_screen_reactive = UNSET
    else:
        json_screen_reactive = screen_reactive
    params["screen_reactive"] = json_screen_reactive

    json_input_reactive: bool | None | Unset
    if isinstance(input_reactive, Unset):
        json_input_reactive = UNSET
    else:
        json_input_reactive = input_reactive
    params["input_reactive"] = json_input_reactive

    json_source: None | str | Unset
    if isinstance(source, Unset):
        json_source = UNSET
    else:
        json_source = source
    params["source"] = json_source

    json_q: None | str | Unset
    if isinstance(q, Unset):
        json_q = UNSET
    else:
        json_q = q
    params["q"] = json_q

    json_include: None | str | Unset
    if isinstance(include, Unset):
        json_include = UNSET
    else:
        json_include = include
    params["include"] = json_include

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/effects",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | ApiResponseEffectListResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseEffectListResponse.from_dict(response.json())

        return response_200

    if response.status_code == 422:
        response_422 = ApiErrorBody.from_dict(response.json())

        return response_422

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ApiErrorBody | ApiResponseEffectListResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    category: None | str | Unset = UNSET,
    audio_reactive: bool | None | Unset = UNSET,
    screen_reactive: bool | None | Unset = UNSET,
    input_reactive: bool | None | Unset = UNSET,
    source: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    include: None | str | Unset = UNSET,
) -> Response[ApiErrorBody | ApiResponseEffectListResponse]:
    """`GET /api/v1/effects` — the effect catalog, narrowed server-side.

    Args:
        category (None | str | Unset):
        audio_reactive (bool | None | Unset):
        screen_reactive (bool | None | Unset):
        input_reactive (bool | None | Unset):
        source (None | str | Unset):
        q (None | str | Unset):
        include (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ApiResponseEffectListResponse]
    """

    kwargs = _get_kwargs(
        category=category,
        audio_reactive=audio_reactive,
        screen_reactive=screen_reactive,
        input_reactive=input_reactive,
        source=source,
        q=q,
        include=include,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
    category: None | str | Unset = UNSET,
    audio_reactive: bool | None | Unset = UNSET,
    screen_reactive: bool | None | Unset = UNSET,
    input_reactive: bool | None | Unset = UNSET,
    source: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    include: None | str | Unset = UNSET,
) -> ApiErrorBody | ApiResponseEffectListResponse | None:
    """`GET /api/v1/effects` — the effect catalog, narrowed server-side.

    Args:
        category (None | str | Unset):
        audio_reactive (bool | None | Unset):
        screen_reactive (bool | None | Unset):
        input_reactive (bool | None | Unset):
        source (None | str | Unset):
        q (None | str | Unset):
        include (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ApiResponseEffectListResponse
    """

    return sync_detailed(
        client=client,
        category=category,
        audio_reactive=audio_reactive,
        screen_reactive=screen_reactive,
        input_reactive=input_reactive,
        source=source,
        q=q,
        include=include,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    category: None | str | Unset = UNSET,
    audio_reactive: bool | None | Unset = UNSET,
    screen_reactive: bool | None | Unset = UNSET,
    input_reactive: bool | None | Unset = UNSET,
    source: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    include: None | str | Unset = UNSET,
) -> Response[ApiErrorBody | ApiResponseEffectListResponse]:
    """`GET /api/v1/effects` — the effect catalog, narrowed server-side.

    Args:
        category (None | str | Unset):
        audio_reactive (bool | None | Unset):
        screen_reactive (bool | None | Unset):
        input_reactive (bool | None | Unset):
        source (None | str | Unset):
        q (None | str | Unset):
        include (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ApiResponseEffectListResponse]
    """

    kwargs = _get_kwargs(
        category=category,
        audio_reactive=audio_reactive,
        screen_reactive=screen_reactive,
        input_reactive=input_reactive,
        source=source,
        q=q,
        include=include,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    category: None | str | Unset = UNSET,
    audio_reactive: bool | None | Unset = UNSET,
    screen_reactive: bool | None | Unset = UNSET,
    input_reactive: bool | None | Unset = UNSET,
    source: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    include: None | str | Unset = UNSET,
) -> ApiErrorBody | ApiResponseEffectListResponse | None:
    """`GET /api/v1/effects` — the effect catalog, narrowed server-side.

    Args:
        category (None | str | Unset):
        audio_reactive (bool | None | Unset):
        screen_reactive (bool | None | Unset):
        input_reactive (bool | None | Unset):
        source (None | str | Unset):
        q (None | str | Unset):
        include (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ApiResponseEffectListResponse
    """

    return (
        await asyncio_detailed(
            client=client,
            category=category,
            audio_reactive=audio_reactive,
            screen_reactive=screen_reactive,
            input_reactive=input_reactive,
            source=source,
            q=q,
            include=include,
        )
    ).parsed
