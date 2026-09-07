from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.source_diagnostics_display_field import SourceDiagnosticsDisplayField


T = TypeVar("T", bound="SourceDiagnosticsEnvelope")


@_attrs_define
class SourceDiagnosticsEnvelope:
    """A versioned payload whose semantics remain owned by its platform crate.

    Attributes:
        display (list[SourceDiagnosticsDisplayField]):
        payload (Any): Opaque platform JSON bounded to 16384 serialized UTF-8 bytes.
        schema (str):
        version (int):
    """

    display: list[SourceDiagnosticsDisplayField]
    payload: Any
    schema: str
    version: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        display = []
        for display_item_data in self.display:
            display_item = display_item_data.to_dict()
            display.append(display_item)

        payload = self.payload

        schema = self.schema

        version = self.version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "display": display,
                "payload": payload,
                "schema": schema,
                "version": version,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.source_diagnostics_display_field import (
            SourceDiagnosticsDisplayField,
        )

        d = dict(src_dict)
        display = []
        _display = d.pop("display")
        for display_item_data in _display:
            display_item = SourceDiagnosticsDisplayField.from_dict(display_item_data)

            display.append(display_item)

        payload = d.pop("payload")

        schema = d.pop("schema")

        version = d.pop("version")

        source_diagnostics_envelope = cls(
            display=display,
            payload=payload,
            schema=schema,
            version=version,
        )

        source_diagnostics_envelope.additional_properties = d
        return source_diagnostics_envelope

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
