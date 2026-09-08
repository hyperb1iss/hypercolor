+++
title = "Set up your rig with an agent"
description = "Install Hypercolor's user skills, map your PC from its case and wiring, and dial in a reusable spatial layout on the real hardware."
weight = 5
+++

A lighting effect looks connected when it moves through your PC in physical order:
across the bottom fans, up the side, and into the top radiator. Hypercolor's `rig-setup`
skill helps an agent build that map with you. The agent handles device inventory, case
geometry, and layout generation. You supply the wiring facts and watch the LEDs to
confirm the result.

The output includes a reusable rig specification as well as a layout and scene. You can
edit the specification and apply it again when you move a fan or replace a controller.

## Install the skills

Start Hypercolor with your devices connected. You need an agent that can read local
files and run shell commands, plus Python 3 for the setup scripts. No source checkout
is needed.

Copy both complete folders from the release bundle's `share/hypercolor/skills/` into
your agent host's skills directory:

| Skill | Use it for |
| --- | --- |
| `rig-setup` | Device coverage, case research, wiring interview, layout generation, and hardware dial-in |
| `hypercolor-control` | Inspecting the daemon, choosing effects, tuning controls, saving scenes, and diagnostics |

You can also get the folders from the repository's
[user skills directory](https://github.com/hyperb1iss/hypercolor/tree/main/skills).
Keep each folder intact, including `SKILL.md`, `scripts/`, and `references/` where
present. Ask your agent to read the installed `SKILL.md` if its host does not discover
skills automatically.

The CLI and REST API are enough for the setup workflow. Check the connection first:

```bash
hypercolor status
hypercolor devices list
```

For structured MCP tools, follow [MCP setup](@/agents/mcp-setup.md). MCP is disabled by
default and is optional for agents with shell access. A chat client with only MCP tools
cannot run the rig generator; use a host with local file and command access for setup.

## Give the agent your build

Start with the case model and a rough description. You do not need to know device IDs
or write layout JSON:

> Use rig-setup to map my PC in Hypercolor. My case is a Lian Li O11D EVO RGB.
> I have three bottom fans, three side fans, a top radiator, and RGB power cables.
> Inspect what is connected, ask about the controller ports, and show me a preview
> before applying the layout. Save the rig files so I can change the build later.

The skill includes an O11D EVO RGB case specification and example rigs for native and
bridged devices. For another case, the agent researches its dimensions and mounting
positions, then creates a case specification. Your port assignments and fan orientation
belong in a separate rig specification, so someone else's wiring is never assumed to
match yours.

## What happens during setup

1. **Inspect hardware and coverage.** The agent reads devices, controller slots, and
   existing layouts. Native drivers come first. If a device needs OpenRGB, the agent
   follows the [fallback setup guide](@/hardware/openrgb-fallback.md). Hardware neither
   stack supports can become a prefilled [support request](@/hardware/unsupported-devices.md).
2. **Describe the wiring.** Answer questions such as which port drives the bottom fans,
   how many fans are chained there, and which end connects to the controller. Detection
   can identify a controller; it cannot see what you plugged into its headers.
3. **Review the preview.** The generator validates bindings against the daemon's
   component templates and produces layout artifacts plus an SVG preview. Check that
   each group sits where you see it through the case glass.
4. **Apply and activate.** The agent creates or updates attachment profiles, the layout,
   and a scene. Activate the scene when you are ready to watch the hardware. Keep the
   previous scene available for switching back.
5. **Dial in on the LEDs.** Watch a directional effect and report observations such as
   "the top row runs backwards." The agent adjusts chain order, rotation, or mirroring
   and reapplies. An identify flash separates a mapping problem from a device that is
   not receiving output.

For bridged hubs, the agent also asks for LED counts and saves the zone sizes. A hub
can be discovered successfully while its unsized channels still contain zero LEDs.

## Keep the result

Ask the agent to leave you the case spec, rig spec, generated preview, and exact command
for applying the rig again. Run the generator from the installed `rig-setup` directory
(substitute your saved paths):

```bash
python3 scripts/gen_layout.py plan --case /path/to/case.json \
  --rig /path/to/my-rig.json --out /path/to/rig-output
python3 scripts/gen_layout.py apply --case /path/to/case.json \
  --rig /path/to/my-rig.json --out /path/to/rig-output
```

The rig spec records which wiring and orientation facts were verified on hardware and
which remain assumptions. Keep those files somewhere durable. After changing hardware,
ask the agent to inspect it again and update the saved rig before applying.

## Use the agent after setup

The `hypercolor-control` skill works with the same live scene you see in the UI. Try:

> Save my current lighting as "Evening," then find a slow purple and cyan effect.

> The side strip is dark. Inspect the device and active layout before changing anything.

> Pause the lighting output, keeping the scene ready to resume.

Browse [Agents & MCP](@/agents/_index.md) for the available tools and automation flows.
