+++
title = "Prompt templates"
description = "The three shipped Hypercolor MCP prompts: mood_lighting, troubleshoot, and setup_automation, with arguments, message flows, and when to reach for each."
weight = 40
template = "page.html"
+++

The Hypercolor MCP server ships **three prompt templates**: `mood_lighting`, `troubleshoot`, and `setup_automation`. A prompt is the third MCP primitive alongside [tools](@/agents/tools-reference.md) and [resources](@/agents/resources-reference.md). Where a tool is a verb the agent calls and a resource is ambient state it reads, a prompt is a pre-built conversation flow: a sequence of messages that already pulls the right resources and lines up the right tool calls so an assistant lands on a good result without improvising the whole interaction.

Most clients surface prompts as slash commands. In Claude Code, `mood_lighting` shows up as `/mood_lighting`; pick it, fill in the arguments, and the assistant replays the template's message sequence with your values substituted in. Everything on this page is pulled from `crates/hypercolor-daemon/src/mcp/prompts.rs`, not paraphrased.

{% callout(type="warning") %}
The MCP server is **off by default**. Until you enable it in config, no prompt resolves and `http://127.0.0.1:9420/mcp` returns 404. Turn it on first in [MCP setup](@/agents/mcp-setup.md), then come back here.
{% end %}

## How prompts work 🔮

The daemon advertises prompts as an MCP capability (`enable_prompts()` in the server builder), so any compliant client lists all three and can request one by name. When a client requests a prompt, the daemon substitutes the arguments you provide into a fixed message sequence and returns the rendered conversation. The assistant then runs that conversation: it reads the embedded resources, reasons over them, and calls tools to act.

Each template encodes the same read-then-act discipline the server's own instructions ask for. Every prompt opens by pulling `hypercolor://state` and other relevant resources before recommending or changing anything, so the assistant is always working from the live picture rather than a guess.

| Prompt             | Slash command       | Required args | Optional args            |
| ------------------ | ------------------- | ------------- | ------------------------ |
| `mood_lighting`    | `/mood_lighting`    | none          | `mood`, `audio_reactive` |
| `troubleshoot`     | `/troubleshoot`     | `issue`       | none                     |
| `setup_automation` | `/setup_automation` | none          | `description`            |

Only `troubleshoot` has a required argument. The other two run fine with no arguments at all, falling back to a sensible default (`mood_lighting` assumes "a cozy vibe", `setup_automation` opens an open-ended automation conversation) and asking follow-up questions from there.

## mood_lighting

Configure lighting to match a mood, vibe, or activity. The template walks the assistant through effect selection, brightness, and color tuning, grounded in your actual hardware.

| Argument         | Required | Description                                                                                                                                     |
| ---------------- | -------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `mood`           | no       | Desired mood or vibe, e.g. `relaxing evening`, `energetic party`, `deep focus coding`. If omitted, the prompt defaults to a cozy vibe and asks. |
| `audio_reactive` | no       | Whether to include audio-reactive effects in the suggestions. Values: `yes`, `no`, `auto`.                                                      |

The rendered flow opens with your mood, then has the assistant read three resources in sequence: `hypercolor://state`, `hypercolor://effects`, and `hypercolor://devices`. The `audio_reactive` argument constrains selection: `yes` considers only reactive effects, `no` excludes them, and `auto` includes one only when it is the strongest match. With the live state, catalog, and hardware in context, the assistant chooses one deterministic best match and calls `set_effect` exactly once. It then uses the returned zone and layer identities with `adjust_controls` for final tuning.

{% callout(type="tip") %}
This is the prompt to reach for when a user says something like "make it feel calm in here" or "party mode." Because it reads the effect catalog and device inventory before choosing, it picks an installed effect that fits the rig. The applied change runs through one `set_effect` call, followed by `adjust_controls` against the returned layer when tuning is needed.
{% end %}

A typical run, with arguments `mood = "deep focus coding"` and `audio_reactive = "no"`, chooses one non-reactive ambient effect, applies it once, and lowers the returned layer's `speed` through `adjust_controls`.

## troubleshoot

Guided troubleshooting for device connectivity, rendering, or performance problems. This is the only prompt with a required argument, because the assistant needs to know what is actually wrong before it runs diagnostics.

| Argument | Required | Description                                                                                               |
| -------- | -------- | --------------------------------------------------------------------------------------------------------- |
| `issue`  | **yes**  | A description of the problem, e.g. `network strip not responding`, `colors look wrong`, `low frame rate`. |

The flow opens with your issue description, has the assistant read `hypercolor://state` and `hypercolor://devices`, then instructs it to run the zero-argument `diagnose` tool for the canonical safe diagnostic report. From its checks, summary, snapshot, and the resource context, the assistant identifies the root cause and gives concrete remediation steps. It uses only registered Hypercolor tools for actions they support and states plainly when remediation belongs outside Hypercolor.

{% callout(type="info") %}
The `diagnose` tool returns the same safe diagnostic payload as the default REST diagnostic pass: `checks`, `summary`, and `snapshot`. The `troubleshoot` prompt is the conversational front end to that report. For symptom-first human troubleshooting outside an agent, see the [troubleshooting section](@/troubleshooting/_index.md), and for the deeper device and audio walkthroughs, [devices not found](@/troubleshooting/devices-not-found.md) and [audio not reacting](@/troubleshooting/audio-not-reacting.md).
{% end %}

## setup_automation

Prepare reusable scenes for an external automation system. The template helps the assistant inspect existing scenes and effects, create an empty named scene when needed, activate it, and configure its lighting state.

| Argument      | Required | Description                                                                                                                                                                          |
| ------------- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `description` | no       | A natural-language description of the desired automation, e.g. `dim lights at 10pm`, `warm colors at sunset`. If omitted, the assistant opens an open-ended automation conversation. |

The flow reads `hypercolor://scenes`, `hypercolor://state`, and `hypercolor://effects`. When a new reusable scene is needed, it calls `create_scene`, activates the scene, chooses one catalog effect, and calls `set_effect` once. It uses the returned zone and layer identities with `adjust_controls` for final tuning.

{% callout(type="warning") %}
Hypercolor does not schedule or trigger scenes. The `create_scene` tool accepts a name plus optional description, enabled state, and mutation mode. It creates an empty reusable scene and does not capture the current output. The external system owns trigger conditions and calls `activate_scene` when they match. See [create_scene in the tools reference](@/agents/tools-reference.md) for the full argument list.
{% end %}

Remember that scenes are whole-rig configurations. Zones are the flexible canvas partitions inside a scene, and scheduling belongs to the external automation system.

## Using prompts from an agent

Prompts are a convenience layer, not a separate API. Everything a prompt does, an agent can do by hand with the underlying tools and resources, so reach for a prompt when you want a known-good flow and call tools directly when you need precise control. The three templates map cleanly onto the most common agent jobs: set a vibe, fix a problem, or prepare a scene for an external scheduler.

If you are wiring an assistant up for the first time, the natural path is to enable the server in [MCP setup](@/agents/mcp-setup.md), skim the [tools reference](@/agents/tools-reference.md) to learn the verbs, and let the prompts orchestrate the common cases. For hand-built CLI and MCP playbooks that go beyond the three shipped prompts, the agent-scripting pages in this section walk through complete automation against the daemon.

{% callout(type="success") %}
All three prompts open by reading state. That is the single most useful habit to copy when you write your own flows: orient from `hypercolor://state` before you act, and your tool calls land predictably.
{% end %}
