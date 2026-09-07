# Hypercolor user skills

Skills in this directory are for people running Hypercolor, not for people developing
it. They drive a live daemon through the REST API, the MCP tools, or the `hypercolor`
CLI and never assume a source checkout. Codebase skills for contributors live in
`.agents/skills/`.

| Skill | What it does |
|---|---|
| `rig-setup` | Turns a physical PC build into a spatial layout and scene: case research, device inventory, owner interview, generated bindings and layout, then a guided dial-in on the lit hardware. Ships reusable case specs under `rig-setup/references/cases/`. |

Install into an agent host by pointing it at the skill directory, for example with the
skills CLI (`npx skills add hyperb1iss/hypercolor --skill rig-setup`) or by copying the
folder into the host's skills location.
