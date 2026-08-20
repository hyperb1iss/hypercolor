"""Effect models."""

from __future__ import annotations

import msgspec


class EffectCoverImage(msgspec.Struct, kw_only=True):
    """Binary cover image payload for an effect."""

    data: bytes
    content_type: str
    url: str
