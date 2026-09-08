+++
title = "ROLI Blocks"
description = "Lightpad grids and LUMI key lighting through the blocksd bridge."
weight = 65
+++

Hypercolor sends colors to a local [blocksd](https://github.com/hyperb1iss/blocksd)
daemon over a Unix socket. The daemon owns MIDI, device keepalive, LittleFoot
program upload, and frame delivery. Hypercolor supplies effects and spatial mapping.

| Device | Required discovery fields | Hypercolor surface |
| --- | --- | --- |
| Lightpad / Lightpad M | `grid_width = 15`, `grid_height = 15` | 225-color matrix |
| LUMI Keys | `key_count = 24`, zero grid dimensions | 24-color strip in key-index order |

Other Blocks and unsupported dimensions are excluded from lighting discovery.
LUMI requires a blocksd build with the `key_frame` API and supported device
firmware (1.3.0 or newer). Older daemons that omit `key_count` still work for
Lightpad grids; Hypercolor does not invent a keyboard surface for them.

## Connect

Start blocksd before Hypercolor. ROLI discovery is enabled by default. A custom
socket can be selected in the Hypercolor configuration:

```toml
[discovery]
blocks_scan = true
blocks_socket_path = "/run/user/1000/blocksd/blocksd.sock"
```

Use the socket path reported by your daemon. Hypercolor's default is
`$XDG_RUNTIME_DIR/blocksd/blocksd.sock`, falling back to
`/tmp/blocksd/blocksd.sock`. An explicit path also supports a daemon installed
with a different platform-specific runtime directory.

Once discovered, assign the Grid or Keys segment to a scene zone like other
lighting devices. LUMI uses a linear key layout; its topology does not model the
different physical heights of black and white keys.

## Frame delivery

Lightpad uses the existing 685-byte binary packet. LUMI uses JSON `key_frame`
with 72 base64-encoded RGB888 bytes. The keyboard reply must carry the matching
UID and a boolean acceptance result. Invalid replies invalidate the connection.

An accepted write means blocksd queued the colors. Startup and heap recovery
may delay display while blocksd checks renderer execution. Neither an API ACK
nor Hypercolor's requested cadence establishes the hardware's displayed frame rate.

## Native effect check

The repository includes a 15-second check that runs Hypercolor's native Rainbow
renderer through the bridge. Run it while blocksd is active and no other
Hypercolor process is writing to the same ROLI devices:

```sh
./scripts/cargo-cache-build.sh cargo run -p hypercolor-core --example blocks_rainbow
```

Pass a custom socket path after `--` if needed. The example touches only
supported ROLI lighting surfaces and leaves blocksd running when it exits.
The example verifies the native effect and bridge path; it does not exercise
the full scene compositor or certify musical MIDI/MPE behavior.
