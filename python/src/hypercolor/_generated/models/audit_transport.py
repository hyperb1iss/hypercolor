from enum import Enum


class AuditTransport(str, Enum):
    HTTP = "http"
    MCP = "mcp"
    WEBSOCKET = "websocket"

    def __str__(self) -> str:
        return str(self.value)
