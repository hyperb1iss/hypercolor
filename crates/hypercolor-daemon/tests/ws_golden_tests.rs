//! Golden fixtures for every binary WebSocket tag.
//!
//! Each fixture takes a fixed input, runs it through the same encoder
//! production uses, and compares the result byte for byte against an on-disk
//! annotated hex file. A refactor that shifts a field, widens an integer, or
//! renumbers a tag fails here before any client sees the drift.
//!
//! The preview family (`0x03`–`0x11`) and the RPC pair (`0x80`/`0x81`) encode
//! through `hypercolor-leptos-ext`, which owns the wire format. The `frames`
//! (`0x01`) and `spectrum` (`0x02`) tags are still written by the daemon, so
//! those fixtures call the daemon's own encoders through
//! `hypercolor_daemon::api::ws::wire`.
//!
//! `tests/fixtures/ws/README.md` documents the file format, the regeneration
//! procedure, and what changing a fixture means for wire compatibility.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use axum::body::Bytes;
use hypercolor_daemon::api::ws::wire::{
    FrameZoneSelection, encode_frame_binary_selected, encode_spectrum_binary,
};
use hypercolor_leptos_ext::ws::{
    BinaryFrameDecode, BinaryFrameEncode, EXTENDED_SCREEN_ZONES_FRAME_TAG,
    INTERACTIVE_PREVIEW_FRAME_TAG, InteractivePreviewFrame, PREVIEW_CANCEL_FRAME_TAG,
    PREVIEW_CHUNK_FRAME_TAG, PreviewCancelFrame, PreviewChunkFrame, PreviewFrame,
    PreviewFrameChannel, PreviewPixelFormat, PreviewPublicationMetadata, PreviewStreamId,
    RPC_REQUEST_TAG, RPC_RESPONSE_TAG, RpcRequest, RpcResponse, SCREEN_ZONES_FRAME_TAG,
    SPECTRUM_FRAME_TAG, ScreenZonesFrame, SpectrumFrame, WIDE_INTERACTIVE_PREVIEW_FRAME_TAG,
    WIDE_PREVIEW_FRAME_TAG, WIDE_SCREEN_ZONES_FRAME_TAG, WIDE_ZONE_PREVIEW_FRAME_TAG,
    ZONE_PREVIEW_FRAME_TAG, ZonePreviewFrame,
};
use hypercolor_types::event::{FrameData, SpectrumData, ZoneColors};

/// Environment variable that rewrites fixtures instead of asserting them.
const BLESS_ENV: &str = "HYPERCOLOR_WS_GOLDEN_BLESS";

/// Bytes per hex line in a fixture file.
const BYTES_PER_LINE: usize = 16;

/// Width of the hex column, so annotations line up across every fixture.
const HEX_COLUMN_WIDTH: usize = BYTES_PER_LINE * 3;

const GOLDEN_FRAME_NUMBER: u32 = 0x1234_5678;
const GOLDEN_TIMESTAMP_MS: u32 = 12_345_678;
const GOLDEN_PUBLICATION_ID: u64 = 0x0102_0304_0506_0708;
const GOLDEN_RPC_ID: u64 = 0x0011_2233_4455_6677;
const GOLDEN_SCENE_ID: [u8; 16] = [
    0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe,
];
const GOLDEN_ZONE_ID: [u8; 16] = [
    0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10, 0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01,
];
const GOLDEN_PREVIEW_ID: &str = "golden-preview";
const GOLDEN_WIDE_PREVIEW_ID: &str = "golden-wide-preview";

/// Smallest width that forces a wide (`u32` dimension) preview layout.
const WIDE_AXIS: u32 = 65_536;

/// Wire tags declared by `hypercolor-leptos-ext`: ten preview constants, four
/// preview channels, spectrum, and the RPC pair. The `frames` tag is daemon-
/// local and counted separately.
const EXPECTED_DECLARED_TAG_COUNT: usize = 17;

/// Bytes of a wide frame's payload stored in its fixture file. The rest is a
/// deterministic fill that would bloat the repository without freezing any
/// layout the header does not already pin.
const WIDE_PAYLOAD_STORED_BYTES: usize = 16;

// ── Fixture model ────────────────────────────────────────────────────────

/// One annotated run of bytes inside a fixture file.
struct Segment {
    len: usize,
    label: String,
}

fn seg(len: usize, label: &str) -> Segment {
    Segment {
        len,
        label: label.to_owned(),
    }
}

/// A single frozen encoding: the production bytes plus everything the fixture
/// file needs to document itself.
struct Golden {
    /// Fixture file stem, also the name accepted by the bless environment
    /// variable.
    name: &'static str,
    tag: u8,
    /// Source constant the tag byte comes from.
    tag_source: &'static str,
    /// Path of the encoder that produced these bytes.
    encoder: &'static str,
    /// Human description of the fixed input.
    input: String,
    segments: Vec<Segment>,
    /// Full encoder output.
    bytes: Vec<u8>,
    /// Leading bytes committed to the fixture file.
    stored_len: usize,
}

impl Golden {
    fn new(
        name: &'static str,
        tag: u8,
        tag_source: &'static str,
        encoder: &'static str,
        input: String,
        segments: Vec<Segment>,
        bytes: Vec<u8>,
    ) -> Self {
        let stored_len = bytes.len();
        Self {
            name,
            tag,
            tag_source,
            encoder,
            input,
            segments,
            bytes,
            stored_len,
        }
    }

    /// Store only the first `stored_len` bytes on disk. Used for wide frames,
    /// whose payloads run to hundreds of kilobytes.
    fn stored_prefix(mut self, stored_len: usize) -> Self {
        assert!(
            stored_len <= self.bytes.len(),
            "{}: stored prefix {stored_len} exceeds {} encoded bytes",
            self.name,
            self.bytes.len()
        );
        self.stored_len = stored_len;
        self
    }

    fn path(&self) -> PathBuf {
        fixture_dir().join(format!("{}.hex", self.name))
    }

    fn stored_bytes(&self) -> &[u8] {
        &self.bytes[..self.stored_len]
    }

    fn is_truncated(&self) -> bool {
        self.stored_len < self.bytes.len()
    }

