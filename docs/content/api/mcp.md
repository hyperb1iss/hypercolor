+++
title = "MCP Server"
description = "Model Context Protocol server for AI-powered lighting control"
weight = 4
template = "page.html"
+++

Hypercolor includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server that lets AI assistants control your RGB lighting through natural language. The MCP server exposes tools, resources, and prompt templates over the Streamable HTTP transport.

## What Is MCP?

MCP is an open protocol that standardizes how AI applications connect to external tools and data sources. When you tell Claude "make my lights react to music" or "set a calm blue ambient effect," the AI assistant uses Hypercolor's MCP tools to translate that into actual API calls.

## Configuration

Enable the MCP server in your Hypercolor config:

```toml
[mcp]
enabled = true
base_path = "/mcp"        # Endpoint path
sse_keep_alive_secs = 30  # SSE keepalive interval
stateful_mode = true       # Enable stateful sessions
```

### Claude Code Configuration

Add Hypercolor to your Claude Code MCP servers. In your project's `.mcp.json`:

```json
{
  "mcpServers": {
    "hypercolor": {
      "type": "streamable-http",
      "url": "http://localhost:9420/mcp"
    }
  }
}
```

Or configure it globally in `~/.claude/settings.json` under the `mcpServers` key for access across all projects.

### Other MCP Clients

Any MCP-compatible client can connect using the Streamable HTTP transport at `http://localhost:9420/mcp`. The server implements the full MCP specification including tools, resources, and prompts.

## Available Tools

The MCP server exposes 16 tools for comprehensive lighting control:

| Tool               | Description                                                    | Read-Only |
| ------------------ | -------------------------------------------------------------- | --------- |
| `get_status`       | Get current system state (effect, devices, FPS, audio)         | Yes       |
| `list_effects`     | Browse available effects with search/filter                    | Yes       |
| `set_effect`       | Apply an effect with optional control values                   | No        |
| `stop_effect`      | Stop the currently running effect                              | No        |
| `set_color`        | Set a static color on all devices                              | No        |
| `get_devices`      | List connected devices and their status                        | Yes       |
| `set_brightness`   | Adjust global brightness (0-100)                               | No        |
| `get_audio_state`  | Get current audio analysis data                                | Yes       |
| `get_sensor_data`  | Read system hardware sensors (CPU temp, GPU load, RAM, etc.)   | Yes       |
| `activate_scene`   | Activate a lighting scene                                      | No        |
| `list_scenes`      | List available scenes                                          | Yes       |
| `create_scene`     | Create a new scene from current state                          | No        |
| `set_display_face` | Assign or clear an HTML display-face effect on a display device| No        |
| `set_profile`      | Switch to a saved profile                                      | No        |
| `get_layout`       | Get the current spatial layout                                 | Yes       |
| `diagnose`         | Run system diagnostics                                         | Yes       |

## Available Resources

Resources provide contextual data that AI assistants can read:

| URI                     | Description                             |
| ----------------------- | --------------------------------------- |
| `hypercolor://state`    | Full system state snapshot              |
| `hypercolor://effects`  | Effect catalog with metadata            |
| `hypercolor://devices`  | Connected device information            |
| `hypercolor://audio`    | Current audio analysis data             |
| `hypercolor://profiles` | Saved profiles and their configurations |

{% callout(type="tip", title="Start with state") %}
The server instructions tell AI assistants to read `hypercolor://state` or call `get_status` before making changes. This ensures the assistant understands the current lighting state before issuing commands.
{% end %}

## Prompt Templates

The MCP server includes prompt templates that guide AI assistants toward effective lighting control:

- **`mood_lighting`** — Helps the assistant choose effects based on mood, activity, or audio characteristics
- **`setup_automation`** — Guides scene setup with appropriate transitions and triggers
- **`troubleshoot`** — Structured prompts for troubleshooting device or audio issues

## Example Interactions

Once configured, you can control your lighting through natural conversation:

- _"Show me what effects are available for audio-reactive lighting"_
- _"Apply the borealis effect with speed set to 7"_
- _"Create a calm scene for late-night work with dim blue ambient lighting"_
- _"What devices are connected? Is everything working?"_
- _"Turn the brightness down to 40%"_
- _"Stop the current effect"_

The AI assistant translates these requests into the appropriate MCP tool calls, handling parameter mapping and error handling automatically.
