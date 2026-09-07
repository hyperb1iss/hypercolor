from enum import Enum


class BindingSourceType1Kind(str, Enum):
    SENSOR = "sensor"

    def __str__(self) -> str:
        return str(self.value)