    /// Render the fixture file: a metadata header, then one annotated hex line
    /// per layout segment.
    fn render(&self) -> String {
        let mut out = String::new();
        push_line(&mut out, "# hypercolor websocket golden fixture");
        push_line(&mut out, &format!("# fixture: {}", self.name));
        push_line(
            &mut out,
            &format!("# tag: 0x{:02x} ({})", self.tag, self.tag_source),
        );
        push_line(&mut out, &format!("# encoder: {}", self.encoder));
        push_line(&mut out, &format!("# input: {}", self.input));
        push_line(&mut out, &format!("# total-bytes: {}", self.bytes.len()));
        push_line(&mut out, &format!("# stored-bytes: {}", self.stored_len));
        if self.is_truncated() {
            push_line(
                &mut out,
                &format!(
                    "# note: the remaining {} payload bytes are a deterministic fill and are",
                    self.bytes.len() - self.stored_len
                ),
            );
            push_line(
                &mut out,
                "#       asserted by the test rather than committed here.",
            );
        }
        push_line(&mut out, "#");
        push_line(
            &mut out,
            "# Regenerating any byte below is a wire-format change. See README.md.",
        );

        let mut offset = 0_usize;
        for segment in &self.segments {
            let end = (offset + segment.len).min(self.stored_len);
            let label = if end - offset < segment.len {
                format!("{} (first {} bytes)", segment.label, end - offset)
            } else {
                segment.label.clone()
            };
            push_segment(&mut out, &self.bytes[offset..end], &label);
            offset += segment.len;
            if offset >= self.stored_len {
                break;
            }
        }
        assert!(
            offset >= self.stored_len,
            "{}: segments cover {offset} of {} stored bytes",
            self.name,
            self.stored_len
        );
        out
    }
}

fn push_line(out: &mut String, text: &str) {
    out.push_str(text);
    out.push('\n');
}

/// Emit one segment as annotated hex, wrapping at [`BYTES_PER_LINE`].
fn push_segment(out: &mut String, bytes: &[u8], label: &str) {
    if bytes.is_empty() {
        return;
    }
    let multiline = bytes.len() > BYTES_PER_LINE;
    for (index, chunk) in bytes.chunks(BYTES_PER_LINE).enumerate() {
        let hex = chunk
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        let annotation = if multiline {
            format!("{label} [+0x{:03x}]", index * BYTES_PER_LINE)
        } else {
            label.to_owned()
        };
        let width = HEX_COLUMN_WIDTH;
        push_line(out, &format!("{hex:<width$}# {annotation}"));
    }
}

/// A fixture file as read back from disk.
struct ParsedFixture {
    bytes: Vec<u8>,
    total_bytes: usize,
    stored_bytes: usize,
}

fn parse_fixture(name: &str, text: &str) -> ParsedFixture {
    let mut bytes = Vec::new();
    let mut total_bytes = None;
    let mut stored_bytes = None;

    for line in text.lines() {
        let (data, comment) = match line.split_once('#') {
            Some((data, comment)) => (data, Some(comment.trim())),
            None => (line, None),
        };
        if let Some(comment) = comment {
            if let Some(value) = comment.strip_prefix("total-bytes:") {
                total_bytes = Some(parse_count(name, "total-bytes", value));
            }
            if let Some(value) = comment.strip_prefix("stored-bytes:") {
                stored_bytes = Some(parse_count(name, "stored-bytes", value));
            }
        }
        for token in data.split_whitespace() {
            assert!(
                token.len() == 2,
                "{name}: hex token '{token}' must be exactly two digits"
            );
            let byte = u8::from_str_radix(token, 16)
                .unwrap_or_else(|_| panic!("{name}: '{token}' is not a hex byte"));
            bytes.push(byte);
        }
    }

    ParsedFixture {
        total_bytes: total_bytes
            .unwrap_or_else(|| panic!("{name}: fixture is missing its '# total-bytes:' header")),
        stored_bytes: stored_bytes
            .unwrap_or_else(|| panic!("{name}: fixture is missing its '# stored-bytes:' header")),
        bytes,
    }
}

fn parse_count(name: &str, field: &str, value: &str) -> usize {
    value
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{name}: '{field}' header is not a number"))
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ws")
}

/// The on-wire frame a fixture describes: its committed prefix plus, for
/// truncated fixtures, the encoder's deterministic payload tail.
fn wire_bytes(golden: &Golden) -> Vec<u8> {
    let text = std::fs::read_to_string(golden.path()).unwrap_or_else(|error| {
        panic!(
            "{}: cannot read fixture: {error}. A blessing run writes fixtures \
             from a different test, so run the suite once more without \
             {BLESS_ENV} to assert freshly written files.",
            golden.name
        )
    });
    let parsed = parse_fixture(golden.name, &text);
    let mut bytes = parsed.bytes;
    let stored = bytes.len();
    bytes.extend_from_slice(&golden.bytes[stored..]);
    bytes
}

/// Deterministic filler bytes. The period is coprime with 3 and 4 so the
/// pattern never aligns with a pixel stride.
fn deterministic_bytes(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|index| {
            let index = u8::try_from(index % 251).unwrap_or(0);
            index.wrapping_mul(7).wrapping_add(seed)
        })
        .collect()
}

/// A minimal JPEG carrying only the SOF0 dimensions the preview codec reads.
fn minimal_jpeg(width: u16, height: u16) -> Vec<u8> {
    let [height_hi, height_lo] = height.to_be_bytes();
    let [width_hi, width_lo] = width.to_be_bytes();
    vec![
        0xff, 0xd8, // SOI
        0xff, 0xc0, // SOF0
        0x00, 0x0b, // segment length
        0x08, // sample precision
        height_hi, height_lo, width_hi, width_lo, 0x01, // component count
        0x01, 0x11, 0x00, // component 0: id, sampling, quant table
        0xff, 0xd9, // EOI
    ]
}

// ── Fixture inputs ───────────────────────────────────────────────────────

fn golden_frame_data() -> FrameData {
    FrameData {
        frame_number: GOLDEN_FRAME_NUMBER,
        timestamp_ms: GOLDEN_TIMESTAMP_MS,
        zones: vec![
            ZoneColors {
                zone_id: "left".to_owned(),
                colors: vec![[0xff, 0x00, 0x00], [0x00, 0xff, 0x00]],
            },
            ZoneColors {
                zone_id: "right".to_owned(),
                colors: vec![[0x00, 0x00, 0xff]],
            },
        ],
    }
}

