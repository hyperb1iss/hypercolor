# Spec Document Conventions

Hypercolor specs live in `docs/specs/`, numbered. Numbers are assigned in rough order but are not guaranteed unique: `76-internal-api-unification.md` and `76-macos-screen-capture-and-host-input.md` both exist today. Before claiming a number, `ls docs/specs/` and take one nothing else is using.

## Naming

`NN-short-descriptive-name.md` — e.g., `19-lian-li-uni-hub-driver.md`, `24-asus-aura-protocol-driver.md`

## Document Shape

Every driver spec opens with a title line, a one-paragraph blockquote summary, a metadata block, and a numbered table of contents. Copy this header verbatim and fill it in:

```markdown
# 17 -- Razer Protocol Driver

> Native USB HID driver for Razer Chroma peripherals. Byte-level packet formats,
> protocol generation dispatch, matrix addressing, and clean-room integration
> with the HAL.

**Status:** Draft
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::razer`
**Author:** Nova
**Date:** 2026-03-04

---

## Table of Contents
```

Sections are numbered `## N. Title` and the table of contents links to their anchors. A closing `## References` section is unnumbered.

## Required Sections

The three protocol driver specs (17, 19, 24) do not share a rigid section list, because a single-protocol device and a seven-protocol vendor need different shapes. What they do share is this skeleton, and every new driver spec should hit all of it:

1. **`## 1. Overview`** — device family, supported product lines, why this driver exists, and what the implementation was derived from. Always section 1.
2. **Device registry or packet format** — section 2 is either `Device Registry` (a full VID/PID/variant table, as in 19 and 24) or `Packet Format` (the byte layout, as in 17 where every device shares one packet). Pick whichever carries more information for the device.
3. **Protocol families, generations, or architecture** — how many wire protocols this vendor actually has and what selects between them. Spec 17 calls it `Protocol Generations`, 19 calls it `Protocol Families`, 24 calls it `Protocol Architecture`.
4. **One section per wire protocol** — byte-by-byte layouts, command vocabulary, color encoding, timing. Spec 19 splits ENE-over-HID, ENE-over-libusb, and TL Fan into three sections; spec 24 splits into four.
5. **`## N. HAL Integration`** — descriptors, transport intent, protocol factory wiring, and where the code lands. Spec 17 §7, spec 19 §12, spec 24 §8.
6. **`## N. Testing Strategy`** — what is asserted without hardware. Spec 17 §9, spec 19 §14, spec 24 §10.
7. **`## References`** — sources, captures, and community documentation used.

Beyond the skeleton, cover these wherever they naturally land:

- **Variant matrix** — which PID + firmware maps to which protocol, and which descriptor is the predicate-free fallback
- **Wire format tables** — one row per field:

  ```
  | Offset | Size | Field | Value | Description |
  |--------|------|-------|-------|-------------|
  | 0 | 1 | Report ID | 0xE0 | ENE HID report identifier |
  | 1 | 1 | Command | 0x10 | Activate command |
  | 2 | 1 | Port | 0-7 | Target fan port |
  | 3-64 | 62 | Padding | 0x00 | Unused |
  ```

- **Color encoding** — byte order (RGB, RBG, BGR), max LEDs per packet, interleaved vs component-separated, padding behavior
- **Timing** — inter-packet delays, frame interval, response timeouts, init sequence timing
- **Topology** — per-variant zone layouts, LED counts, addressing schemes, and whether zone counts are firmware-reported or hardcoded
- **Implementation notes** — platform quirks, firmware-specific branches, known issues

## Existing Specs as Templates

| Spec                               | Best Template For                                                    |
| ---------------------------------- | -------------------------------------------------------------------- |
| `17-razer-protocol-driver.md`      | Multi-version protocols, CRC algorithms                              |
| `19-lian-li-uni-hub-driver.md`     | Multi-variant devices, dual transport types, firmware disambiguation |
| `24-asus-aura-protocol-driver.md`  | Runtime topology discovery, large device databases, SMBus            |
| `16-hardware-abstraction-layer.md` | Overall HAL architecture reference                                   |
| `62-zerocopy-protocol-structs.md`  | The zerocopy packet-struct pattern every driver follows              |

## Quality Bar

A spec is complete when an agent can implement the entire driver from the spec alone. Every byte position, every timing requirement, every variant branch should be documented.
