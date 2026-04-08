# Security Policy

## Supported Versions

Hypercolor is currently in pre-release (v0.1.x). Security fixes are applied to the latest
release on the `main` branch.

## Reporting a Vulnerability

If you discover a security vulnerability, please report it responsibly.

**Email:** stef@hyperbliss.tech

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

Hypercolor runs as a local daemon communicating with USB/HID devices and a web UI. The primary
attack surface includes:

- **REST API / WebSocket** on localhost (`:9420`)
- **USB/HID communication** with connected devices
- **HTML effects** rendered via embedded Servo

We take all reports seriously, but local-only attack vectors may be prioritized differently
than remotely exploitable ones.