fn golden_spectrum_data() -> SpectrumData {
    SpectrumData {
        timestamp_ms: GOLDEN_TIMESTAMP_MS,
        level: 0.25,
        bass: 0.5,
        mid: 0.75,
        treble: 1.0,
        beat: true,
        beat_confidence: 0.875,
        bpm: Some(128.0),
        bins: vec![0.0, 0.25, 0.5, 1.0],
    }
}

fn golden_spectrum_frame() -> SpectrumFrame {
    let spectrum = golden_spectrum_data();
    SpectrumFrame {
        timestamp_ms: spectrum.timestamp_ms,
        level: spectrum.level,
        bass: spectrum.bass,
        mid: spectrum.mid,
        treble: spectrum.treble,
        beat: spectrum.beat,
        beat_confidence: spectrum.beat_confidence,
        bins: spectrum.bins,
    }
}

fn golden_preview_frame(
    channel: PreviewFrameChannel,
    width: u32,
    height: u32,
    format: PreviewPixelFormat,
    payload: Vec<u8>,
) -> PreviewFrame {
    PreviewFrame {
        channel,
        frame_number: GOLDEN_FRAME_NUMBER,
        timestamp_ms: GOLDEN_TIMESTAMP_MS,
        width,
        height,
        format,
        payload: Bytes::from(payload),
    }
}

fn golden_canvas_preview() -> PreviewFrame {
    golden_preview_frame(
        PreviewFrameChannel::Canvas,
        4,
        2,
        PreviewPixelFormat::Rgb,
        deterministic_bytes(4 * 2 * 3, 0x10),
    )
}

fn golden_screen_canvas_preview() -> PreviewFrame {
    golden_preview_frame(
        PreviewFrameChannel::ScreenCanvas,
        2,
        2,
        PreviewPixelFormat::Rgba,
        deterministic_bytes(2 * 2 * 4, 0x20),
    )
}

fn golden_web_viewport_preview() -> PreviewFrame {
    golden_preview_frame(
        PreviewFrameChannel::WebViewportCanvas,
        16,
        8,
        PreviewPixelFormat::Jpeg,
        minimal_jpeg(16, 8),
    )
}

fn golden_display_preview() -> PreviewFrame {
    golden_preview_frame(
        PreviewFrameChannel::DisplayPreview,
        2,
        2,
        PreviewPixelFormat::Rgb,
        deterministic_bytes(2 * 2 * 3, 0x30),
    )
}

fn golden_wide_preview() -> PreviewFrame {
    golden_preview_frame(
        PreviewFrameChannel::Canvas,
        WIDE_AXIS,
        1,
        PreviewPixelFormat::Rgb,
        deterministic_bytes(WIDE_AXIS as usize * 3, 0x40),
    )
}

fn golden_zone_preview() -> ZonePreviewFrame {
    ZonePreviewFrame {
        scene_id: GOLDEN_SCENE_ID,
        zone_id: GOLDEN_ZONE_ID,
        frame_number: GOLDEN_FRAME_NUMBER,
        timestamp_ms: GOLDEN_TIMESTAMP_MS,
        width: 2,
        height: 2,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(deterministic_bytes(2 * 2 * 3, 0x50)),
    }
}

fn golden_wide_zone_preview() -> ZonePreviewFrame {
    ZonePreviewFrame {
        width: WIDE_AXIS,
        height: 1,
        payload: Bytes::from(deterministic_bytes(WIDE_AXIS as usize * 3, 0x60)),
        ..golden_zone_preview()
    }
}

fn golden_interactive_preview() -> InteractivePreviewFrame {
    InteractivePreviewFrame {
        preview_id: GOLDEN_PREVIEW_ID.to_owned(),
        frame_number: GOLDEN_FRAME_NUMBER,
        timestamp_ms: GOLDEN_TIMESTAMP_MS,
        width: 2,
        height: 2,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from(deterministic_bytes(2 * 2 * 4, 0x70)),
    }
}

fn golden_wide_interactive_preview() -> InteractivePreviewFrame {
    InteractivePreviewFrame {
        preview_id: GOLDEN_WIDE_PREVIEW_ID.to_owned(),
        width: WIDE_AXIS,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(deterministic_bytes(WIDE_AXIS as usize * 3, 0x80)),
        ..golden_interactive_preview()
    }
}

fn golden_screen_zones() -> ScreenZonesFrame {
    ScreenZonesFrame {
        frame_number: GOLDEN_FRAME_NUMBER,
        timestamp_ms: GOLDEN_TIMESTAMP_MS,
        source_width: 1920,
        source_height: 1080,
        grid_cols: 4,
        grid_rows: 3,
        letterbox: [1, 1, 0, 0],
        payload: Bytes::from(deterministic_bytes(4 * 3 * 3, 0x90)),
    }
}

fn golden_wide_screen_zones() -> ScreenZonesFrame {
    ScreenZonesFrame {
        source_width: 70_000,
        payload: Bytes::from(deterministic_bytes(4 * 3 * 3, 0xa0)),
        ..golden_screen_zones()
    }
}

fn golden_extended_screen_zones() -> ScreenZonesFrame {
    ScreenZonesFrame {
        source_width: 3840,
        source_height: 1080,
        grid_cols: 256,
        grid_rows: 1,
        letterbox: [0, 0, 3, 3],
        payload: Bytes::from(deterministic_bytes(256 * 3, 0xb0)),
        ..golden_screen_zones()
    }
}

fn golden_preview_chunk() -> PreviewChunkFrame {
    PreviewChunkFrame {
        metadata: PreviewPublicationMetadata {
            stream: PreviewStreamId::Zone {
                scene_id: GOLDEN_SCENE_ID,
                zone_id: GOLDEN_ZONE_ID,
            },
            publication_id: GOLDEN_PUBLICATION_ID,
            frame_number: GOLDEN_FRAME_NUMBER,
            timestamp_ms: GOLDEN_TIMESTAMP_MS,
            width: 2,
            height: 2,
            format: PreviewPixelFormat::Rgb,
        },
        total_encoded_bytes: 64,
        chunk_offset: 0,
        chunk_index: 0,
        chunk_count: 2,
        payload: Bytes::from(deterministic_bytes(32, 0xc0)),
    }
}

