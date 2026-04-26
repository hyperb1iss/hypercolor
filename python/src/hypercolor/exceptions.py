"""Exception hierarchy for Hypercolor."""

from __future__ import annotations

from typing import Any

import msgspec


class ApiErrorDetails(msgspec.Struct, kw_only=True):
    """Structured API error details."""

    code: str
    message: str
    details: dict[str, Any] | None = None


class HypercolorError(Exception):
    """Base exception for all client-side and daemon-side failures."""

    def __init__(
        self,
        message: str,
        *,
        error: ApiErrorDetails | None = None,
        status_code: int | None = None,
    ) -> None:
        super().__init__(message)
        self.error = error
        self.status_code = status_code

    @property
    def code(self) -> str | None:
        """Return the daemon error code if one exists."""
        return self.error.code if self.error is not None else None


class HypercolorConnectionError(HypercolorError):
    """Raised when the client cannot reach the daemon."""


class HypercolorAuthenticationError(HypercolorError):
    """Raised when authentication fails."""


class HypercolorNotFoundError(HypercolorError):
    """Raised when a resource does not exist."""


class HypercolorValidationError(HypercolorError):
    """Raised when a request body or query string is invalid."""


class HypercolorRateLimitError(HypercolorError):
    """Raised when the daemon rejects a request due to rate limiting."""


class HypercolorConflictError(HypercolorError):
    """Raised when a request conflicts with current daemon state."""


class HypercolorUnavailableError(HypercolorError):
    """Raised when the daemon is starting up or otherwise unavailable."""


class HypercolorApiError(HypercolorError):
    """Raised for daemon errors without a more specific mapping."""


ConnectionError = HypercolorConnectionError
AuthenticationError = HypercolorAuthenticationError
NotFoundError = HypercolorNotFoundError
ValidationError = HypercolorValidationError
RateLimitError = HypercolorRateLimitError
ConflictError = HypercolorConflictError
