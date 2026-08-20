from enum import Enum


class BTreeMapAdditionalPropertyType7Kind(str, Enum):
    COLOR_RGBA = "color_rgba"

    def __str__(self) -> str:
        return str(self.value)
