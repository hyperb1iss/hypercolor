+++
title = "Binary frame format"
description = "The wire format for Hypercolor's binary WebSocket frames: tag bytes, header layouts, and the preview, spectrum, zone, and screen-zones codecs."
weight = 40
+++

The daemon streams high-frequency data over the same `/api/v1/ws` socket as the
JSON control channel, but as binary WebSocket messages instead of text. Every
binary frame opens with one or two header bytes that tell a decoder exactly what
it is holding before it reads a single byte of payload. This page is the byte-level
contract for those frames.

The format is owned by one crate: `hypercolor-leptos-ext::ws` (feature `ws-core`,
pure Rust with no Leptos or WASM dependency). The daemon's encoders conform to it,
the web UI and the TUI both decode with it, and the round-trip is tested in
`daemon/src/api/ws/tests.rs`. There is no second copy of these layouts anywhere in
the codebase, and you should never hand-roll one. If you are building a non-Rust
client, mirror the bytes documented here exactly.

For the JSON control channel, the subprotocol token (`hypercolor-v1`), and how you
subscribe to the channels that produce these frames, see the
[WebSocket protocol reference](@/api/websocket.md).

{% callout(type="info") %}
All multi-byte integers and floats are **little-endian**. Floats are IEEE-754
`f32`. Direct frames derive payload length from their header and WebSocket message
boundary. Publications larger than one message use the `0x0F` chunk envelope,
which carries explicit total length, offset, and chunk-count fields.
{% end %}

## Two framing conventions ⚡

Hypercolor uses two binary framing conventions on the wire, and the first
byte tells you which one you are looking at. Do not assume a uniform header.

The **streaming data frames** (preview canvases, the audio spectrum, zone
previews, and screen zones) use a **single tag byte** at offset 0. That tag byte
is the channel identity. There is no schema byte; these codecs version their layout
through the tag space itself and through fixed header lengths.

The **preview transport control frames** use a tag byte at offset 0 and schema
`1` at offset 1. The chunk envelope (`0x0F`) carries routing, publication, and
reassembly metadata before one slice of a larger encoded preview, and the
cancellation frame (`0x10`) retires a publication a client may still be
reassembling. Both validate the schema byte before touching the body.

{% callout(type="warning") %}
Direct streaming frames do **not** carry a schema byte. Preview transport control
frames do. A decoder that blindly skips two bytes on a spectrum frame will read
its `timestamp_ms` one byte short. Branch on the first byte first, then apply the
right convention.
{% end %}

## Tag byte map

Every binary frame is identified by its first byte. These are the load-bearing
magic numbers, taken straight from the source constants.

| Tag | Frame | Convention | Source constant |
|---|---|---|---|
| `0x01` | LED color frames | single byte | `led_frame` |
| `0x02` | Audio spectrum | single byte | `SPECTRUM_FRAME_TAG` |
| `0x03` | Preview: render canvas | single byte | `PreviewFrameChannel::Canvas` |
| `0x05` | Preview: screen-capture canvas | single byte | `PreviewFrameChannel::ScreenCanvas` |
| `0x06` | Preview: web viewport canvas | single byte | `PreviewFrameChannel::WebViewportCanvas` |
| `0x07` | Preview: display face | single byte | `PreviewFrameChannel::DisplayPreview` |
| `0x08` | Zone preview | single byte | `ZONE_PREVIEW_FRAME_TAG` |
| `0x09` | Screen zones (ambilight grid) | single byte | `SCREEN_ZONES_FRAME_TAG` |
| `0x0A` | Addressed interactive preview | single byte | `INTERACTIVE_PREVIEW_FRAME_TAG` |
| `0x0B` | Wide passive preview | single byte | `WIDE_PREVIEW_FRAME_TAG` |
| `0x0C` | Wide zone preview | single byte | `WIDE_ZONE_PREVIEW_FRAME_TAG` |
| `0x0D` | Wide interactive preview | single byte | `WIDE_INTERACTIVE_PREVIEW_FRAME_TAG` |
| `0x0E` | Wide screen zones | single byte | `WIDE_SCREEN_ZONES_FRAME_TAG` |
| `0x0F` | Preview chunk envelope | tag + schema | `PREVIEW_CHUNK_FRAME_TAG` |
| `0x10` | Preview publication cancellation | tag + schema | `PREVIEW_CANCEL_FRAME_TAG` |
| `0x11` | Extended screen zones | single byte | `EXTENDED_SCREEN_ZONES_FRAME_TAG` |

