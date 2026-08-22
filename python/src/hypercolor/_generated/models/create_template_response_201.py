from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.response_meta import ResponseMeta
    from ..models.template_detail import TemplateDetail


T = TypeVar("T", bound="CreateTemplateResponse201")


@_attrs_define
class CreateTemplateResponse201:
    """
    Attributes:
        data (TemplateDetail): Response for `POST /api/v1/attachments/templates`, the created
            template.

            The summary's fields plus everything needed to place the attachment:
            `led_positions` is expanded from the topology at request time, so it
            is present here but never in the listing.

            The item routes that also return this body today (`GET` and `PUT` on
            `/attachments/templates/{id}`) are deleted in wave 78.5, which leaves
            creation as the only caller. The type is chartered on creation for
            that reason, and the collection listing keeps its own summary shape.
        meta (ResponseMeta): Response metadata included in every envelope.
    """

    data: TemplateDetail
    meta: ResponseMeta
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        data = self.data.to_dict()

        meta = self.meta.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "data": data,
                "meta": meta,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.response_meta import ResponseMeta
        from ..models.template_detail import TemplateDetail

        d = dict(src_dict)
        data = TemplateDetail.from_dict(d.pop("data"))

        meta = ResponseMeta.from_dict(d.pop("meta"))

        create_template_response_201 = cls(
            data=data,
            meta=meta,
        )

        create_template_response_201.additional_properties = d
        return create_template_response_201

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
