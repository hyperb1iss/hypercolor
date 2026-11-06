# hypercolor-pixel-protocols

*Wire encoders for the open pixel streaming protocols: DDP, E1.31 (sACN), and Art-Net 4.*

A small leaf crate that turns channel bytes into UDP payloads for the three open
standards pixel controllers speak. It owns packet layouts, sequence counters, and
DMX512 universe math, and nothing else: no sockets, no async runtime, no device
policy. Drivers own transport, pacing, configuration, and color handling, and
every driver that streams these protocols encodes through it, so a layout fix
lands once.

## Position in the Workspace

- Depends on: `uuid`
- Consumed by: `hypercolor-driver-wled`

## Key Public Surface

- `ddp`: `DdpPacket`, `DdpSequence`, `DdpSpan`, `build_ddp_frame`,
  `build_ddp_spans`, `encode_ddp_packet`, `pixel_aligned_chunk_len`, and the
  `DDP_*` constants (port 4048, 1440-byte payload, data type bytes)
- `e131`: `E131Packet` (data packets with sync address and option bits),
  `E131SyncPacket`, `E131SequenceTracker`, `universes_needed`, and the
  `E131_*` constants (port 5568, universe and priority ranges)
- `artnet`: `ArtDmxPacket`, `ArtDmxSequenceTracker`, `PortAddress`, and the
  `ARTNET_*` constants (port 6454, protocol revision 14)
- `dmx`: `split_into_universes`, `universes_spanned`,
  `pixel_aligned_universe_channels`, `DMX_UNIVERSE_CHANNELS`

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor), open-source RGB
lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