{% callout(type="info") %}
`0x04` is intentionally unused in the current channel set. Treat any unknown tag
as a frame you should skip rather than reject the connection; the channel space
is designed to grow.
{% end %}

## Legacy preview frame (`0x03`, `0x05`, `0x06`, `0x07`)

A preview frame carries one rendered image: the composed render canvas, the screen
capture the ambilight pipeline sees, the web viewport, or a display face. All four
channels share a single 14-byte header (`PREVIEW_FRAME_HEADER_LEN = 14`) and differ
only by their tag byte. This byte-exact compatibility layout is used whenever both
dimensions fit `u16`; larger surfaces use `0x0B`.

```text
offset  size  field
0       1     tag (0x03 | 0x05 | 0x06 | 0x07)
1       4     frame_number  u32
5       4     timestamp_ms  u32
9       2     width         u16
11      2     height        u16
13      1     format        u8  (0=Rgb, 1=Rgba, 2=Jpeg)
14      ..    payload
```

The `format` byte selects the payload encoding through `PreviewPixelFormat`:

| Value | Format | Bytes per pixel | Payload length |
|---|---|---|---|
| `0` | `Rgb` | 3 | `width * height * 3` |
| `1` | `Rgba` | 4 | `width * height * 4` |
| `2` | `Jpeg` | n/a | runs to end of message |

For the raw formats (`Rgb`, `Rgba`) the payload is tightly packed, row-major,
top-left origin, and its length is fully determined by `width`, `height`, and the
per-pixel byte count. For `Jpeg` there is no fixed length; the payload is a
complete JPEG image that runs from offset 14 to the end of the direct publication.

{% callout(type="tip") %}
Native Rust clients holding the message as `bytes::Bytes` can decode with
`PreviewFrame::decode_bytes`, which slices the payload as a refcounted view instead
of copying it. Browser clients decode straight from a `js_sys::ArrayBuffer` via
`PreviewFrameView::decode_array_buffer` and read pixels with `rgba_at` or pull the
whole frame with one boundary crossing through `to_rgba_vec`.
{% end %}

The default render canvas is 640×480 but is configurable, so never hardcode
dimensions, so always read `width` and `height` from the header. The canvas can resize
live, and the next frame's header will simply carry the new size.

## Legacy zone preview frame (`0x08`)

A zone preview is a preview canvas scoped to one zone of one scene. Scenes are
whole-rig configurations; zones are flexible partitions of the canvas within a scene.
The frame carries both identifiers so a client subscribed to several zones can route
each frame without ambiguity. The header is 46 bytes
(`ZONE_PREVIEW_FRAME_HEADER_LEN = 46`).

```text
offset  size  field
0       1     tag (0x08)
1       4     frame_number  u32
5       4     timestamp_ms  u32
9       16    scene_id      [u8; 16]   (UUID bytes)
25      16    zone_id       [u8; 16]   (UUID bytes)
41      2     width         u16
43      2     height        u16
45      1     format        u8  (0=Rgb, 1=Rgba, 2=Jpeg)
46      ..    payload
```

The `scene_id` and `zone_id` are raw 16-byte UUIDs, written in the same byte order
they appear in their canonical form. The `format` byte and the payload follow the
exact same rules as the preview frame above. The browser decoder is
`ZonePreviewFrameView::decode_array_buffer`.

{% callout(type="warning") %}
Note the field order difference from the basic preview frame: in a zone preview the
`frame_number` and `timestamp_ms` come **before** the two UUIDs, and `width`/`height`
land at offsets 41 and 43, not 9 and 11. The two layouts are not interchangeable;
branch on the tag and apply the matching offsets.
{% end %}

For the REST and concurrency side of zones (the routes, `If-Match` revisions, and
`ZoneOutcome::Stale`) see the Studio zone documentation. This page covers only the
preview wire format.

## Legacy screen zones frame (`0x09`)

The screen zones frame is the ambilight grid: the smoothed, color-tuned per-sector
colors extracted from screen capture, exactly as screen-reactive effects consume
them. The payload is a row-major RGB grid, `grid_cols * grid_rows * 3` bytes. The
header is 19 bytes (`SCREEN_ZONES_FRAME_HEADER_LEN = 19`).

