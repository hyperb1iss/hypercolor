from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.diagnose_latest_frame_snapshot import DiagnoseLatestFrameSnapshot
    from ..models.diagnose_render_window_snapshot import DiagnoseRenderWindowSnapshot


T = TypeVar("T", bound="DiagnoseRenderSnapshot")


@_attrs_define
class DiagnoseRenderSnapshot:
    """
    Attributes:
        recent_window (DiagnoseRenderWindowSnapshot):
        latest_frame (DiagnoseLatestFrameSnapshot | None | Unset):
    """

    recent_window: DiagnoseRenderWindowSnapshot
    latest_frame: DiagnoseLatestFrameSnapshot | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.diagnose_latest_frame_snapshot import DiagnoseLatestFrameSnapshot

        recent_window = self.recent_window.to_dict()

        latest_frame: dict[str, Any] | None | Unset
        if isinstance(self.latest_frame, Unset):
            latest_frame = UNSET
        elif isinstance(self.latest_frame, DiagnoseLatestFrameSnapshot):
            latest_frame = self.latest_frame.to_dict()
        else:
            latest_frame = self.latest_frame

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "recent_window": recent_window,
            }
        )
        if latest_frame is not UNSET:
            field_dict["latest_frame"] = latest_frame

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.diagnose_latest_frame_snapshot import DiagnoseLatestFrameSnapshot
        from ..models.diagnose_render_window_snapshot import (
            DiagnoseRenderWindowSnapshot,
        )

        d = dict(src_dict)
        recent_window = DiagnoseRenderWindowSnapshot.from_dict(d.pop("recent_window"))

        def _parse_latest_frame(
            data: object,
        ) -> DiagnoseLatestFrameSnapshot | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                latest_frame_type_1 = DiagnoseLatestFrameSnapshot.from_dict(data)

                return latest_frame_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DiagnoseLatestFrameSnapshot | None | Unset, data)

        latest_frame = _parse_latest_frame(d.pop("latest_frame", UNSET))

        diagnose_render_snapshot = cls(
            recent_window=recent_window,
            latest_frame=latest_frame,
        )

        diagnose_render_snapshot.additional_properties = d
        return diagnose_render_snapshot

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
