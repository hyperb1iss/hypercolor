from enum import Enum


class InputSourcePlatformStatusType1Type(str, Enum):
    MACOS_SCREEN = "macos_screen"

    def __str__(self) -> str:
        return str(self.value)