```text
offset  size  field
0       1     tag (0x09)
1       4     frame_number   u32
5       4     timestamp_ms   u32
9       2     source_width   u16
11      2     source_height  u16
13      1     grid_cols      u8
14      1     grid_rows      u8
15      1     letterbox_top  u8
16      1     letterbox_bottom u8
17      1     letterbox_left u8
18      1     letterbox_right u8
19      ..    payload (grid_cols * grid_rows * 3 bytes, row-major RGB)
```

`source_width` and `source_height` describe the captured display the grid was
sampled from. The four `letterbox` bytes are bars expressed in grid units (top,
bottom, left, right) so a client can mask the inactive border sectors when a 16:9
source is letterboxed into a different aspect. To read one sector's color, the
decoder offers `ScreenZonesFrame::zone_rgb(row, col)`, which computes
`(row * grid_cols + col) * 3` and returns the three bytes, or `None` if the
coordinate is out of range.

## Wide preview frames (`0x0B` through `0x0E`)

Wide layouts are additive. They keep legacy tags byte-exact for existing clients
and replace only dimension fields with `u32` when an axis exceeds `u16::MAX`.

| Tag | Frame | Wide header change |
|---|---|---|
| `0x0B` | Passive preview | byte 1 is the original channel tag; dimensions are at offsets 10 and 14; payload starts at 19 |
| `0x0C` | Zone preview | dimensions are at offsets 41 and 45; payload starts at 50 |
| `0x0D` | Interactive preview | dimensions are at offsets 10 and 14; preview id starts at 19 |
| `0x0E` | Screen zones | source dimensions are at offsets 9 and 13; grid metadata starts at 17; payload starts at 23 |

There is no fixed axis ceiling below `u32::MAX`. The daemon admits a requested
surface using checked pixel and byte arithmetic, with a 512 MiB publication
resource budget. Passive width and height may both be zero to select source size;
if exactly one is zero, the daemon preserves the source aspect ratio. Interactive
previews require both axes to be nonzero.

## Extended screen zones frame (`0x11`)

Screen zones widen in two steps rather than one. Its grid and letterbox fields
are `u8` in the legacy layout, so a grid can outgrow 255 sectors on an axis while
the source dimensions still fit `u16`. Tag `0x0E` widens only the source
dimensions; tag `0x11` widens everything, so `grid_cols`, `grid_rows`, and the
four letterbox bars each become `u32` and the header is 41 bytes
(`EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN = 41`).

```text
offset  size  field
0       1     tag (0x11)
1       4     frame_number   u32
5       4     timestamp_ms   u32
9       4     source_width   u32
13      4     source_height  u32
17      4     grid_cols      u32
21      4     grid_rows      u32
25      16    letterbox      4 × u32 (top, bottom, left, right)
41      ..    payload (grid_cols * grid_rows * 3 bytes, row-major RGB)
```

The encoder picks the narrowest layout each frame fits, so a client must accept
all three tags on the `screen_zones` channel and read the grid from whichever
header it received.

## Preview chunk envelope (`0x0F`)

Preview publications larger than the 1 MiB per-message budget are sent as ordered
chunks without resizing or truncation.

```text
offset  size  field
0       1     tag = 0x0F
1       1     schema = 1
2       1     stream_kind (0=passive, 1=zone, 2=interactive, 3=screen_zones)
3       1     channel tag
4       1     pixel format
5       2     stream_identity_len u16
7       8     publication_id u64
15      4     frame_number u32
19      4     timestamp_ms u32
23      4     width u32
27      4     height u32
31      8     total_encoded_bytes u64
39      8     chunk_offset u64
47      4     chunk_index u32
51      4     chunk_count u32
55      N     stream identity
55+N    ..    chunk payload
```

The stream identity is empty for passive and screen-zone streams, 32 raw UUID
bytes for a zone stream, and the UTF-8 preview id for an interactive stream.
Clients reassemble by stream and publication id, require contiguous ordered
chunks with stable metadata, and bound both per-publication and per-connection
memory. Reassembly state is connection-scoped and must be cleared on reconnect.

