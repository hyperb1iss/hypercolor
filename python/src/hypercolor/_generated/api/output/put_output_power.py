from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_output_power_response import ApiResponseOutputPowerResponse
from ...models.set_output_power_request import SetOutputPowerRequest
from ...types import Response


def _get_kwargs(
    *,
    body: SetOutputPowerRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/api/v1/output/power",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiResponseOutputPowerResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseOutputPowerResponse.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ApiResponseOutputPowerResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    body: SetOutputPowerRequest,
) -> Response[ApiResponseOutputPowerResponse]:
    """`PUT /api/v1/output/power` - Set desired global output power.

    Args:
        body (SetOutputPowerRequest): Request for `PUT /api/v1/output/power`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiResponseOutputPowerResponse]
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
    body: SetOutputPowerRequest,
) -> ApiResponseOutputPowerResponse | None:
    """`PUT /api/v1/output/power` - Set desired global output power.

    Args:
        body (SetOutputPowerRequest): Request for `PUT /api/v1/output/power`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiResponseOutputPowerResponse
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    body: SetOutputPowerRequest,
) -> Response[ApiResponseOutputPowerResponse]:
    """`PUT /api/v1/output/power` - Set desired global output power.

    Args:
        body (SetOutputPowerRequest): Request for `PUT /api/v1/output/power`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiResponseOutputPowerResponse]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    body: SetOutputPowerRequest,
) -> ApiResponseOutputPowerResponse | None:
    """`PUT /api/v1/output/power` - Set desired global output power.

    Args:
        body (SetOutputPowerRequest): Request for `PUT /api/v1/output/power`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiResponseOutputPowerResponse
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
