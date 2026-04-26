"""Shared pytest fixtures for hypercolor-python."""

from __future__ import annotations

from collections.abc import AsyncIterator

import pytest

from hypercolor.client import HypercolorClient


@pytest.fixture
async def client() -> AsyncIterator[HypercolorClient]:
    """Create a disposable async client for tests."""
    async with HypercolorClient(host="hyperia.test", port=9420) as hypercolor_client:
        yield hypercolor_client
