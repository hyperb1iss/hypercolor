from enum import StrEnum


class DeviceClassHint(StrEnum):
    AUDIO = "audio"
    CONTROLLER = "controller"
    DISPLAY = "display"
    HUB = "hub"
    KEYBOARD = "keyboard"
    LIGHT = "light"
    MOUSE = "mouse"
    OTHER = "other"

    def __str__(self) -> str:
        return str(self.value)
