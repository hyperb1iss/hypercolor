"""Generated multipart upload requests carry exactly the parts that are set."""

from __future__ import annotations

import io
import re
from typing import Any

import httpx

from hypercolor._generated.api.assets import upload_asset
from hypercolor._generated.api.effects import install_effect
from hypercolor._generated.models.asset_upload_form import AssetUploadForm
from hypercolor._generated.models.effect_install_form import EffectInstallForm
from hypercolor._generated.types import File

BASE_URL = "http://hyperia.test:9420"


def _wire_parts(kwargs: dict[str, Any]) -> dict[str, bytes]:
    """Encode the generated request as httpx would and read back its parts."""
    request = httpx.Request(
        kwargs["method"].upper(),
        f"{BASE_URL}{kwargs['url']}",
        params=kwargs.get("params"),
        files=kwargs["files"],
    )
    body = request.read()
    content_type = request.headers["content-type"]
    boundary = content_type.split("boundary=", 1)[1].encode()
    parts: dict[str, bytes] = {}
    for chunk in body.split(b"--" + boundary):
        head, _, value = chunk.partition(b"\r\n\r\n")
        match = re.search(rb'name="([^"]+)"', head)
        if match:
            parts[match.group(1).decode()] = value.removesuffix(b"\r\n")
    return parts


def _file(name: str, payload: bytes, mime_type: str) -> File:
    return File(payload=io.BytesIO(payload), file_name=name, mime_type=mime_type)


def test_asset_upload_with_only_a_file_sends_no_optional_parts() -> None:
    form = AssetUploadForm(file=_file("glow.png", b"\x89PNG", "image/png"))

    parts = _wire_parts(upload_asset._get_kwargs(body=form))

    assert set(parts) == {"file"}
    assert parts["file"] == b"\x89PNG"


def test_asset_upload_sends_each_optional_part_that_is_set() -> None:
    form = AssetUploadForm(
        file=_file("intro.json", b"{}", "application/json"),
        name="Intro",
        tags="ambient,loop",
        type_="lottie",
    )

    parts = _wire_parts(upload_asset._get_kwargs(body=form))

    assert set(parts) == {"file", "name", "tags", "type"}
    assert parts["name"] == b"Intro"
    assert parts["tags"] == b"ambient,loop"
    assert parts["type"] == b"lottie"


def test_effect_install_sends_only_its_file_part() -> None:
    form = EffectInstallForm(file=_file("aurora.html", b"<html></html>", "text/html"))

    kwargs = install_effect._get_kwargs(body=form)
    parts = _wire_parts(kwargs)

    assert kwargs["url"] == "/api/v1/effects/install"
    assert set(parts) == {"file"}
    assert parts["file"] == b"<html></html>"


def test_optional_asset_parts_do_not_admit_none() -> None:
    """A multipart part has no null; a None would be sent as the text "None"."""
    annotations = AssetUploadForm.__annotations__
    for field in ("name", "tags", "type_"):
        assert "None" not in str(annotations[field]), f"{field}: {annotations[field]}"
