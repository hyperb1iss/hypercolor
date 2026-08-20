from enum import Enum


class ControlValueKind(str, Enum):
    BOOL = "bool"
    COLOR_LINEAR = "color_linear"
    COLOR_RGB = "color_rgb"
    COLOR_RGBA = "color_rgba"
    DURATION = "duration"
    ENUM = "enum"
    FLAGS = "flags"
    FLOAT = "float"
    GRADIENT = "gradient"
    INT = "int"
    IP = "ip"
    LIST = "list"
    MAC = "mac"
    MAP = "map"
    NULL = "null"
    RECT = "rect"
    SECRET_REF = "secret_ref"
    TEXT = "text"
    UNKNOWN = "unknown"

    def __str__(self) -> str:
        return str(self.value)
