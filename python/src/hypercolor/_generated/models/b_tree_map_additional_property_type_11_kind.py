from enum import Enum


class BTreeMapAdditionalPropertyType11Kind(str, Enum):
    COLOR_LINEAR = "color_linear"

    def __str__(self) -> str:
        return str(self.value)
