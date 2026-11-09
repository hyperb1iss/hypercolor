from ..tolerant_enum import TolerantStrEnum


class DriverTransportKindType0(TolerantStrEnum):
    BRIDGE = "bridge"
    MIDI = "midi"
    NETWORK = "network"
    SERIAL = "serial"
    SMBUS = "smbus"
    USB = "usb"
    VIRTUAL = "virtual"

    def __str__(self) -> str:
        return str(self.value)