fn golden_preview_cancel() -> PreviewCancelFrame {
    PreviewCancelFrame {
        stream: PreviewStreamId::Interactive(GOLDEN_PREVIEW_ID.to_owned()),
        publication_id: GOLDEN_PUBLICATION_ID,
    }
}

fn golden_rpc_request() -> RpcRequest {
    RpcRequest::new(
        GOLDEN_RPC_ID,
        "preview.open",
        Bytes::from_static(&[0x01, 0x02, 0x03, 0x04]),
    )
}

fn golden_rpc_response() -> RpcResponse {
    RpcResponse::ok(GOLDEN_RPC_ID, Bytes::from_static(&[0x0a, 0x0b, 0x0c]))
}

// ── Fixture table ────────────────────────────────────────────────────────

fn preview_segments(frame: &PreviewFrame) -> Vec<Segment> {
    let wide = !frame.uses_legacy_layout();
    let mut segments = Vec::new();
    if wide {
        segments.push(seg(1, "tag (wide passive preview)"));
        segments.push(seg(1, "channel tag"));
    } else {
        segments.push(seg(1, "tag (channel)"));
    }
    segments.push(seg(4, "frame_number u32le"));
    segments.push(seg(4, "timestamp_ms u32le"));
    if wide {
        segments.push(seg(4, "width u32le"));
        segments.push(seg(4, "height u32le"));
    } else {
        segments.push(seg(2, "width u16le"));
        segments.push(seg(2, "height u16le"));
    }
    segments.push(seg(1, "format u8 (0=rgb, 1=rgba, 2=jpeg)"));
    segments.push(seg(frame.payload.len(), "payload"));
    segments
}

fn zone_preview_segments(frame: &ZonePreviewFrame) -> Vec<Segment> {
    let wide = !frame.uses_legacy_layout();
    let mut segments = vec![
        seg(
            1,
            if wide {
                "tag (wide zone preview)"
            } else {
                "tag (zone preview)"
            },
        ),
        seg(4, "frame_number u32le"),
        seg(4, "timestamp_ms u32le"),
        seg(16, "scene_id uuid bytes"),
        seg(16, "zone_id uuid bytes"),
    ];
    if wide {
        segments.push(seg(4, "width u32le"));
        segments.push(seg(4, "height u32le"));
    } else {
        segments.push(seg(2, "width u16le"));
        segments.push(seg(2, "height u16le"));
    }
    segments.push(seg(1, "format u8"));
    segments.push(seg(frame.payload.len(), "payload"));
    segments
}

fn interactive_preview_segments(frame: &InteractivePreviewFrame) -> Vec<Segment> {
    let wide = !frame.uses_legacy_layout();
    let mut segments = vec![
        seg(
            1,
            if wide {
                "tag (wide interactive preview)"
            } else {
                "tag (interactive preview)"
            },
        ),
        seg(1, "preview_id length u8"),
        seg(4, "frame_number u32le"),
        seg(4, "timestamp_ms u32le"),
    ];
    if wide {
        segments.push(seg(4, "width u32le"));
        segments.push(seg(4, "height u32le"));
    } else {
        segments.push(seg(2, "width u16le"));
        segments.push(seg(2, "height u16le"));
    }
    segments.push(seg(1, "format u8"));
    segments.push(seg(frame.preview_id.len(), "preview_id utf8"));
    segments.push(seg(frame.payload.len(), "payload"));
    segments
}

fn screen_zones_segments(frame: &ScreenZonesFrame) -> Vec<Segment> {
    let mut segments = vec![seg(1, "tag"), seg(4, "frame_number u32le")];
    segments.push(seg(4, "timestamp_ms u32le"));
    if frame.uses_legacy_layout() {
        segments.push(seg(2, "source_width u16le"));
        segments.push(seg(2, "source_height u16le"));
        segments.push(seg(1, "grid_cols u8"));
        segments.push(seg(1, "grid_rows u8"));
        segments.push(seg(4, "letterbox u8 x4 (top, bottom, left, right)"));
    } else if frame.uses_compact_grid_layout() {
        segments.push(seg(4, "source_width u32le"));
        segments.push(seg(4, "source_height u32le"));
        segments.push(seg(1, "grid_cols u8"));
        segments.push(seg(1, "grid_rows u8"));
        segments.push(seg(4, "letterbox u8 x4 (top, bottom, left, right)"));
    } else {
        segments.push(seg(4, "source_width u32le"));
        segments.push(seg(4, "source_height u32le"));
        segments.push(seg(4, "grid_cols u32le"));
        segments.push(seg(4, "grid_rows u32le"));
        segments.push(seg(16, "letterbox u32le x4 (top, bottom, left, right)"));
    }
    segments.push(seg(frame.payload.len(), "payload rgb grid, row-major"));
    segments
}

fn frames_golden(name: &'static str, selection: &FrameZoneSelection) -> Golden {
    let frame = golden_frame_data();
    let bytes = encode_frame_binary_selected(&frame, selection);
    let included: Vec<&ZoneColors> = match selection {
        FrameZoneSelection::All => frame.zones.iter().collect(),
        FrameZoneSelection::Named(names) => frame
            .zones
            .iter()
            .filter(|zone| names.contains(&zone.zone_id))
            .collect(),
    };

    let mut segments = vec![
        seg(1, "tag (led frame)"),
        seg(4, "frame_number u32le"),
        seg(4, "timestamp_ms u32le"),
        seg(1, "zone_count u8"),
    ];
    for (index, zone) in included.iter().enumerate() {
        segments.push(seg(2, &format!("zone[{index}] id length u16le")));
        segments.push(seg(
            zone.zone_id.len(),
            &format!("zone[{index}] id utf8 \"{}\"", zone.zone_id),
        ));
        segments.push(seg(2, &format!("zone[{index}] led_count u16le")));
        segments.push(seg(
            zone.colors.len() * 3,
            &format!("zone[{index}] colors rgb"),
        ));
    }

    let input = match selection {
        FrameZoneSelection::All => {
            "2 zones (\"left\" 2 leds, \"right\" 1 led), selection = all".to_owned()
        }
        FrameZoneSelection::Named(_) => {
            "2 zones (\"left\" 2 leds, \"right\" 1 led), selection = [\"right\"]".to_owned()
        }
    };

    Golden::new(
        name,
        0x01,
        "led_frame (daemon-local literal)",
        "hypercolor_daemon::api::ws::wire::encode_frame_binary_selected",
        input,
        segments,
        bytes,
    )
}

