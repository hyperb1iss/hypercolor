from enum import StrEnum


class BindingSourceType1Kind(StrEnum):
    SENSOR = "sensor"

    def __str__(self) -> str:
        return str(self.value)
