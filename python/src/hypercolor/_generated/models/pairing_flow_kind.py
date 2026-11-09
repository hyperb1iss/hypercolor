from ..tolerant_enum import TolerantStrEnum


class PairingFlowKind(TolerantStrEnum):
    CREDENTIALS_FORM = "credentials_form"
    PHYSICAL_ACTION = "physical_action"

    def __str__(self) -> str:
        return str(self.value)
