from enum import Enum


class ControlValueKind(str, Enum):
    BOOL = "bool"
    COLOR_RGB = "color_rgb"
    COLOR_RGBA = "color_rgba"
    DURATION_MS = "duration_ms"
    ENUM = "enum"
    FLAGS = "flags"
    FLOAT = "float"
    INTEGER = "integer"
    IP_ADDRESS = "ip_address"
    LIST = "list"
    MAC_ADDRESS = "mac_address"
    NULL = "null"
    OBJECT = "object"
    SECRET_REF = "secret_ref"
    STRING = "string"
    UNKNOWN = "unknown"

    def __str__(self) -> str:
        return str(self.value)
