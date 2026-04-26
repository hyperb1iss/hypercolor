+++
title = "API"
description = "REST, WebSocket, CLI, and MCP interfaces for controlling Hypercolor"
sort_by = "weight"
template = "section.html"
+++

Hypercolor exposes multiple interfaces for controlling your lighting. All interfaces operate on the same underlying engine state via the event bus, so changes made through one interface are immediately visible to all others.

| Interface                           | Transport         | Best For                            |
| ----------------------------------- | ----------------- | ----------------------------------- |
| **[REST API](@/api/rest.md)**       | HTTP on `:9420`   | Scripting, integrations, automation |
| **[WebSocket](@/api/websocket.md)** | WS on `:9420`     | Real-time state streaming, live UIs |
| **[CLI](@/api/cli.md)**             | HTTP to daemon    | Terminal workflows, shell scripts   |
| **[MCP Server](@/api/mcp.md)**      | HTTP (Streamable) | AI assistant integration            |

The daemon also exposes a `/health` endpoint for monitoring and a `/preview` page for browser-based effect preview.
