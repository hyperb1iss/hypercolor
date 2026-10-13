from __future__ import annotations

from collections.abc import Mapping
from io import BytesIO
from typing import Any, TypeVar, cast

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

        Attributes:
            file (File): The asset bytes, sent as a file part. Required.
            name (None | str | Unset): Display name; defaults to the uploaded file name.
            tags (None | str | Unset): Tags, comma separated or as a JSON array of strings.
            type_ (None | str | Unset): Asset type hint (`lottie` or `stream`), overriding the `type` query
                parameter and content sniffing.
    """

    file: File
    name: None | str | Unset = UNSET
    tags: None | str | Unset = UNSET
    type_: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        file = self.file.to_tuple()

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        tags: None | str | Unset
        if isinstance(self.tags, Unset):
            tags = UNSET
        else:
            tags = self.tags

        type_: None | str | Unset
        if isinstance(self.type_, Unset):
            type_ = UNSET
        else:
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
            if isinstance(self.name, str):
                files.append(("name", (None, str(self.name).encode(), "text/plain")))
            else:
                files.append(("name", (None, str(self.name).encode(), "text/plain")))

        if not isinstance(self.tags, Unset):
            if isinstance(self.tags, str):
                files.append(("tags", (None, str(self.tags).encode(), "text/plain")))
            else:
                files.append(("tags", (None, str(self.tags).encode(), "text/plain")))

        if not isinstance(self.type_, Unset):
            if isinstance(self.type_, str):
                files.append(("type", (None, str(self.type_).encode(), "text/plain")))
            else:
                files.append(("type", (None, str(self.type_).encode(), "text/plain")))

        for prop_name, prop in self.additional_properties.items():
            files.append((prop_name, (None, str(prop).encode(), "text/plain")))

        return files

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        file = File(payload=BytesIO(d.pop("file")))

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        def _parse_tags(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        tags = _parse_tags(d.pop("tags", UNSET))

        def _parse_type_(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        type_ = _parse_type_(d.pop("type", UNSET))

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
