from enum import StrEnum


class DriverTransportKindType3(StrEnum):
    MIDI = "midi"

    def __str__(self) -> str:
        return str(self.value)
