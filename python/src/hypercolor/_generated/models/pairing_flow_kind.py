from enum import StrEnum


class PairingFlowKind(StrEnum):
    CREDENTIALS_FORM = "credentials_form"
    PHYSICAL_ACTION = "physical_action"

    def __str__(self) -> str:
        return str(self.value)