The envelope carries no payload format of its own. Reassembled bytes are one of
the ordinary frames documented above, identified by the `channel` byte at offset
3, so a client concatenates the chunks and hands the result to the same decoder
it would have used for a direct publication.

## Preview cancellation (`0x10`)

The daemon retires a publication a client may still be holding partial chunks
for by sending a cancellation frame. The header is 14 bytes plus the stream
identity (`PREVIEW_CANCEL_FIXED_HEADER_LEN = 14`, `PREVIEW_CANCEL_SCHEMA = 1`).

```text
offset  size  field
0       1     tag = 0x10
1       1     schema = 1
2       1     stream_kind (0=passive, 1=zone, 2=interactive, 3=screen_zones)
3       1     channel tag
4       2     stream_identity_len u16
6       8     publication_id u64
14      N     stream identity
```

The `stream_kind`, `channel`, and identity fields address the stream exactly as
they do in the chunk envelope. On receipt, drop any partial reassembly buffer
held for that publication id and release its memory; the publication will not be
completed.

## Spectrum frame (`0x02`)

The spectrum frame is one audio analysis snapshot: the overall level, the three
band energies, beat detection, and the full FFT bin array. The header is 27 bytes
(`SPECTRUM_FRAME_HEADER_LEN = 27`), followed by `bin_count` little-endian `f32`
values.

```text
offset  size  field
0       1     tag (0x02)
1       4     timestamp_ms     u32
5       1     bin_count        u8
6       4     level            f32
10      4     bass             f32
14      4     mid              f32
18      4     treble           f32
22      1     beat             u8  (0 | 1)
23      4     beat_confidence  f32
27      ..    bins             bin_count × f32
```

Because `bin_count` is a `u8`, the wire format carries at most 255 bins; the encoder
truncates anything longer. The `level`, `bass`, `mid`, and `treble` values are the
normalized energies that audio-reactive effects key off. `beat` is a hard 0/1 flag
and `beat_confidence` is its `f32` certainty.

{% callout(type="info") %}
BPM is deliberately **not** in the binary spectrum frame. Clients that need tempo
read it from the JSON metrics channel instead. The binary frame stays lean so it can
stream at audio rate without dragging slow-moving fields along on every packet.
{% end %}

## Decode errors

Every codec on this page reports failures through `PreviewFrameDecodeError`.

| Variant | Meaning |
|---|---|
| `TooShort` | message shorter than the fixed header |
| `UnknownChannel` | tag byte is not a known channel |
| `UnknownPixelFormat` | `format` byte is not 0/1/2 |
| `DimensionsOverflow` | `width × height × bpp` overflows `usize` |
| `PayloadTooShort` | header valid but payload truncated |

A robust client validates the header before allocating for the payload. Every codec
here checks its declared length against the actual message length, so a truncated or
malformed frame fails cleanly instead of reading past the buffer.

## Schema bytes

Every schema byte on the wire today is `1`, on both preview transport control
frames. The byte exists so a control frame's layout can be revised without
burning a new tag; a decoder rejects a schema value it does not recognize rather
than guessing at the body.

The preview transport's own `v1` and `v2` capability strings are a separate
mechanism, and they never reach the binary wire. They negotiate the memory
budgets a receiver will honor, in JSON, on the control channel; see the
[WebSocket protocol reference](@/api/websocket.md#the-hello-handshake). A decoder
reading the bytes on this page never needs to know which one was negotiated.

## Where this lives

| Concern | File |
|---|---|
| Tag constants and public re-exports | `ws/mod.rs` |
| Preview, zone-preview, screen-zones codecs | `ws/preview.rs` |
| Spectrum codec | `ws/spectrum.rs` |
| Codec round-trip tests | `crates/hypercolor-leptos-ext/tests/ws_preview_frame_tests.rs` |
| Daemon conformance tests | `daemon/src/api/ws/tests.rs` |
| Machine-checked frame manifest | `protocol/websocket-v1.json` |

All source paths are relative to `crates/hypercolor-leptos-ext/src/`; the two test
paths and the manifest are repo-relative. The manifest lists every tag, layout
name, and transport budget, and the daemon test suite asserts it against the code,
so a layout change that skips the manifest fails CI. When any layout on this page
changes, the source constant and its round-trip test change with it; read those,
never this prose, when the bytes have to be exactly right.
