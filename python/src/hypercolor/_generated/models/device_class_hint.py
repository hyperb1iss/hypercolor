from enum import Enum


class DeviceClassHint(str, Enum):
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
