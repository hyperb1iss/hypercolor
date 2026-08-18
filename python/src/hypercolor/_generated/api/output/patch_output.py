from http import HTTPStatus
from typing import Any, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_output_resource import ApiResponseOutputResource
from ...models.output_patch_request import OutputPatchRequest
from ...types import Response


def _get_kwargs(
    *,
    body: OutputPatchRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "patch",
        "url": "/api/v1/output",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ApiResponseOutputResource | None:
    if response.status_code == 200:
        response_200 = ApiResponseOutputResource.from_dict(response.json())

        return response_200

    if response.status_code == 422:
        response_422 = cast(Any, None)
        return response_422

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | ApiResponseOutputResource]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    body: OutputPatchRequest,
) -> Response[Any | ApiResponseOutputResource]:
    """`PATCH /api/v1/output` — Set power, brightness, or both.

    Args:
        body (OutputPatchRequest): `PATCH /api/v1/output` — partial: either or both fields.

            The range bound on `brightness` is a domain rule, not a parse rule:
            the service rejects an out-of-range value as a validation error so
            the caller gets a named field back instead of a decoder complaint.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseOutputResource]
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
    body: OutputPatchRequest,
) -> Any | ApiResponseOutputResource | None:
    """`PATCH /api/v1/output` — Set power, brightness, or both.

    Args:
        body (OutputPatchRequest): `PATCH /api/v1/output` — partial: either or both fields.

            The range bound on `brightness` is a domain rule, not a parse rule:
            the service rejects an out-of-range value as a validation error so
            the caller gets a named field back instead of a decoder complaint.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseOutputResource
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    body: OutputPatchRequest,
) -> Response[Any | ApiResponseOutputResource]:
    """`PATCH /api/v1/output` — Set power, brightness, or both.

    Args:
        body (OutputPatchRequest): `PATCH /api/v1/output` — partial: either or both fields.

            The range bound on `brightness` is a domain rule, not a parse rule:
            the service rejects an out-of-range value as a validation error so
            the caller gets a named field back instead of a decoder complaint.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseOutputResource]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    body: OutputPatchRequest,
) -> Any | ApiResponseOutputResource | None:
    """`PATCH /api/v1/output` — Set power, brightness, or both.

    Args:
        body (OutputPatchRequest): `PATCH /api/v1/output` — partial: either or both fields.

            The range bound on `brightness` is a domain rule, not a parse rule:
            the service rejects an out-of-range value as a validation error so
            the caller gets a named field back instead of a decoder complaint.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseOutputResource
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
