from enum import Enum


class Redaction(str, Enum):
    PLAIN = "plain"
    SECRET = "secret"

    def __str__(self) -> str:
        return str(self.value)
