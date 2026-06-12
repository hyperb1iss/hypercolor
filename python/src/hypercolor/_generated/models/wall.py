from enum import Enum


class Wall(str, Enum):
    EAST = "east"
    NORTH = "north"
    SOUTH = "south"
    WEST = "west"

    def __str__(self) -> str:
        return str(self.value)
