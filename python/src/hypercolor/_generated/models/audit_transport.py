from enum import StrEnum


class AuditTransport(StrEnum):
    HTTP = "http"
    MCP = "mcp"
    WEBSOCKET = "websocket"

    def __str__(self) -> str:
        return str(self.value)
