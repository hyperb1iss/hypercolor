from ..tolerant_enum import TolerantStrEnum


class AuditTransport(TolerantStrEnum):
    HTTP = "http"
    MCP = "mcp"
    WEBSOCKET = "websocket"

    def __str__(self) -> str:
        return str(self.value)
