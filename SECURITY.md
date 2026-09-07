# Security Policy

## Supported Versions

Hypercolor is pre-1.0. Security fixes land on the latest release line only.

| Version | Supported |
| ------- | --------- |
| 0.3.x   | yes       |
| < 0.3   | no        |

## Reporting a Vulnerability

If you discover a security vulnerability, please report it responsibly through
[GitHub private vulnerability reporting](https://github.com/hyperb1iss/hypercolor/security/advisories/new).
Reports filed there reach the maintainers privately without exposing details in
the public issue tracker.

**What to include:**

- Description of the vulnerability
- Steps to reproduce
- Impact assessment (what can an attacker do?)
- Suggested fix, if you have one

**Response timeline:**

- Acknowledgment within 48 hours
- Initial assessment within 7 days
- Fix or mitigation plan within 30 days for confirmed vulnerabilities

Please do not open public issues for security vulnerabilities. We'll coordinate disclosure
with you once a fix is available.

## Scope

Hypercolor runs as a local daemon communicating with USB/HID devices and a web UI. Out of the
box the daemon binds to `127.0.0.1:9420` with `network.access_mode = "local_only"` and
`network.allow_unauthenticated_remote_access = false`. Under those defaults it refuses to bind a
non-loopback address unless `HYPERCOLOR_API_KEY` is set.

Two supported settings lift that requirement, and either one exposes an unauthenticated control
API to whoever can reach the bound address:

- `network.access_mode = "lan_trusted"` drops the API key requirement unconditionally.
- `network.allow_unauthenticated_remote_access = true` drops it under the `local_only` and
  `custom` access modes.

Anyone who reaches the daemon under either setting gets full control of devices, scenes, and
effects, so treat both as a deliberate trust decision about the surrounding network.

The primary attack surface includes:

- **REST API / WebSocket / MCP** on localhost by default (`:9420`)
- **USB/HID communication** with connected devices
- **HTML effects** rendered via embedded Servo

We take all reports seriously, but local-only attack vectors may be prioritized differently
than remotely exploitable ones.
