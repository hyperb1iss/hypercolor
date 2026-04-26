"""Public API for hypercolor-python."""

from .client import HypercolorClient
from .exceptions import (
    HypercolorApiError,
    HypercolorAuthenticationError,
    HypercolorConflictError,
    HypercolorConnectionError,
    HypercolorError,
    HypercolorNotFoundError,
    HypercolorRateLimitError,
    HypercolorUnavailableError,
    HypercolorValidationError,
)
from .sync_client import SyncHypercolorClient
from .websocket import HypercolorEventStream

__all__ = [
    "HypercolorApiError",
    "HypercolorAuthenticationError",
    "HypercolorClient",
    "HypercolorConflictError",
    "HypercolorConnectionError",
    "HypercolorError",
    "HypercolorEventStream",
    "HypercolorNotFoundError",
    "HypercolorRateLimitError",
    "HypercolorUnavailableError",
    "HypercolorValidationError",
    "SyncHypercolorClient",
]

__version__ = "0.1.0"
