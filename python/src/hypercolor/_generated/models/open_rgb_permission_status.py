from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="OpenRgbPermissionStatus")


@_attrs_define
class OpenRgbPermissionStatus:
    """A host prerequisite and its actionable remedy.

    Attributes:
        detail (str):
        id (str):
        ok (bool):
        remedy (None | str | Unset):
    """

    detail: str
    id: str
    ok: bool
    remedy: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        detail = self.detail

        id = self.id

        ok = self.ok

        remedy: None | str | Unset
        if isinstance(self.remedy, Unset):
            remedy = UNSET
        else:
            remedy = self.remedy

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "detail": detail,
                "id": id,
                "ok": ok,
            }
        )
        if remedy is not UNSET:
            field_dict["remedy"] = remedy

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        detail = d.pop("detail")

        id = d.pop("id")

        ok = d.pop("ok")

        def _parse_remedy(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        remedy = _parse_remedy(d.pop("remedy", UNSET))

        open_rgb_permission_status = cls(
            detail=detail,
            id=id,
            ok=ok,
            remedy=remedy,
        )

        open_rgb_permission_status.additional_properties = d
        return open_rgb_permission_status

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
