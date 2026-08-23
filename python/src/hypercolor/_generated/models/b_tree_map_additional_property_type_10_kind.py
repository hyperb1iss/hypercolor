from enum import Enum


class BTreeMapAdditionalPropertyType10Kind(str, Enum):
    COLOR_RGBA = "color_rgba"

    def __str__(self) -> str:
        return str(self.value)
