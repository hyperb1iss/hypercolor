from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.diagnose_latest_frame_snapshot import DiagnoseLatestFrameSnapshot
    from ..models.diagnose_render_window_snapshot import DiagnoseRenderWindowSnapshot
    from ..models.render_output_demand_status import RenderOutputDemandStatus
    from ..models.render_supervision_status import RenderSupervisionStatus


T = TypeVar("T", bound="DiagnoseRenderSnapshot")


@_attrs_define
class DiagnoseRenderSnapshot:
    """
    Attributes:
        recent_window (DiagnoseRenderWindowSnapshot):
        latest_frame (DiagnoseLatestFrameSnapshot | None | Unset):
        output_demand (RenderOutputDemandStatus | Unset): Output demand: what consumes the render loop's frames.

            The render loop renders only while something consumes its output: a
            connected device, a display, a preview or canvas stream, a capture, or
            an explicit demand hold. With none of those it parks, renders nothing,
            and resumes at its full tier the moment a consumer appears.
        supervision (RenderSupervisionStatus | Unset): Health and recovery state of the supervised render thread.

            The render thread restarts its pipeline in-process after a panic or an
            error exit, within a bounded budget, and the daemon exits so its
            service manager restarts it once that budget runs out or the compositor
            GPU device is lost.
    """

    recent_window: DiagnoseRenderWindowSnapshot
    latest_frame: DiagnoseLatestFrameSnapshot | None | Unset = UNSET
    output_demand: RenderOutputDemandStatus | Unset = UNSET
    supervision: RenderSupervisionStatus | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.diagnose_latest_frame_snapshot import (
            DiagnoseLatestFrameSnapshot,
        )

        recent_window = self.recent_window.to_dict()

        latest_frame: dict[str, Any] | None | Unset
        if isinstance(self.latest_frame, Unset):
            latest_frame = UNSET
        elif isinstance(self.latest_frame, DiagnoseLatestFrameSnapshot):
            latest_frame = self.latest_frame.to_dict()
        else:
            latest_frame = self.latest_frame

        output_demand: dict[str, Any] | Unset = UNSET
        if not isinstance(self.output_demand, Unset):
            output_demand = self.output_demand.to_dict()

        supervision: dict[str, Any] | Unset = UNSET
        if not isinstance(self.supervision, Unset):
            supervision = self.supervision.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "recent_window": recent_window,
            }
        )
        if latest_frame is not UNSET:
            field_dict["latest_frame"] = latest_frame
        if output_demand is not UNSET:
            field_dict["output_demand"] = output_demand
        if supervision is not UNSET:
            field_dict["supervision"] = supervision

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.diagnose_latest_frame_snapshot import (
            DiagnoseLatestFrameSnapshot,
        )
        from ..models.diagnose_render_window_snapshot import (
            DiagnoseRenderWindowSnapshot,
        )
        from ..models.render_output_demand_status import (
            RenderOutputDemandStatus,
        )
        from ..models.render_supervision_status import (
            RenderSupervisionStatus,
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
                latest_frame_type_0 = DiagnoseLatestFrameSnapshot.from_dict(data)

                return latest_frame_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DiagnoseLatestFrameSnapshot | None | Unset, data)

        latest_frame = _parse_latest_frame(d.pop("latest_frame", UNSET))

        _output_demand = d.pop("output_demand", UNSET)
        output_demand: RenderOutputDemandStatus | Unset
        if isinstance(_output_demand, Unset):
            output_demand = UNSET
        else:
            output_demand = RenderOutputDemandStatus.from_dict(_output_demand)

        _supervision = d.pop("supervision", UNSET)
        supervision: RenderSupervisionStatus | Unset
        if isinstance(_supervision, Unset):
            supervision = UNSET
        else:
            supervision = RenderSupervisionStatus.from_dict(_supervision)

        diagnose_render_snapshot = cls(
            recent_window=recent_window,
            latest_frame=latest_frame,
            output_demand=output_demand,
            supervision=supervision,
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
