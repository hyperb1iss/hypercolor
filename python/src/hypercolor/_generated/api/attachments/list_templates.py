from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_error_body import ApiErrorBody
from ...models.list_templates_response_200 import ListTemplatesResponse200
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    category: None | str | Unset = UNSET,
    vendor: None | str | Unset = UNSET,
    origin: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    controller_id: None | str | Unset = UNSET,
    model: None | str | Unset = UNSET,
    slot_id: None | str | Unset = UNSET,
    led_min: int | None | Unset = UNSET,
    led_max: int | None | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_offset: int | None | Unset
    if isinstance(offset, Unset):
        json_offset = UNSET
    else:
        json_offset = offset
    params["offset"] = json_offset

    json_limit: int | None | Unset
    if isinstance(limit, Unset):
        json_limit = UNSET
    else:
        json_limit = limit
    params["limit"] = json_limit

    json_category: None | str | Unset
    if isinstance(category, Unset):
        json_category = UNSET
    else:
        json_category = category
    params["category"] = json_category

    json_vendor: None | str | Unset
    if isinstance(vendor, Unset):
        json_vendor = UNSET
    else:
        json_vendor = vendor
    params["vendor"] = json_vendor

    json_origin: None | str | Unset
    if isinstance(origin, Unset):
        json_origin = UNSET
    else:
        json_origin = origin
    params["origin"] = json_origin

    json_q: None | str | Unset
    if isinstance(q, Unset):
        json_q = UNSET
    else:
        json_q = q
    params["q"] = json_q

    json_controller_id: None | str | Unset
    if isinstance(controller_id, Unset):
        json_controller_id = UNSET
    else:
        json_controller_id = controller_id
    params["controller_id"] = json_controller_id

    json_model: None | str | Unset
    if isinstance(model, Unset):
        json_model = UNSET
    else:
        json_model = model
    params["model"] = json_model

    json_slot_id: None | str | Unset
    if isinstance(slot_id, Unset):
        json_slot_id = UNSET
    else:
        json_slot_id = slot_id
    params["slot_id"] = json_slot_id

    json_led_min: int | None | Unset
    if isinstance(led_min, Unset):
        json_led_min = UNSET
    else:
        json_led_min = led_min
    params["led_min"] = json_led_min

    json_led_max: int | None | Unset
    if isinstance(led_max, Unset):
        json_led_max = UNSET
    else:
        json_led_max = led_max
    params["led_max"] = json_led_max

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/attachments/templates",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiErrorBody | ListTemplatesResponse200 | None:
    if response.status_code == 200:
        response_200 = ListTemplatesResponse200.from_dict(response.json())

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
) -> Response[ApiErrorBody | ListTemplatesResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    category: None | str | Unset = UNSET,
    vendor: None | str | Unset = UNSET,
    origin: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    controller_id: None | str | Unset = UNSET,
    model: None | str | Unset = UNSET,
    slot_id: None | str | Unset = UNSET,
    led_min: int | None | Unset = UNSET,
    led_max: int | None | Unset = UNSET,
) -> Response[ApiErrorBody | ListTemplatesResponse200]:
    """List attachment templates

    Args:
        offset (int | None | Unset):
        limit (int | None | Unset):
        category (None | str | Unset):
        vendor (None | str | Unset):
        origin (None | str | Unset):
        q (None | str | Unset):
        controller_id (None | str | Unset):
        model (None | str | Unset):
        slot_id (None | str | Unset):
        led_min (int | None | Unset):
        led_max (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ListTemplatesResponse200]
    """

    kwargs = _get_kwargs(
        offset=offset,
        limit=limit,
        category=category,
        vendor=vendor,
        origin=origin,
        q=q,
        controller_id=controller_id,
        model=model,
        slot_id=slot_id,
        led_min=led_min,
        led_max=led_max,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    category: None | str | Unset = UNSET,
    vendor: None | str | Unset = UNSET,
    origin: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    controller_id: None | str | Unset = UNSET,
    model: None | str | Unset = UNSET,
    slot_id: None | str | Unset = UNSET,
    led_min: int | None | Unset = UNSET,
    led_max: int | None | Unset = UNSET,
) -> ApiErrorBody | ListTemplatesResponse200 | None:
    """List attachment templates

    Args:
        offset (int | None | Unset):
        limit (int | None | Unset):
        category (None | str | Unset):
        vendor (None | str | Unset):
        origin (None | str | Unset):
        q (None | str | Unset):
        controller_id (None | str | Unset):
        model (None | str | Unset):
        slot_id (None | str | Unset):
        led_min (int | None | Unset):
        led_max (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ListTemplatesResponse200
    """

    return sync_detailed(
        client=client,
        offset=offset,
        limit=limit,
        category=category,
        vendor=vendor,
        origin=origin,
        q=q,
        controller_id=controller_id,
        model=model,
        slot_id=slot_id,
        led_min=led_min,
        led_max=led_max,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    category: None | str | Unset = UNSET,
    vendor: None | str | Unset = UNSET,
    origin: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    controller_id: None | str | Unset = UNSET,
    model: None | str | Unset = UNSET,
    slot_id: None | str | Unset = UNSET,
    led_min: int | None | Unset = UNSET,
    led_max: int | None | Unset = UNSET,
) -> Response[ApiErrorBody | ListTemplatesResponse200]:
    """List attachment templates

    Args:
        offset (int | None | Unset):
        limit (int | None | Unset):
        category (None | str | Unset):
        vendor (None | str | Unset):
        origin (None | str | Unset):
        q (None | str | Unset):
        controller_id (None | str | Unset):
        model (None | str | Unset):
        slot_id (None | str | Unset):
        led_min (int | None | Unset):
        led_max (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiErrorBody | ListTemplatesResponse200]
    """

    kwargs = _get_kwargs(
        offset=offset,
        limit=limit,
        category=category,
        vendor=vendor,
        origin=origin,
        q=q,
        controller_id=controller_id,
        model=model,
        slot_id=slot_id,
        led_min=led_min,
        led_max=led_max,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    category: None | str | Unset = UNSET,
    vendor: None | str | Unset = UNSET,
    origin: None | str | Unset = UNSET,
    q: None | str | Unset = UNSET,
    controller_id: None | str | Unset = UNSET,
    model: None | str | Unset = UNSET,
    slot_id: None | str | Unset = UNSET,
    led_min: int | None | Unset = UNSET,
    led_max: int | None | Unset = UNSET,
) -> ApiErrorBody | ListTemplatesResponse200 | None:
    """List attachment templates

    Args:
        offset (int | None | Unset):
        limit (int | None | Unset):
        category (None | str | Unset):
        vendor (None | str | Unset):
        origin (None | str | Unset):
        q (None | str | Unset):
        controller_id (None | str | Unset):
        model (None | str | Unset):
        slot_id (None | str | Unset):
        led_min (int | None | Unset):
        led_max (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiErrorBody | ListTemplatesResponse200
    """

    return (
        await asyncio_detailed(
            client=client,
            offset=offset,
            limit=limit,
            category=category,
            vendor=vendor,
            origin=origin,
            q=q,
            controller_id=controller_id,
            model=model,
            slot_id=slot_id,
            led_min=led_min,
            led_max=led_max,
        )
    ).parsed
