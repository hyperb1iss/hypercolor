from __future__ import annotations

from collections.abc import Mapping
from io import BytesIO
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from .. import types
from ..types import UNSET, File, Unset

T = TypeVar("T", bound="AssetUploadForm")


@_attrs_define
class AssetUploadForm:
    """The `multipart/form-data` fields `POST /api/v1/assets` reads.

    This documents the wire form for clients and the OpenAPI document; the
    daemon reads the parts one by one rather than deserializing this type.
    The optional parts are documented as absent-or-string, never null: a
    multipart part has no null, and a generated client that sent one would
    send the literal text.

        Attributes:
            file (File): The asset bytes, sent as a file part. Required.
            name (str | Unset): Display name; defaults to the uploaded file name. Omit the part
                rather than sending an empty or null value.
            tags (str | Unset): Tags, comma separated or as a JSON array of strings. Omit the part
                when there are none.
            type_ (str | Unset): Asset type hint (`lottie` or `stream`), overriding the `type` query
                parameter and content sniffing. Omit the part to let the daemon
                sniff.
    """

    file: File
    name: str | Unset = UNSET
    tags: str | Unset = UNSET
    type_: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        file = self.file.to_tuple()

        name = self.name

        tags = self.tags

        type_ = self.type_

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "file": file,
            }
        )
        if name is not UNSET:
            field_dict["name"] = name
        if tags is not UNSET:
            field_dict["tags"] = tags
        if type_ is not UNSET:
            field_dict["type"] = type_

        return field_dict

    def to_multipart(self) -> types.RequestFiles:
        files: types.RequestFiles = []

        files.append(("file", self.file.to_tuple()))

        if not isinstance(self.name, Unset):
            files.append(("name", (None, str(self.name).encode(), "text/plain")))

        if not isinstance(self.tags, Unset):
            files.append(("tags", (None, str(self.tags).encode(), "text/plain")))

        if not isinstance(self.type_, Unset):
            files.append(("type", (None, str(self.type_).encode(), "text/plain")))

        for prop_name, prop in self.additional_properties.items():
            files.append((prop_name, (None, str(prop).encode(), "text/plain")))

        return files

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        file = File(payload=BytesIO(d.pop("file")))

        name = d.pop("name", UNSET)

        tags = d.pop("tags", UNSET)

        type_ = d.pop("type", UNSET)

        asset_upload_form = cls(
            file=file,
            name=name,
            tags=tags,
            type_=type_,
        )

        asset_upload_form.additional_properties = d
        return asset_upload_form

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
