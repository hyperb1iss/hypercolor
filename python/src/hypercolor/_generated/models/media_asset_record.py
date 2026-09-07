from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.asset_scan_status import AssetScanStatus
    from ..models.asset_warning_type_0 import AssetWarningType0
    from ..models.asset_warning_type_1 import AssetWarningType1


T = TypeVar("T", bound="MediaAssetRecord")


@_attrs_define
class MediaAssetRecord:
    """Persisted metadata for one user media asset.

    Attributes:
        byte_len (int):
        created_at (str):
        hash_sha256 (str):
        id (UUID): Opaque identifier for a user media asset.
        mime_type (str):
        modified_at (str):
        name (str):
        duration_us (int | None | Unset):
        frame_count (int | None | Unset):
        intrinsic_height (int | None | Unset):
        intrinsic_width (int | None | Unset):
        scan_status (AssetScanStatus | Unset): Metadata scan state for an asset record.
        tags (list[str] | Unset):
        warnings (list[AssetWarningType0 | AssetWarningType1] | Unset):
    """

    byte_len: int
    created_at: str
    hash_sha256: str
    id: UUID
    mime_type: str
    modified_at: str
    name: str
    duration_us: int | None | Unset = UNSET
    frame_count: int | None | Unset = UNSET
    intrinsic_height: int | None | Unset = UNSET
    intrinsic_width: int | None | Unset = UNSET
    scan_status: AssetScanStatus | Unset = UNSET
    tags: list[str] | Unset = UNSET
    warnings: list[AssetWarningType0 | AssetWarningType1] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.asset_warning_type_0 import AssetWarningType0

        byte_len = self.byte_len

        created_at = self.created_at

        hash_sha256 = self.hash_sha256

        id = str(self.id)

        mime_type = self.mime_type

        modified_at = self.modified_at

        name = self.name

        duration_us: int | None | Unset
        if isinstance(self.duration_us, Unset):
            duration_us = UNSET
        else:
            duration_us = self.duration_us

        frame_count: int | None | Unset
        if isinstance(self.frame_count, Unset):
            frame_count = UNSET
        else:
            frame_count = self.frame_count

        intrinsic_height: int | None | Unset
        if isinstance(self.intrinsic_height, Unset):
            intrinsic_height = UNSET
        else:
            intrinsic_height = self.intrinsic_height

        intrinsic_width: int | None | Unset
        if isinstance(self.intrinsic_width, Unset):
            intrinsic_width = UNSET
        else:
            intrinsic_width = self.intrinsic_width

        scan_status: dict[str, Any] | Unset = UNSET
        if not isinstance(self.scan_status, Unset):
            scan_status = self.scan_status.to_dict()

        tags: list[str] | Unset = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        warnings: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.warnings, Unset):
            warnings = []
            for warnings_item_data in self.warnings:
                warnings_item: dict[str, Any]
                if isinstance(warnings_item_data, AssetWarningType0):
                    warnings_item = warnings_item_data.to_dict()
                else:
                    warnings_item = warnings_item_data.to_dict()

                warnings.append(warnings_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "byte_len": byte_len,
                "created_at": created_at,
                "hash_sha256": hash_sha256,
                "id": id,
                "mime_type": mime_type,
                "modified_at": modified_at,
                "name": name,
            }
        )
        if duration_us is not UNSET:
            field_dict["duration_us"] = duration_us
        if frame_count is not UNSET:
            field_dict["frame_count"] = frame_count
        if intrinsic_height is not UNSET:
            field_dict["intrinsic_height"] = intrinsic_height
        if intrinsic_width is not UNSET:
            field_dict["intrinsic_width"] = intrinsic_width
        if scan_status is not UNSET:
            field_dict["scan_status"] = scan_status
        if tags is not UNSET:
            field_dict["tags"] = tags
        if warnings is not UNSET:
            field_dict["warnings"] = warnings

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.asset_scan_status import AssetScanStatus
        from ..models.asset_warning_type_0 import AssetWarningType0
        from ..models.asset_warning_type_1 import AssetWarningType1

        d = dict(src_dict)
        byte_len = d.pop("byte_len")

        created_at = d.pop("created_at")

        hash_sha256 = d.pop("hash_sha256")

        id = UUID(d.pop("id"))

        mime_type = d.pop("mime_type")

        modified_at = d.pop("modified_at")

        name = d.pop("name")

        def _parse_duration_us(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        duration_us = _parse_duration_us(d.pop("duration_us", UNSET))

        def _parse_frame_count(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        frame_count = _parse_frame_count(d.pop("frame_count", UNSET))

        def _parse_intrinsic_height(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        intrinsic_height = _parse_intrinsic_height(d.pop("intrinsic_height", UNSET))

        def _parse_intrinsic_width(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        intrinsic_width = _parse_intrinsic_width(d.pop("intrinsic_width", UNSET))

        _scan_status = d.pop("scan_status", UNSET)
        scan_status: AssetScanStatus | Unset
        if isinstance(_scan_status, Unset):
            scan_status = UNSET
        else:
            scan_status = AssetScanStatus.from_dict(_scan_status)

        tags = cast(list[str], d.pop("tags", UNSET))

        _warnings = d.pop("warnings", UNSET)
        warnings: list[AssetWarningType0 | AssetWarningType1] | Unset = UNSET
        if _warnings is not UNSET:
            warnings = []
            for warnings_item_data in _warnings:

                def _parse_warnings_item(
                    data: object,
                ) -> AssetWarningType0 | AssetWarningType1:
                    try:
                        if not isinstance(data, dict):
                            raise TypeError()
                        componentsschemas_asset_warning_type_0 = (
                            AssetWarningType0.from_dict(data)
                        )

                        return componentsschemas_asset_warning_type_0
                    except (TypeError, ValueError, AttributeError, KeyError):
                        pass
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_asset_warning_type_1 = (
                        AssetWarningType1.from_dict(data)
                    )

                    return componentsschemas_asset_warning_type_1

                warnings_item = _parse_warnings_item(warnings_item_data)

                warnings.append(warnings_item)

        media_asset_record = cls(
            byte_len=byte_len,
            created_at=created_at,
            hash_sha256=hash_sha256,
            id=id,
            mime_type=mime_type,
            modified_at=modified_at,
            name=name,
            duration_us=duration_us,
            frame_count=frame_count,
            intrinsic_height=intrinsic_height,
            intrinsic_width=intrinsic_width,
            scan_status=scan_status,
            tags=tags,
            warnings=warnings,
        )

        media_asset_record.additional_properties = d
        return media_asset_record

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
