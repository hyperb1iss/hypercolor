# OpenRGB SDK Provenance

## Mode

Public-docs/capture mode.

This crate is implemented from public OpenRGB protocol documentation and tests
against local fake SDK server frames. Black-box captures from a running,
unmodified OpenRGB SDK server may be added later. OpenRGB implementation source
files are not used for this crate.

## Approved Sources

| Surface | Source | Fields Used |
| --- | --- | --- |
| SDK packet protocol | `Documentation/OpenRGBSDK.md` and https://openrgb.org/sdk.html | packet header, packet IDs, controller data, mode data, zone data, segment data, LED data, update packets |
| RGBController vocabulary | `Documentation/RGBControllerAPI.md` | device types, mode flag bits, color mode values, RGB color byte order |

## Forbidden Sources

Do not use OpenRGB implementation sources such as `NetworkProtocol`,
`NetworkClient`, `RGBController`, controller implementations, detector
implementations, or GPL Rust bindings when writing this crate.

## Notes

The SDK currently supports protocol versions 1 through 5. Protocol version 0 is
unversioned and is rejected by negotiation code.

Mode flag semantics are limited to the public RGBController API. Device-specific
persisting mode behavior must come from a later approved source before the
driver auto-enables those modes.

The active driver slice uses synthesized golden controller-data fixtures for
supported protocol versions plus fake SDK server integration tests. A real
captured packet corpus is a future compatibility-hardening gate, not a
requirement for the clean SDK crate to exist in this milestone.
