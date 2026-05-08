from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="GpuCompositorProbeStatus")


@_attrs_define
class GpuCompositorProbeStatus:
    """
    Attributes:
        adapter_name (str):
        backend (str):
        linux_servo_gpu_import_backend_compatible (bool):
        max_storage_textures_per_shader_stage (int):
        max_texture_dimension_2d (int):
        texture_format (str):
        linux_servo_gpu_import_backend_reason (None | str | Unset):
    """

    adapter_name: str
    backend: str
    linux_servo_gpu_import_backend_compatible: bool
    max_storage_textures_per_shader_stage: int
    max_texture_dimension_2d: int
    texture_format: str
    linux_servo_gpu_import_backend_reason: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        adapter_name = self.adapter_name

        backend = self.backend

        linux_servo_gpu_import_backend_compatible = (
            self.linux_servo_gpu_import_backend_compatible
        )

        max_storage_textures_per_shader_stage = (
            self.max_storage_textures_per_shader_stage
        )

        max_texture_dimension_2d = self.max_texture_dimension_2d

        texture_format = self.texture_format

        linux_servo_gpu_import_backend_reason: None | str | Unset
        if isinstance(self.linux_servo_gpu_import_backend_reason, Unset):
            linux_servo_gpu_import_backend_reason = UNSET
        else:
            linux_servo_gpu_import_backend_reason = (
                self.linux_servo_gpu_import_backend_reason
            )

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "adapter_name": adapter_name,
                "backend": backend,
                "linux_servo_gpu_import_backend_compatible": linux_servo_gpu_import_backend_compatible,
                "max_storage_textures_per_shader_stage": max_storage_textures_per_shader_stage,
                "max_texture_dimension_2d": max_texture_dimension_2d,
                "texture_format": texture_format,
            }
        )
        if linux_servo_gpu_import_backend_reason is not UNSET:
            field_dict["linux_servo_gpu_import_backend_reason"] = (
                linux_servo_gpu_import_backend_reason
            )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        adapter_name = d.pop("adapter_name")

        backend = d.pop("backend")

        linux_servo_gpu_import_backend_compatible = d.pop(
            "linux_servo_gpu_import_backend_compatible"
        )

        max_storage_textures_per_shader_stage = d.pop(
            "max_storage_textures_per_shader_stage"
        )

        max_texture_dimension_2d = d.pop("max_texture_dimension_2d")

        texture_format = d.pop("texture_format")

        def _parse_linux_servo_gpu_import_backend_reason(
            data: object,
        ) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        linux_servo_gpu_import_backend_reason = (
            _parse_linux_servo_gpu_import_backend_reason(
                d.pop("linux_servo_gpu_import_backend_reason", UNSET)
            )
        )

        gpu_compositor_probe_status = cls(
            adapter_name=adapter_name,
            backend=backend,
            linux_servo_gpu_import_backend_compatible=linux_servo_gpu_import_backend_compatible,
            max_storage_textures_per_shader_stage=max_storage_textures_per_shader_stage,
            max_texture_dimension_2d=max_texture_dimension_2d,
            texture_format=texture_format,
            linux_servo_gpu_import_backend_reason=linux_servo_gpu_import_backend_reason,
        )

        gpu_compositor_probe_status.additional_properties = d
        return gpu_compositor_probe_status

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
