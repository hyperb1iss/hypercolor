"""Public API for hypercolor-python."""

from .client import HypercolorClient, scene_fence
from .exceptions import (
    HypercolorApiError,
    HypercolorAuthenticationError,
    HypercolorConflictError,
    HypercolorConnectionError,
    HypercolorError,
    HypercolorNotFoundError,
    HypercolorPreconditionError,
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
    "HypercolorPreconditionError",
    "HypercolorRateLimitError",
    "HypercolorUnavailableError",
    "HypercolorValidationError",
    "SyncHypercolorClient",
    "scene_fence",
]

__version__ = "0.7.3"
