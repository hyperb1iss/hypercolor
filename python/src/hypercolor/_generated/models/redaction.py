from enum import StrEnum


class Redaction(StrEnum):
    PLAIN = "plain"
    SECRET = "secret"

    def __str__(self) -> str:
        return str(self.value)
