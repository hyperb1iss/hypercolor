+++
title = "Binary frame format"
description = "The wire format for Hypercolor's binary WebSocket frames: tag bytes, header layouts, and the preview, spectrum, zone, screen-zones, and RPC codecs."
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
`f32`. There is no length prefix on the frame itself — the WebSocket message
boundary is the frame boundary, and each codec derives its payload length from its
own header fields.
{% end %}

## Two framing conventions ⚡

Hypercolor uses two distinct binary framing conventions on the wire, and the first
byte tells you which one you are looking at. Do not assume a uniform header.

The **streaming data frames** — preview canvases, the audio spectrum, zone
previews, and screen zones — use a **single tag byte** at offset 0. That tag byte
is the channel identity. There is no schema byte; these codecs version their layout
through the tag space itself and through fixed header lengths.

The **RPC frames** — request and response — use a **two-byte prefix**: a tag byte
at offset 0 followed by a schema byte at offset 1. This is the `BinaryFrameSchema`
contract (`TAG`, `SCHEMA`, `NAME`), and decoders validate both bytes with
`validate_frame_prefix` before touching the body.

{% callout(type="warning") %}
The streaming frames do **not** carry a schema byte. Only RPC frames do. A decoder
that blindly skips two bytes on a spectrum frame will read its `timestamp_ms` one
byte short. Branch on the first byte first, then apply the right convention.
{% end %}

## Tag byte map

Every binary frame is identified by its first byte. These are the load-bearing
magic numbers, taken straight from the source constants.

| Tag | Frame | Convention | Source constant |
|---|---|---|---|
| `0x02` | Audio spectrum | single byte | `SPECTRUM_FRAME_TAG` |
| `0x03` | Preview — render canvas | single byte | `PreviewFrameChannel::Canvas` |
| `0x05` | Preview — screen-capture canvas | single byte | `PreviewFrameChannel::ScreenCanvas` |
| `0x06` | Preview — web viewport canvas | single byte | `PreviewFrameChannel::WebViewportCanvas` |
| `0x07` | Preview — display face | single byte | `PreviewFrameChannel::DisplayPreview` |
| `0x08` | Zone preview | single byte | `ZONE_PREVIEW_FRAME_TAG` |
| `0x09` | Screen zones (ambilight grid) | single byte | `SCREEN_ZONES_FRAME_TAG` |
| `0x80` | RPC request | tag + schema | `RPC_REQUEST_TAG` |
| `0x81` | RPC response | tag + schema | `RPC_RESPONSE_TAG` |

{% callout(type="info") %}
`0x04` is intentionally unused in the current channel set, and `0x01` is reserved
for the JSON control channel's framing on the daemon side. Treat any unknown tag as
a frame you should skip rather than reject the connection — the channel space is
designed to grow.
{% end %}

## Preview frame (`0x03`, `0x05`, `0x06`, `0x07`)

A preview frame carries one rendered image: the composed render canvas, the screen
capture the ambilight pipeline sees, the web viewport, or a display face. All four
channels share a single 14-byte header (`PREVIEW_FRAME_HEADER_LEN = 14`) and differ
only by their tag byte.

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
per-pixel byte count. For `Jpeg` there is no fixed length — the payload is a
complete JPEG image that runs from offset 14 to the end of the WebSocket message.

{% callout(type="tip") %}
Native Rust clients holding the message as `bytes::Bytes` can decode with
`PreviewFrame::decode_bytes`, which slices the payload as a refcounted view instead
of copying it. Browser clients decode straight from a `js_sys::ArrayBuffer` via
`PreviewFrameView::decode_array_buffer` and read pixels with `rgba_at` or pull the
whole frame with one boundary crossing through `to_rgba_vec`.
{% end %}

The default render canvas is 640×480 but is configurable, so never hardcode
dimensions — always read `width` and `height` from the header. The canvas can resize
live, and the next frame's header will simply carry the new size.

## Zone preview frame (`0x08`)

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

