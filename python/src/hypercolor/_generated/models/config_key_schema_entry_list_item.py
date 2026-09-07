from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.protection import Protection
from ..models.redaction import Redaction
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.apply_policy_type_0 import ApplyPolicyType0
    from ..models.apply_policy_type_1 import ApplyPolicyType1
    from ..models.apply_policy_type_2 import ApplyPolicyType2
    from ..models.apply_policy_type_3 import ApplyPolicyType3
    from ..models.apply_policy_type_4 import ApplyPolicyType4


T = TypeVar("T", bound="ConfigKeySchemaEntryListItem")


@_attrs_define
class ConfigKeySchemaEntryListItem:
    """One schema row as served to clients (`GET /config/schema`,
    wave 4.3) — the wire projection of a descriptor.

        Attributes:
            apply (ApplyPolicyType0 | ApplyPolicyType1 | ApplyPolicyType2 | ApplyPolicyType3 | ApplyPolicyType4): How a
                change to a key takes effect.
            has_validator (bool): Whether the daemon runs extra validation beyond type checking.
            pattern (str): The pattern text: an exact key, a section root, a namespace
                root suffixed `.*`, or `*` for the extensions catch-all.
            redaction (Redaction): How a key's value renders on read surfaces (config GET, schema,
                events).
            protection (Protection | Unset): Which writes to a row need a protected-control credential.

                Protected controls start or retarget a consented capture stream,
                so they stay behind an API key even on a keyless install; tuning an
                already-consented stream does not.
    """

    apply: (
        ApplyPolicyType0
        | ApplyPolicyType1
        | ApplyPolicyType2
        | ApplyPolicyType3
        | ApplyPolicyType4
    )
    has_validator: bool
    pattern: str
    redaction: Redaction
    protection: Protection | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.apply_policy_type_0 import ApplyPolicyType0
        from ..models.apply_policy_type_1 import ApplyPolicyType1
        from ..models.apply_policy_type_2 import ApplyPolicyType2
        from ..models.apply_policy_type_3 import ApplyPolicyType3

        apply: dict[str, Any]
        if isinstance(self.apply, ApplyPolicyType0):
            apply = self.apply.to_dict()
        elif isinstance(self.apply, ApplyPolicyType1):
            apply = self.apply.to_dict()
        elif isinstance(self.apply, ApplyPolicyType2):
            apply = self.apply.to_dict()
        elif isinstance(self.apply, ApplyPolicyType3):
            apply = self.apply.to_dict()
        else:
            apply = self.apply.to_dict()

        has_validator = self.has_validator

        pattern = self.pattern

        redaction = self.redaction.value

        protection: str | Unset = UNSET
        if not isinstance(self.protection, Unset):
            protection = self.protection.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "apply": apply,
                "has_validator": has_validator,
                "pattern": pattern,
                "redaction": redaction,
            }
        )
        if protection is not UNSET:
            field_dict["protection"] = protection

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.apply_policy_type_0 import ApplyPolicyType0
        from ..models.apply_policy_type_1 import ApplyPolicyType1
        from ..models.apply_policy_type_2 import ApplyPolicyType2
        from ..models.apply_policy_type_3 import ApplyPolicyType3
        from ..models.apply_policy_type_4 import ApplyPolicyType4

        d = dict(src_dict)

        def _parse_apply(
            data: object,
        ) -> (
            ApplyPolicyType0
            | ApplyPolicyType1
            | ApplyPolicyType2
            | ApplyPolicyType3
            | ApplyPolicyType4
        ):
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_apply_policy_type_0 = ApplyPolicyType0.from_dict(data)

                return componentsschemas_apply_policy_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_apply_policy_type_1 = ApplyPolicyType1.from_dict(data)

                return componentsschemas_apply_policy_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_apply_policy_type_2 = ApplyPolicyType2.from_dict(data)

                return componentsschemas_apply_policy_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_apply_policy_type_3 = ApplyPolicyType3.from_dict(data)

                return componentsschemas_apply_policy_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_apply_policy_type_4 = ApplyPolicyType4.from_dict(data)

            return componentsschemas_apply_policy_type_4

        apply = _parse_apply(d.pop("apply"))

        has_validator = d.pop("has_validator")

        pattern = d.pop("pattern")

        redaction = Redaction(d.pop("redaction"))

        _protection = d.pop("protection", UNSET)
        protection: Protection | Unset
        if isinstance(_protection, Unset):
            protection = UNSET
        else:
            protection = Protection(_protection)

        config_key_schema_entry_list_item = cls(
            apply=apply,
            has_validator=has_validator,
            pattern=pattern,
            redaction=redaction,
            protection=protection,
        )

        config_key_schema_entry_list_item.additional_properties = d
        return config_key_schema_entry_list_item

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
