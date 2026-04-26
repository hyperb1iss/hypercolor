"""Attachment models reserved for future API surface."""

from __future__ import annotations

import msgspec


class AttachmentTemplate(msgspec.Struct, kw_only=True):
    """Minimal attachment template placeholder."""

    id: str
    name: str
