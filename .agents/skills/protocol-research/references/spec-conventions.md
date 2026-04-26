# Spec Document Conventions

Hypercolor specs live in `docs/specs/` with sequential numbering.

## Naming

`NN-short-descriptive-name.md` — e.g., `19-lian-li-uni-hub-driver.md`, `24-asus-aura-protocol-driver.md`

## Required Sections

### 1. Overview

Device family, supported product lines, why this driver exists.

### 2. Device Identification

| Field             | Example                                  |
| ----------------- | ---------------------------------------- |
| Vendor            | Lian Li                                  |
| VID               | 0x0CF2                                   |
| PIDs              | 0xA100 (SL), 0xA101 (AL), 0xA102 (SL V2) |
| Firmware versions | 1.7+ (HID), older (vendor control)       |
| Transport         | USB HID Feature Reports                  |

Include a variant matrix showing which PID + firmware → which protocol.

### 3. Wire Format

Byte-by-byte packet diagrams for each command type. Use markdown tables:

```
| Offset | Size | Field | Value | Description |
|--------|------|-------|-------|-------------|
| 0 | 1 | Report ID | 0xE0 | ENE HID report identifier |
| 1 | 1 | Command | 0x35 | Activate command |
| 2 | 1 | Port | 0-7 | Target fan port |
| 3-64 | 62 | Padding | 0x00 | Unused |
```

### 4. Command Vocabulary

Table of all commands with their byte values, arguments, and expected responses.

### 5. Color Encoding

- Byte order (RGB, RBG, BGR)
- Maximum LEDs per packet
- Packing format (interleaved vs component-separated)
- Padding behavior (zero-fill)

### 6. Timing

- Inter-packet delays
- Frame interval (target FPS)
- Response timeouts
- Init sequence timing

### 7. Topology

Per-variant zone layouts, LED counts, addressing schemes.

### 8. Implementation Notes

Platform quirks, firmware-specific branches, known issues, testing notes.

## Existing Specs as Templates

| Spec                               | Size | Best Template For                                  |
| ---------------------------------- | ---- | -------------------------------------------------- |
| `17-razer-protocol-driver.md`      | 22KB | Multi-version protocols, CRC algorithms            |
| `19-lian-li-uni-hub-driver.md`     | 40KB | Multi-variant devices, dual transport types        |
| `24-asus-aura-protocol-driver.md`  | 60KB | Runtime topology discovery, large device databases |
| `16-hardware-abstraction-layer.md` | 14KB | Overall HAL architecture reference                 |

## Quality Bar

A spec is complete when an agent can implement the entire driver from the spec alone. Every byte position, every timing requirement, every variant branch should be documented.