fn goldens() -> Vec<Golden> {
    let mut goldens = vec![
        frames_golden("01-frames-all", &FrameZoneSelection::All),
        frames_golden(
            "01-frames-selected",
            &FrameZoneSelection::new(&["right".to_owned()]),
        ),
    ];

    let spectrum = golden_spectrum_data();
    goldens.push(Golden::new(
        "02-spectrum",
        SPECTRUM_FRAME_TAG,
        "SPECTRUM_FRAME_TAG",
        "hypercolor_daemon::api::ws::wire::encode_spectrum_binary",
        "4 bins requested, 4 source bins, beat = true".to_owned(),
        vec![
            seg(1, "tag"),
            seg(4, "timestamp_ms u32le"),
            seg(1, "bin_count u8"),
            seg(4, "level f32le"),
            seg(4, "bass f32le"),
            seg(4, "mid f32le"),
            seg(4, "treble f32le"),
            seg(1, "beat u8"),
            seg(4, "beat_confidence f32le"),
            seg(spectrum.bins.len() * 4, "bins f32le"),
        ],
        encode_spectrum_binary(&spectrum, 4),
    ));

    for (name, tag_source, frame) in [
        (
            "03-preview-canvas",
            "PreviewFrameChannel::Canvas",
            golden_canvas_preview(),
        ),
        (
            "05-preview-screen-canvas",
            "PreviewFrameChannel::ScreenCanvas",
            golden_screen_canvas_preview(),
        ),
        (
            "06-preview-web-viewport-canvas",
            "PreviewFrameChannel::WebViewportCanvas",
            golden_web_viewport_preview(),
        ),
        (
            "07-preview-display",
            "PreviewFrameChannel::DisplayPreview",
            golden_display_preview(),
        ),
    ] {
        let input = format!("{}x{} {:?}", frame.width, frame.height, frame.format);
        let segments = preview_segments(&frame);
        goldens.push(Golden::new(
            name,
            frame.channel.tag(),
            tag_source,
            "hypercolor_leptos_ext::ws::PreviewFrame::try_encode",
            input,
            segments,
            frame.encode().to_vec(),
        ));
    }

    let zone = golden_zone_preview();
    goldens.push(Golden::new(
        "08-zone-preview",
        ZONE_PREVIEW_FRAME_TAG,
        "ZONE_PREVIEW_FRAME_TAG",
        "hypercolor_leptos_ext::ws::ZonePreviewFrame::try_encode",
        format!(
            "{}x{} rgb, fixed scene and zone uuids",
            zone.width, zone.height
        ),
        zone_preview_segments(&zone),
        zone.encode().to_vec(),
    ));

    let screen_zones = golden_screen_zones();
    goldens.push(Golden::new(
        "09-screen-zones",
        SCREEN_ZONES_FRAME_TAG,
        "SCREEN_ZONES_FRAME_TAG",
        "hypercolor_leptos_ext::ws::ScreenZonesFrame::try_encode",
        format!(
            "{}x{} source, {}x{} grid, letterbox {:?}",
            screen_zones.source_width,
            screen_zones.source_height,
            screen_zones.grid_cols,
            screen_zones.grid_rows,
            screen_zones.letterbox
        ),
        screen_zones_segments(&screen_zones),
        screen_zones.encode().to_vec(),
    ));

    let interactive = golden_interactive_preview();
    goldens.push(Golden::new(
        "0a-interactive-preview",
        INTERACTIVE_PREVIEW_FRAME_TAG,
        "INTERACTIVE_PREVIEW_FRAME_TAG",
        "hypercolor_leptos_ext::ws::InteractivePreviewFrame::encode",
        format!(
            "preview_id \"{}\", {}x{} rgba",
            interactive.preview_id, interactive.width, interactive.height
        ),
        interactive_preview_segments(&interactive),
        interactive
            .encode()
            .expect("interactive preview frame encodes")
            .to_vec(),
    ));

    let wide_preview = golden_wide_preview();
    let wide_preview_header = wide_preview.header_len();
    goldens.push(
        Golden::new(
            "0b-wide-preview",
            WIDE_PREVIEW_FRAME_TAG,
            "WIDE_PREVIEW_FRAME_TAG",
            "hypercolor_leptos_ext::ws::PreviewFrame::try_encode",
            format!(
                "canvas channel, {}x{} rgb (wide layout)",
                wide_preview.width, wide_preview.height
            ),
            preview_segments(&wide_preview),
            wide_preview.encode().to_vec(),
        )
        .stored_prefix(wide_preview_header + WIDE_PAYLOAD_STORED_BYTES),
    );

    let wide_zone = golden_wide_zone_preview();
    let wide_zone_header = wide_zone.header_len();
    goldens.push(
        Golden::new(
            "0c-wide-zone-preview",
            WIDE_ZONE_PREVIEW_FRAME_TAG,
            "WIDE_ZONE_PREVIEW_FRAME_TAG",
            "hypercolor_leptos_ext::ws::ZonePreviewFrame::try_encode",
            format!("{}x{} rgb (wide layout)", wide_zone.width, wide_zone.height),
            zone_preview_segments(&wide_zone),
            wide_zone.encode().to_vec(),
        )
        .stored_prefix(wide_zone_header + WIDE_PAYLOAD_STORED_BYTES),
    );

    let wide_interactive = golden_wide_interactive_preview();
    let wide_interactive_header = wide_interactive.prefix_len() + wide_interactive.preview_id.len();
    goldens.push(
        Golden::new(
            "0d-wide-interactive-preview",
            WIDE_INTERACTIVE_PREVIEW_FRAME_TAG,
            "WIDE_INTERACTIVE_PREVIEW_FRAME_TAG",
            "hypercolor_leptos_ext::ws::InteractivePreviewFrame::encode",
            format!(
                "preview_id \"{}\", {}x{} rgb (wide layout)",
                wide_interactive.preview_id, wide_interactive.width, wide_interactive.height
            ),
            interactive_preview_segments(&wide_interactive),
            wide_interactive
                .encode()
                .expect("wide interactive preview frame encodes")
                .to_vec(),
        )
        .stored_prefix(wide_interactive_header + WIDE_PAYLOAD_STORED_BYTES),
    );

    let wide_screen_zones = golden_wide_screen_zones();
    goldens.push(Golden::new(
        "0e-wide-screen-zones",
        WIDE_SCREEN_ZONES_FRAME_TAG,
        "WIDE_SCREEN_ZONES_FRAME_TAG",
        "hypercolor_leptos_ext::ws::ScreenZonesFrame::try_encode",
        format!(
            "{}x{} source (wide layout), {}x{} grid",
            wide_screen_zones.source_width,
            wide_screen_zones.source_height,
            wide_screen_zones.grid_cols,
            wide_screen_zones.grid_rows
        ),
        screen_zones_segments(&wide_screen_zones),
        wide_screen_zones.encode().to_vec(),
    ));

    let chunk = golden_preview_chunk();
    goldens.push(Golden::new(
        "0f-preview-chunk",
        PREVIEW_CHUNK_FRAME_TAG,
        "PREVIEW_CHUNK_FRAME_TAG",
        "hypercolor_leptos_ext::ws::PreviewChunkFrame::try_encode",
        "zone stream, chunk 0 of 2, 32 byte slice of a 64 byte publication".to_owned(),
        vec![
            seg(1, "tag"),
            seg(1, "schema u8"),
            seg(
                1,
                "stream_kind u8 (0=passive, 1=zone, 2=interactive, 3=screen_zones)",
            ),
            seg(1, "channel tag u8"),
            seg(1, "format u8"),
            seg(2, "stream_identity_len u16le"),
            seg(8, "publication_id u64le"),
            seg(4, "frame_number u32le"),
            seg(4, "timestamp_ms u32le"),
            seg(4, "width u32le"),
            seg(4, "height u32le"),
            seg(8, "total_encoded_bytes u64le"),
            seg(8, "chunk_offset u64le"),
            seg(4, "chunk_index u32le"),
            seg(4, "chunk_count u32le"),
            seg(32, "stream identity (scene_id, zone_id)"),
            seg(chunk.payload.len(), "chunk payload"),
        ],
        chunk
            .try_encode()
            .expect("preview chunk frame encodes")
            .to_vec(),
    ));

    let cancel = golden_preview_cancel();
    goldens.push(Golden::new(
        "10-preview-cancel",
        PREVIEW_CANCEL_FRAME_TAG,
        "PREVIEW_CANCEL_FRAME_TAG",
        "hypercolor_leptos_ext::ws::PreviewCancelFrame::try_encode",
        format!("interactive stream \"{GOLDEN_PREVIEW_ID}\""),
        vec![
            seg(1, "tag"),
            seg(1, "schema u8"),
            seg(1, "stream_kind u8"),
            seg(1, "channel tag u8"),
            seg(2, "stream_identity_len u16le"),
            seg(8, "publication_id u64le"),
            seg(GOLDEN_PREVIEW_ID.len(), "stream identity (preview_id utf8)"),
        ],
        cancel
            .try_encode()
            .expect("preview cancel frame encodes")
            .to_vec(),
    ));

    let extended = golden_extended_screen_zones();
    goldens.push(Golden::new(
        "11-extended-screen-zones",
        EXTENDED_SCREEN_ZONES_FRAME_TAG,
        "EXTENDED_SCREEN_ZONES_FRAME_TAG",
        "hypercolor_leptos_ext::ws::ScreenZonesFrame::try_encode",
        format!(
            "{}x{} source, {}x{} grid (extended layout), letterbox {:?}",
            extended.source_width,
            extended.source_height,
            extended.grid_cols,
            extended.grid_rows,
            extended.letterbox
        ),
        screen_zones_segments(&extended),
        extended.encode().to_vec(),
    ));

    let request = golden_rpc_request();
    goldens.push(Golden::new(
        "80-rpc-request",
        RPC_REQUEST_TAG,
        "RPC_REQUEST_TAG",
        "hypercolor_leptos_ext::ws::RpcRequest::encode",
        format!("method \"{}\", 4 byte payload", request.method),
        vec![
            seg(1, "tag"),
            seg(1, "schema u8"),
            seg(8, "request id u64le"),
            seg(2, "method length u16le"),
            seg(request.method.len(), "method utf8"),
            seg(request.payload.len(), "payload"),
        ],
        request.encode().to_vec(),
    ));

    let response = golden_rpc_response();
    goldens.push(Golden::new(
        "81-rpc-response",
        RPC_RESPONSE_TAG,
        "RPC_RESPONSE_TAG",
        "hypercolor_leptos_ext::ws::RpcResponse::encode",
        "status 200, 3 byte payload".to_owned(),
        vec![
            seg(1, "tag"),
            seg(1, "schema u8"),
            seg(8, "request id u64le"),
            seg(2, "status u16le"),
            seg(response.payload.len(), "payload"),
        ],
        response.encode().to_vec(),
    ));

    goldens
}