For the REST and concurrency side of zones — the routes, `If-Match` revisions, and
`ZoneOutcome::Stale` — see the Studio zone documentation. This page covers only the
preview wire format.

## Screen zones frame (`0x09`)

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

## RPC frames (`0x80`, `0x81`)

RPC is the one binary surface that uses the two-byte `BinaryFrameSchema` prefix:
a tag byte then a schema byte (`RPC_SCHEMA = 1`), both validated by
`validate_frame_prefix` before the body is read. A request carries a correlation
`id`, a method name, and an opaque payload; a response echoes the `id`, returns a
status code, and carries its own payload.

### Request (`0x80`)

```text
offset  size  field
0       1     tag (0x80)
1       1     schema (0x01)
2       8     id           u64
10      2     method_len   u16
12      ..    method       method_len bytes (UTF-8)
12+ml   ..    payload      runs to end of message
```

### Response (`0x81`)

```text
offset  size  field
0       1     tag (0x81)
1       1     schema (0x01)
2       8     id           u64   (echoes the request id)
10      2     status       u16
12      ..    payload      runs to end of message
```

The `id` is a monotonic per-client correlation counter; the client starts at 1, and
`RpcClient::call_raw` loops over incoming responses until it sees the matching `id`,
so out-of-order or interleaved responses are handled. The `status` field maps to
`RpcStatus`, which follows HTTP conventions: `200` OK, `400` bad request, `404` not
found, `500` internal error, and `2xx` counts as success via `RpcStatus::is_success`.
The `method` is plain UTF-8; a non-UTF-8 method byte sequence is rejected as a decode
error rather than lossily coerced.

## Decode errors

Two error enums cover the two framing conventions. The streaming codecs return
`PreviewFrameDecodeError`, and the RPC codecs return `DecodeError`.

| Variant | Convention | Meaning |
|---|---|---|
| `TooShort` | streaming | message shorter than the fixed header |
| `UnknownChannel` | streaming | tag byte is not a known channel |
| `UnknownPixelFormat` | streaming | `format` byte is not 0/1/2 |
| `DimensionsOverflow` | streaming | `width × height × bpp` overflows `usize` |
| `PayloadTooShort` | streaming | header valid but payload truncated |
| `Truncated` | RPC | body shorter than the fixed field block |
| `WrongTag` | RPC | tag byte does not match the expected frame |
| `WrongSchema` | RPC | schema byte does not match `RPC_SCHEMA` |
| `InvalidHeader` | RPC | method length overflows the buffer |
| `InvalidBody` | RPC | method bytes are not valid UTF-8 |

A robust client validates the header before allocating for the payload. Every codec
here checks its declared length against the actual message length, so a truncated or
malformed frame fails cleanly instead of reading past the buffer.

## Schema negotiation

The `SchemaRange` and `negotiate_highest_common_schema` helpers exist for versioned
frames that carry a schema byte. A client advertises the inclusive range of schema
versions it understands, the server does the same, and the negotiated version is the
highest value in the intersection of the two ranges. If the ranges do not overlap,
negotiation returns `None` and the two peers have no common version to speak. Today
only the RPC frames carry a schema byte, so this machinery is forward-looking
headroom for the streaming frames rather than something every frame exercises.

## Where this lives

| Concern | File |
|---|---|
| Frame prefix, encode/decode traits, `DecodeError` | `ws/frame.rs` |
| `BinaryFrameSchema` trait, public re-exports | `ws/mod.rs` |
| Preview, zone-preview, screen-zones codecs | `ws/preview.rs` |
| Spectrum codec | `ws/spectrum.rs` |
| RPC request/response, `RpcStatus`, client/server | `ws/rpc.rs` |
| Schema range negotiation | `ws/schema.rs` |
| Round-trip conformance tests | `daemon/src/api/ws/tests.rs` |

All paths are relative to `crates/hypercolor-leptos-ext/src/` except the test file,
which lives in the daemon crate. When any layout on this page changes, the source
constant and its round-trip test change with it — read those, never this prose, when
the bytes have to be exactly right.
