from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.control_action_status import ControlActionStatus
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_response_control_action_result_data_result_type_0 import (
        ApiResponseControlActionResultDataResultType0,
    )


T = TypeVar("T", bound="ApiResponseControlActionResultData")


@_attrs_define
class ApiResponseControlActionResultData:
    """Result from invoking an action.

    Attributes:
        action_id (str):
        revision (int):
        status (ControlActionStatus): Action execution status.
        surface_id (str):
        result (ApiResponseControlActionResultDataResultType0 | None | Unset): Optional typed result.
    """

    action_id: str
    revision: int
    status: ControlActionStatus
    surface_id: str
    result: ApiResponseControlActionResultDataResultType0 | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.api_response_control_action_result_data_result_type_0 import (
            ApiResponseControlActionResultDataResultType0,
        )

        action_id = self.action_id

        revision = self.revision

        status = self.status.value

        surface_id = self.surface_id

        result: dict[str, Any] | None | Unset
        if isinstance(self.result, Unset):
            result = UNSET
        elif isinstance(self.result, ApiResponseControlActionResultDataResultType0):
            result = self.result.to_dict()
        else:
            result = self.result

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_id": action_id,
                "revision": revision,
                "status": status,
                "surface_id": surface_id,
            }
        )
        if result is not UNSET:
            field_dict["result"] = result

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_response_control_action_result_data_result_type_0 import (
            ApiResponseControlActionResultDataResultType0,
        )

        d = dict(src_dict)
        action_id = d.pop("action_id")

        revision = d.pop("revision")

        status = ControlActionStatus(d.pop("status"))

        surface_id = d.pop("surface_id")

        def _parse_result(
            data: object,
        ) -> ApiResponseControlActionResultDataResultType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                result_type_0 = ApiResponseControlActionResultDataResultType0.from_dict(
                    data
                )

                return result_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ApiResponseControlActionResultDataResultType0 | None | Unset, data
            )

        result = _parse_result(d.pop("result", UNSET))

        api_response_control_action_result_data = cls(
            action_id=action_id,
            revision=revision,
            status=status,
            surface_id=surface_id,
            result=result,
        )

        api_response_control_action_result_data.additional_properties = d
        return api_response_control_action_result_data

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
