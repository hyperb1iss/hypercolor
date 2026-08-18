from enum import Enum


class InputSourcePlatformStatusType0Type(str, Enum):
    MACOS_INPUT = "macos_input"

    def __str__(self) -> str:
        return str(self.value)
