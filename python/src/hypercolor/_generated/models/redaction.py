from ..tolerant_enum import TolerantStrEnum


class Redaction(TolerantStrEnum):
    PLAIN = "plain"
    SECRET = "secret"

    def __str__(self) -> str:
        return str(self.value)
