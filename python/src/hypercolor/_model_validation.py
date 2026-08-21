from __future__ import annotations

import types
from enum import Enum
from functools import cache
from typing import Any, Union, get_args, get_origin, get_type_hints

from attrs import fields, has

from ._generated import models
from ._generated.types import Unset

_MODEL_TYPES = {**vars(models), "Unset": Unset}


@cache
def _field_types(model_type: type[object]) -> dict[str, Any]:
    return get_type_hints(model_type, localns=_MODEL_TYPES)


def validate_generated_model(model: object) -> None:
    _validate_value(model, type(model), type(model).__name__)


def _validate_value(value: object, expected: Any, path: str) -> None:
    if expected is Any:
        return

    origin = get_origin(expected)
    if origin in (Union, types.UnionType):
        _validate_union(value, get_args(expected), path)
        return

    if origin is list:
        _validate_list(value, get_args(expected), path)
        return

    if origin is dict:
        _validate_dict(value, get_args(expected), path)
        return

    _validate_scalar(value, expected, path)


def _validate_union(value: object, candidates: tuple[Any, ...], path: str) -> None:
    for candidate in candidates:
        try:
            _validate_value(value, candidate, path)
        except TypeError:
            continue
        return
    raise TypeError(f"{path} does not match its generated union")


def _validate_list(value: object, types_: tuple[Any, ...], path: str) -> None:
    if not isinstance(value, list):
        raise TypeError(f"{path} must be a list")
    (item_type,) = types_
    for index, item in enumerate(value):
        _validate_value(item, item_type, f"{path}[{index}]")


def _validate_dict(value: object, types_: tuple[Any, ...], path: str) -> None:
    if not isinstance(value, dict):
        raise TypeError(f"{path} must be an object")
    key_type, item_type = types_
    for key, item in value.items():
        _validate_value(key, key_type, f"{path}.<key>")
        _validate_value(item, item_type, f"{path}.{key}")


def _validate_scalar(value: object, expected: Any, path: str) -> None:
    if expected is bool:
        valid = type(value) is bool
    elif expected is int:
        valid = type(value) is int
    elif expected is float:
        valid = isinstance(value, (int, float)) and not isinstance(value, bool)
    elif expected in (str, type(None)):
        valid = type(value) is expected
    elif isinstance(expected, type) and issubclass(expected, Enum):
        valid = isinstance(value, expected)
    elif isinstance(expected, type) and has(expected):
        if not isinstance(value, expected):
            raise TypeError(f"{path} must be {expected.__name__}")
        field_types = _field_types(expected)
        for field in fields(expected):
            _validate_value(
                getattr(value, field.name),
                field_types[field.name],
                f"{path}.{field.name}",
            )
        return
    else:
        valid = isinstance(value, expected)

    if not valid:
        raise TypeError(f"{path} has the wrong generated type")
