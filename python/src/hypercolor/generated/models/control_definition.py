from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.control_kind_type_0 import ControlKindType0
from ..models.control_kind_type_1 import ControlKindType1
from ..models.control_kind_type_2 import ControlKindType2
from ..models.control_kind_type_3 import ControlKindType3
from ..models.control_kind_type_4 import ControlKindType4
from ..models.control_kind_type_5 import ControlKindType5
from ..models.control_kind_type_6 import ControlKindType6
from ..models.control_kind_type_7 import ControlKindType7
from ..models.control_kind_type_8 import ControlKindType8
from ..models.control_type import ControlType
from ..models.preview_source import PreviewSource
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.control_binding import ControlBinding
    from ..models.control_kind_type_9 import ControlKindType9
    from ..models.control_value_type_0 import ControlValueType0
    from ..models.control_value_type_1 import ControlValueType1
    from ..models.control_value_type_2 import ControlValueType2
    from ..models.control_value_type_3 import ControlValueType3
    from ..models.control_value_type_4 import ControlValueType4
    from ..models.control_value_type_5 import ControlValueType5
    from ..models.control_value_type_6 import ControlValueType6
    from ..models.control_value_type_7 import ControlValueType7


T = TypeVar("T", bound="ControlDefinition")


@_attrs_define
class ControlDefinition:
    """A single user-facing parameter declared by an effect.

    The UI auto-generates widgets from these definitions. The engine
    injects current values into the active renderer every frame.

        Attributes:
            control_type (ControlType): Widget kind for a user-facing effect control.

                Each variant maps to a specific UI component in the control panel.
            default_value (ControlValueType0 | ControlValueType1 | ControlValueType2 | ControlValueType3 | ControlValueType4
                | ControlValueType5 | ControlValueType6 | ControlValueType7): Runtime value of a control parameter.

                The variant must be compatible with the corresponding [`ControlType`]:

                | `ControlType`    | Valid `ControlValue`       |
                |------------------|----------------------------|
                | `Slider`         | `Float(f32)`               |
                | `Toggle`         | `Boolean(bool)`            |
                | `ColorPicker`    | `Color([f32; 4])`          |
                | `GradientEditor` | `Gradient(Vec<GradientStop>)` |
                | `Dropdown`       | `Enum(String)`             |
                | `TextInput`      | `Text(String)`             |
                | `Rect`           | `Rect(ViewportRect)`       |
            name (str): Human-readable label shown in the control panel.
            aspect_lock (float | None | Unset): Optional fixed aspect ratio (`width / height`) for rect controls.
            binding (ControlBinding | None | Unset):
            group (None | str | Unset): Optional grouping for the control panel UI.
            id (str | Unset): Stable control identifier used in API payloads and renderer globals.
            kind (ControlKindType0 | ControlKindType1 | ControlKindType2 | ControlKindType3 | ControlKindType4 |
                ControlKindType5 | ControlKindType6 | ControlKindType7 | ControlKindType8 | ControlKindType9 | Unset): Semantic
                control kind declared by an effect source.

                This keeps `LightScript` metadata semantics intact even when
                multiple kinds map to the same UI widget type.
            labels (list[str] | Unset): Labels for `Dropdown` options.
            max_ (float | None | Unset): Maximum numeric bound (applicable to `Slider` controls).
            min_ (float | None | Unset): Minimum numeric bound (applicable to `Slider` controls).
            preview_source (None | PreviewSource | Unset):
            step (float | None | Unset): Step increment for numeric controls. `None` means continuous.
            tooltip (None | str | Unset): Help text shown on hover/focus.
    """

    control_type: ControlType
    default_value: (
        ControlValueType0
        | ControlValueType1
        | ControlValueType2
        | ControlValueType3
        | ControlValueType4
        | ControlValueType5
        | ControlValueType6
        | ControlValueType7
    )
    name: str
    aspect_lock: float | None | Unset = UNSET
    binding: ControlBinding | None | Unset = UNSET
    group: None | str | Unset = UNSET
    id: str | Unset = UNSET
    kind: (
        ControlKindType0
        | ControlKindType1
        | ControlKindType2
        | ControlKindType3
        | ControlKindType4
        | ControlKindType5
        | ControlKindType6
        | ControlKindType7
        | ControlKindType8
        | ControlKindType9
        | Unset
    ) = UNSET
    labels: list[str] | Unset = UNSET
    max_: float | None | Unset = UNSET
    min_: float | None | Unset = UNSET
    preview_source: None | PreviewSource | Unset = UNSET
    step: float | None | Unset = UNSET
    tooltip: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.control_binding import ControlBinding
        from ..models.control_value_type_0 import ControlValueType0
        from ..models.control_value_type_1 import ControlValueType1
        from ..models.control_value_type_2 import ControlValueType2
        from ..models.control_value_type_3 import ControlValueType3
        from ..models.control_value_type_4 import ControlValueType4
        from ..models.control_value_type_5 import ControlValueType5
        from ..models.control_value_type_6 import ControlValueType6

        control_type = self.control_type.value

        default_value: dict[str, Any]
        if isinstance(self.default_value, ControlValueType0):
            default_value = self.default_value.to_dict()
        elif isinstance(self.default_value, ControlValueType1):
            default_value = self.default_value.to_dict()
        elif isinstance(self.default_value, ControlValueType2):
            default_value = self.default_value.to_dict()
        elif isinstance(self.default_value, ControlValueType3):
            default_value = self.default_value.to_dict()
        elif isinstance(self.default_value, ControlValueType4):
            default_value = self.default_value.to_dict()
        elif isinstance(self.default_value, ControlValueType5):
            default_value = self.default_value.to_dict()
        elif isinstance(self.default_value, ControlValueType6):
            default_value = self.default_value.to_dict()
        else:
            default_value = self.default_value.to_dict()

        name = self.name

        aspect_lock: float | None | Unset
        if isinstance(self.aspect_lock, Unset):
            aspect_lock = UNSET
        else:
            aspect_lock = self.aspect_lock

        binding: dict[str, Any] | None | Unset
        if isinstance(self.binding, Unset):
            binding = UNSET
        elif isinstance(self.binding, ControlBinding):
            binding = self.binding.to_dict()
        else:
            binding = self.binding

        group: None | str | Unset
        if isinstance(self.group, Unset):
            group = UNSET
        else:
            group = self.group

        id = self.id

        kind: dict[str, Any] | str | Unset
        if isinstance(self.kind, Unset):
            kind = UNSET
        elif isinstance(self.kind, ControlKindType0):
            kind = self.kind.value
        elif isinstance(self.kind, ControlKindType1):
            kind = self.kind.value
        elif isinstance(self.kind, ControlKindType2):
            kind = self.kind.value
        elif isinstance(self.kind, ControlKindType3):
            kind = self.kind.value
        elif isinstance(self.kind, ControlKindType4):
            kind = self.kind.value
        elif isinstance(self.kind, ControlKindType5):
            kind = self.kind.value
        elif isinstance(self.kind, ControlKindType6):
            kind = self.kind.value
        elif isinstance(self.kind, ControlKindType7):
            kind = self.kind.value
        elif isinstance(self.kind, ControlKindType8):
            kind = self.kind.value
        else:
            kind = self.kind.to_dict()

        labels: list[str] | Unset = UNSET
        if not isinstance(self.labels, Unset):
            labels = self.labels

        max_: float | None | Unset
        if isinstance(self.max_, Unset):
            max_ = UNSET
        else:
            max_ = self.max_

        min_: float | None | Unset
        if isinstance(self.min_, Unset):
            min_ = UNSET
        else:
            min_ = self.min_

        preview_source: None | str | Unset
        if isinstance(self.preview_source, Unset):
            preview_source = UNSET
        elif isinstance(self.preview_source, PreviewSource):
            preview_source = self.preview_source.value
        else:
            preview_source = self.preview_source

        step: float | None | Unset
        if isinstance(self.step, Unset):
            step = UNSET
        else:
            step = self.step

        tooltip: None | str | Unset
        if isinstance(self.tooltip, Unset):
            tooltip = UNSET
        else:
            tooltip = self.tooltip

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "control_type": control_type,
                "default_value": default_value,
                "name": name,
            }
        )
        if aspect_lock is not UNSET:
            field_dict["aspect_lock"] = aspect_lock
        if binding is not UNSET:
            field_dict["binding"] = binding
        if group is not UNSET:
            field_dict["group"] = group
        if id is not UNSET:
            field_dict["id"] = id
        if kind is not UNSET:
            field_dict["kind"] = kind
        if labels is not UNSET:
            field_dict["labels"] = labels
        if max_ is not UNSET:
            field_dict["max"] = max_
        if min_ is not UNSET:
            field_dict["min"] = min_
        if preview_source is not UNSET:
            field_dict["preview_source"] = preview_source
        if step is not UNSET:
            field_dict["step"] = step
        if tooltip is not UNSET:
            field_dict["tooltip"] = tooltip

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.control_binding import ControlBinding
        from ..models.control_kind_type_9 import ControlKindType9
        from ..models.control_value_type_0 import ControlValueType0
        from ..models.control_value_type_1 import ControlValueType1
        from ..models.control_value_type_2 import ControlValueType2
        from ..models.control_value_type_3 import ControlValueType3
        from ..models.control_value_type_4 import ControlValueType4
        from ..models.control_value_type_5 import ControlValueType5
        from ..models.control_value_type_6 import ControlValueType6
        from ..models.control_value_type_7 import ControlValueType7

        d = dict(src_dict)
        control_type = ControlType(d.pop("control_type"))

        def _parse_default_value(
            data: object,
        ) -> (
            ControlValueType0
            | ControlValueType1
            | ControlValueType2
            | ControlValueType3
            | ControlValueType4
            | ControlValueType5
            | ControlValueType6
            | ControlValueType7
        ):
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_control_value_type_0 = ControlValueType0.from_dict(
                    data
                )

                return componentsschemas_control_value_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_control_value_type_1 = ControlValueType1.from_dict(
                    data
                )

                return componentsschemas_control_value_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_control_value_type_2 = ControlValueType2.from_dict(
                    data
                )

                return componentsschemas_control_value_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_control_value_type_3 = ControlValueType3.from_dict(
                    data
                )

                return componentsschemas_control_value_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_control_value_type_4 = ControlValueType4.from_dict(
                    data
                )

                return componentsschemas_control_value_type_4
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_control_value_type_5 = ControlValueType5.from_dict(
                    data
                )

                return componentsschemas_control_value_type_5
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_control_value_type_6 = ControlValueType6.from_dict(
                    data
                )

                return componentsschemas_control_value_type_6
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_control_value_type_7 = ControlValueType7.from_dict(data)

            return componentsschemas_control_value_type_7

        default_value = _parse_default_value(d.pop("default_value"))

        name = d.pop("name")

        def _parse_aspect_lock(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        aspect_lock = _parse_aspect_lock(d.pop("aspect_lock", UNSET))

        def _parse_binding(data: object) -> ControlBinding | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                binding_type_1 = ControlBinding.from_dict(data)

                return binding_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ControlBinding | None | Unset, data)

        binding = _parse_binding(d.pop("binding", UNSET))

        def _parse_group(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        group = _parse_group(d.pop("group", UNSET))

        id = d.pop("id", UNSET)

        def _parse_kind(
            data: object,
        ) -> (
            ControlKindType0
            | ControlKindType1
            | ControlKindType2
            | ControlKindType3
            | ControlKindType4
            | ControlKindType5
            | ControlKindType6
            | ControlKindType7
            | ControlKindType8
            | ControlKindType9
            | Unset
        ):
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_control_kind_type_0 = ControlKindType0(data)

                return componentsschemas_control_kind_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_control_kind_type_1 = ControlKindType1(data)

                return componentsschemas_control_kind_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_control_kind_type_2 = ControlKindType2(data)

                return componentsschemas_control_kind_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_control_kind_type_3 = ControlKindType3(data)

                return componentsschemas_control_kind_type_3
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_control_kind_type_4 = ControlKindType4(data)

                return componentsschemas_control_kind_type_4
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_control_kind_type_5 = ControlKindType5(data)

                return componentsschemas_control_kind_type_5
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_control_kind_type_6 = ControlKindType6(data)

                return componentsschemas_control_kind_type_6
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_control_kind_type_7 = ControlKindType7(data)

                return componentsschemas_control_kind_type_7
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_control_kind_type_8 = ControlKindType8(data)

                return componentsschemas_control_kind_type_8
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_control_kind_type_9 = ControlKindType9.from_dict(data)

            return componentsschemas_control_kind_type_9

        kind = _parse_kind(d.pop("kind", UNSET))

        labels = cast(list[str], d.pop("labels", UNSET))

        def _parse_max_(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        max_ = _parse_max_(d.pop("max", UNSET))

        def _parse_min_(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        min_ = _parse_min_(d.pop("min", UNSET))

        def _parse_preview_source(data: object) -> None | PreviewSource | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                preview_source_type_1 = PreviewSource(data)

                return preview_source_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | PreviewSource | Unset, data)

        preview_source = _parse_preview_source(d.pop("preview_source", UNSET))

        def _parse_step(data: object) -> float | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(float | None | Unset, data)

        step = _parse_step(d.pop("step", UNSET))

        def _parse_tooltip(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        tooltip = _parse_tooltip(d.pop("tooltip", UNSET))

        control_definition = cls(
            control_type=control_type,
            default_value=default_value,
            name=name,
            aspect_lock=aspect_lock,
            binding=binding,
            group=group,
            id=id,
            kind=kind,
            labels=labels,
            max_=max_,
            min_=min_,
            preview_source=preview_source,
            step=step,
            tooltip=tooltip,
        )

        control_definition.additional_properties = d
        return control_definition

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
