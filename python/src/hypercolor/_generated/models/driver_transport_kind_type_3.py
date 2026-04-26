from enum import Enum


class DriverTransportKindType3(str, Enum):
    MIDI = "midi"

    def __str__(self) -> str:
        return str(self.value)
