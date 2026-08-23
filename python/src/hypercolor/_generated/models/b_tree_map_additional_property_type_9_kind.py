from enum import Enum


class BTreeMapAdditionalPropertyType9Kind(str, Enum):
    COLOR_RGB = "color_rgb"

    def __str__(self) -> str:
        return str(self.value)