// ── Tests ────────────────────────────────────────────────────────────────

#[test]
fn every_binary_tag_matches_its_golden_fixture() {
    let bless = std::env::var(BLESS_ENV).unwrap_or_default();
    let requested: BTreeSet<&str> = bless
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    let bless_all = requested.contains("all");

    let goldens = goldens();
    let names: BTreeSet<&str> = goldens.iter().map(|golden| golden.name).collect();
    for name in &requested {
        assert!(
            *name == "all" || names.contains(name),
            "{BLESS_ENV} names unknown fixture '{name}'; known fixtures: {names:?}"
        );
    }

    let mut failures = Vec::new();
    for golden in &goldens {
        let path = golden.path();
        if bless_all || requested.contains(golden.name) {
            std::fs::create_dir_all(fixture_dir()).expect("fixture directory is writable");
            std::fs::write(&path, golden.render())
                .unwrap_or_else(|error| panic!("{}: cannot write fixture: {error}", golden.name));
            println!("blessed {} -> {}", golden.name, path.display());
        }

        let Ok(text) = std::fs::read_to_string(&path) else {
            failures.push(format!(
                "{}: fixture {} is missing; regenerate it with {BLESS_ENV}={}",
                golden.name,
                path.display(),
                golden.name
            ));
            continue;
        };
        let parsed = parse_fixture(golden.name, &text);

        if parsed.total_bytes != golden.bytes.len() {
            failures.push(format!(
                "{}: encoder produced {} bytes, fixture declares {}",
                golden.name,
                golden.bytes.len(),
                parsed.total_bytes
            ));
        }
        if parsed.stored_bytes != parsed.bytes.len() {
            failures.push(format!(
                "{}: fixture declares {} stored bytes but carries {}",
                golden.name,
                parsed.stored_bytes,
                parsed.bytes.len()
            ));
        }
        if parsed.bytes != golden.stored_bytes() {
            failures.push(format!(
                "{}: bytes differ from the fixture\n{}",
                golden.name,
                describe_mismatch(golden.stored_bytes(), &parsed.bytes)
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "binary wire format drifted from its golden fixtures. Changing these bytes \
         breaks every deployed client, so it needs the spec 76 §0 dual-accept \
         process, not a fixture rewrite.\n\n{}",
        failures.join("\n\n")
    );
}

#[test]
fn every_fixture_starts_with_its_declared_tag() {
    for golden in goldens() {
        assert_eq!(
            golden.bytes.first().copied(),
            Some(golden.tag),
            "{} does not open with tag 0x{:02x}",
            golden.name,
            golden.tag
        );
    }
}

#[test]
fn golden_fixtures_cover_the_whole_known_tag_space() {
    let manifest = protocol_manifest_tags();
    let mut expected: BTreeSet<u8> = manifest.keys().copied().collect();
    expected.insert(RPC_REQUEST_TAG);
    expected.insert(RPC_RESPONSE_TAG);

    let covered: BTreeSet<u8> = goldens().iter().map(|golden| golden.tag).collect();

    let missing: Vec<String> = expected
        .difference(&covered)
        .map(|tag| {
            format!(
                "0x{tag:02x} ({})",
                manifest.get(tag).map_or("rpc", std::string::String::as_str)
            )
        })
        .collect();
    assert!(
        missing.is_empty(),
        "these wire tags have no golden fixture: {}. A new binary tag must be \
         frozen by a fixture in the same change that introduces it.",
        missing.join(", ")
    );

    let unexpected: Vec<String> = covered
        .difference(&expected)
        .map(|tag| format!("0x{tag:02x}"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "these fixtures claim tags that protocol/websocket-v1.json does not \
         declare: {}",
        unexpected.join(", ")
    );

    assert!(
        !covered.contains(&0x04),
        "0x04 is deliberately unassigned in the channel tag space"
    );
}

#[test]
fn golden_fixtures_cover_every_tag_leptos_ext_declares() {
    let declared = declared_wire_tags();
    assert_eq!(
        declared.len(),
        EXPECTED_DECLARED_TAG_COUNT,
        "hypercolor-leptos-ext declares {} wire tags, this suite expects \
         {EXPECTED_DECLARED_TAG_COUNT}: {declared:?}. A new tag needs a golden \
         fixture and a bump here; if the count dropped because the constants \
         moved, point declared_wire_tags() at their new home.",
        declared.len()
    );

    let covered: BTreeSet<u8> = goldens().iter().map(|golden| golden.tag).collect();
    let missing: Vec<String> = declared
        .iter()
        .filter(|(tag, _)| !covered.contains(tag))
        .map(|(tag, name)| format!("0x{tag:02x} ({name})"))
        .collect();
    assert!(
        missing.is_empty(),
        "these declared wire tags have no golden fixture: {}",
        missing.join(", ")
    );
}

#[test]
fn wire_tag_constants_are_frozen() {
    assert_eq!(SPECTRUM_FRAME_TAG, 0x02);
    assert_eq!(PreviewFrameChannel::Canvas.tag(), 0x03);
    assert_eq!(PreviewFrameChannel::ScreenCanvas.tag(), 0x05);
    assert_eq!(PreviewFrameChannel::WebViewportCanvas.tag(), 0x06);
    assert_eq!(PreviewFrameChannel::DisplayPreview.tag(), 0x07);
    assert_eq!(ZONE_PREVIEW_FRAME_TAG, 0x08);
    assert_eq!(SCREEN_ZONES_FRAME_TAG, 0x09);
    assert_eq!(INTERACTIVE_PREVIEW_FRAME_TAG, 0x0a);
    assert_eq!(WIDE_PREVIEW_FRAME_TAG, 0x0b);
    assert_eq!(WIDE_ZONE_PREVIEW_FRAME_TAG, 0x0c);
    assert_eq!(WIDE_INTERACTIVE_PREVIEW_FRAME_TAG, 0x0d);
    assert_eq!(WIDE_SCREEN_ZONES_FRAME_TAG, 0x0e);
    assert_eq!(PREVIEW_CHUNK_FRAME_TAG, 0x0f);
    assert_eq!(PREVIEW_CANCEL_FRAME_TAG, 0x10);
    assert_eq!(EXTENDED_SCREEN_ZONES_FRAME_TAG, 0x11);
    assert_eq!(RPC_REQUEST_TAG, 0x80);
    assert_eq!(RPC_RESPONSE_TAG, 0x81);
}

#[test]
fn the_daemon_spectrum_encoder_matches_the_leptos_ext_codec() {
    let daemon_bytes = encode_spectrum_binary(&golden_spectrum_data(), 4);
    assert_eq!(
        daemon_bytes,
        golden_spectrum_frame().encode().to_vec(),
        "the daemon's hand-written spectrum header must stay byte-identical to \
         SpectrumFrame::encode, which replaces it in spec 76 §5"
    );
}

#[test]
fn fixtures_round_trip_through_the_leptos_ext_decoders() {
    let by_name: BTreeMap<&str, Golden> = goldens()
        .into_iter()
        .map(|golden| (golden.name, golden))
        .collect();
    let bytes_for = |name: &str| {
        wire_bytes(
            by_name
                .get(name)
                .unwrap_or_else(|| panic!("fixture {name} exists")),
        )
    };

    for (name, expected) in [
        ("03-preview-canvas", golden_canvas_preview()),
        ("05-preview-screen-canvas", golden_screen_canvas_preview()),
        (
            "06-preview-web-viewport-canvas",
            golden_web_viewport_preview(),
        ),
        ("07-preview-display", golden_display_preview()),
        ("0b-wide-preview", golden_wide_preview()),
    ] {
        let decoded = PreviewFrame::decode(&bytes_for(name)).expect("preview frame decodes");
        assert_eq!(decoded, expected, "{name} round trip");
    }

    for (name, expected) in [
        ("08-zone-preview", golden_zone_preview()),
        ("0c-wide-zone-preview", golden_wide_zone_preview()),
    ] {
        let decoded = ZonePreviewFrame::decode(&bytes_for(name)).expect("zone preview decodes");
        assert_eq!(decoded, expected, "{name} round trip");
    }

    for (name, expected) in [
        ("0a-interactive-preview", golden_interactive_preview()),
        (
            "0d-wide-interactive-preview",
            golden_wide_interactive_preview(),
        ),
    ] {
        let decoded =
            InteractivePreviewFrame::decode(&bytes_for(name)).expect("interactive preview decodes");
        assert_eq!(decoded, expected, "{name} round trip");
    }

    for (name, expected) in [
        ("09-screen-zones", golden_screen_zones()),
        ("0e-wide-screen-zones", golden_wide_screen_zones()),
        ("11-extended-screen-zones", golden_extended_screen_zones()),
    ] {
        let decoded = ScreenZonesFrame::decode(&bytes_for(name)).expect("screen zones decodes");
        assert_eq!(decoded, expected, "{name} round trip");
    }

    let chunk = PreviewChunkFrame::decode_bytes(&Bytes::from(bytes_for("0f-preview-chunk")))
        .expect("preview chunk decodes");
    assert_eq!(chunk, golden_preview_chunk());

    let cancel = PreviewCancelFrame::decode_bytes(&Bytes::from(bytes_for("10-preview-cancel")))
        .expect("preview cancel decodes");
    assert_eq!(cancel, golden_preview_cancel());

    let spectrum = SpectrumFrame::decode(&bytes_for("02-spectrum")).expect("spectrum decodes");
    assert_eq!(spectrum, golden_spectrum_frame());

    let request = RpcRequest::decode(&bytes_for("80-rpc-request")).expect("rpc request decodes");
    assert_eq!(request, golden_rpc_request());

    let response =
        RpcResponse::decode(&bytes_for("81-rpc-response")).expect("rpc response decodes");
    assert_eq!(response, golden_rpc_response());
}

#[test]
fn wide_frame_payloads_are_written_verbatim() {
    // The wide fixtures commit only their header plus the first bytes of the
    // payload, so the pass-through the fixture cannot show is asserted here.
    let preview = golden_wide_preview();
    let encoded = preview.encode();
    assert_eq!(&encoded[preview.header_len()..], &preview.payload[..]);

    let zone = golden_wide_zone_preview();
    let encoded = zone.encode();
    assert_eq!(&encoded[zone.header_len()..], &zone.payload[..]);

    let interactive = golden_wide_interactive_preview();
    let encoded = interactive.encode().expect("wide interactive encodes");
    let payload_offset = interactive.prefix_len() + interactive.preview_id.len();
    assert_eq!(&encoded[payload_offset..], &interactive.payload[..]);
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Tag bytes `hypercolor-leptos-ext` declares, scanned out of its wire codecs
/// so a tag added there fails this suite until it arrives with a fixture. The
/// `frames` tag is absent because the daemon, not leptos-ext, still owns it.
fn declared_wire_tags() -> BTreeMap<u8, String> {
    const PREVIEW_SOURCE: &str = include_str!("../../hypercolor-leptos-ext/src/ws/preview.rs");
    const SPECTRUM_SOURCE: &str = include_str!("../../hypercolor-leptos-ext/src/ws/spectrum.rs");
    const RPC_SOURCE: &str = include_str!("../../hypercolor-leptos-ext/src/ws/rpc.rs");

    let mut tags = BTreeMap::new();
    for source in [PREVIEW_SOURCE, SPECTRUM_SOURCE, RPC_SOURCE] {
        let mut in_channel_enum = false;
        for line in source.lines() {
            let line = line.trim();
            if line.starts_with("pub enum PreviewFrameChannel") {
                in_channel_enum = true;
                continue;
            }
            if in_channel_enum && line == "}" {
                in_channel_enum = false;
                continue;
            }

            if let Some(rest) = line.strip_prefix("pub const ")
                && let Some((name, value)) = rest.split_once(": u8 = ")
                && name.ends_with("_TAG")
            {
                record_tag(&mut tags, name, value.trim_end_matches(';'));
            } else if in_channel_enum && let Some((name, value)) = line.split_once(" = ") {
                record_tag(&mut tags, name, value.trim_end_matches(','));
            }
        }
    }
    tags
}

fn record_tag(tags: &mut BTreeMap<u8, String>, name: &str, value: &str) {
    let Some(hex) = value.trim().strip_prefix("0x") else {
        return;
    };
    let Ok(tag) = u8::from_str_radix(hex, 16) else {
        return;
    };
    tags.insert(tag, name.trim().to_owned());
}

fn protocol_manifest_tags() -> BTreeMap<u8, String> {
    let manifest: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/websocket-v1.json"
    )))
    .expect("websocket protocol manifest parses");

    manifest["binary_messages"]
        .as_array()
        .expect("manifest binary_messages is an array")
        .iter()
        .map(|message| {
            let name = message["name"]
                .as_str()
                .expect("binary message has a name")
                .to_owned();
            let tag = message["tag"]
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .expect("binary message tag fits in u8");
            (tag, name)
        })
        .collect()
}

fn describe_mismatch(expected: &[u8], actual: &[u8]) -> String {
    let offset = expected
        .iter()
        .zip(actual.iter())
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| expected.len().min(actual.len()));

    let window_start = offset.saturating_sub(8);
    let window_end = (offset + 8).min(expected.len().max(actual.len()));
    format!(
        "  first difference at byte {offset} (0x{offset:04x})\n  \
         encoder:  {}\n  fixture:  {}",
        hex_window(expected, window_start, window_end),
        hex_window(actual, window_start, window_end)
    )
}

fn hex_window(bytes: &[u8], start: usize, end: usize) -> String {
    let end = end.min(bytes.len());
    if start >= end {
        return "<end of data>".to_owned();
    }
    bytes[start..end]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
