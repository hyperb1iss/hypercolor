//! Binary payload caches and encoders for WebSocket streaming.
//!
//! Holds the sharded LRU caches for frame, spectrum, and canvas binaries,
//! plus the precomputed sRGB scale table used for dimmed canvas previews.
//! The single-slot router cache consumed by command dispatch also lives here.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, PoisonError};

use axum::body::Bytes;
use axum::extract::ws::Utf8Bytes;
use serde::Serialize;
use serde::ser::SerializeSeq;
use tracing::warn;

use hypercolor_types::canvas::{
    PublishedSurfaceStorageIdentity, linear_to_srgb_u8, srgb_u8_to_linear,
};

use super::preview_encode::{PreviewJpegEncoder, encode_canvas_jpeg_payload_scaled_stateless};
use super::protocol::{ActiveFramesConfig, CanvasFormat, FrameFormat, FrameZoneSelection};
use crate::api::AppState;

/// Maximum number of events that can be buffered per WebSocket client.
pub(super) const WS_BUFFER_SIZE: usize = 64;
pub(super) const WS_CANVAS_BYTES_PER_PIXEL_RGBA: u64 = 4;
pub(super) const WS_CANVAS_HEADER: u8 = 0x03;
pub(super) const WS_SCREEN_CANVAS_HEADER: u8 = 0x05;
/// Binary header byte for per-display preview JPEG frames streamed by
/// the `display_preview` channel. Body layout matches the canvas frame:
/// `[frame_number:u32LE][timestamp:u32LE][width:u16LE][height:u16LE][format:u8=2 (JPEG)][jpeg_payload]`.
pub(super) const WS_DISPLAY_PREVIEW_HEADER: u8 = 0x07;
const WS_CANVAS_BINARY_CACHE_CAPACITY: usize = 32;
const WS_FRAME_PAYLOAD_CACHE_CAPACITY: usize = 64;
const WS_SPECTRUM_PAYLOAD_CACHE_CAPACITY: usize = 32;
const WS_PREVIEW_SCALE_LUT_CACHE_CAPACITY: usize = 8;
const WS_CACHE_SHARD_COUNT: usize = 8;

pub(super) static WS_CLIENT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(super) static WS_TOTAL_BYTES_SENT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_CANVAS_PAYLOAD_BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_CANVAS_RAW_BODY_BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_CANVAS_RAW_BODY_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_CANVAS_JPEG_BODY_BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_CANVAS_JPEG_BODY_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_FRAME_PAYLOAD_BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_FRAME_PAYLOAD_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_SPECTRUM_PAYLOAD_BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_SPECTRUM_PAYLOAD_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);

type CanvasBinaryCacheShard = StdMutex<VecDeque<(CanvasBinaryCacheKey, Bytes)>>;
type CanvasRawBodyCacheShard = StdMutex<VecDeque<(CanvasRawBodyCacheKey, Bytes)>>;
type CanvasJpegBodyCacheShard = StdMutex<VecDeque<(CanvasJpegBodyCacheKey, Bytes)>>;
type FramePayloadCacheShard = StdMutex<VecDeque<(FramePayloadCacheKey, FrameRelayMessage)>>;
type SpectrumPayloadCacheShard = StdMutex<VecDeque<(SpectrumPayloadCacheKey, Bytes)>>;
type PreviewJpegEncoderShard = StdMutex<PreviewJpegEncoderState>;
type PreviewScaleLutCache = StdMutex<VecDeque<(u32, Arc<[u8; 256]>)>>;
type CommandRouterCache = StdMutex<Option<(usize, axum::Router)>>;

pub(super) static WS_CANVAS_BINARY_CACHE: LazyLock<Vec<CanvasBinaryCacheShard>> =
    LazyLock::new(|| {
        (0..WS_CACHE_SHARD_COUNT)
            .map(|_| {
                StdMutex::new(VecDeque::with_capacity(per_shard_capacity(
                    WS_CANVAS_BINARY_CACHE_CAPACITY,
                )))
            })
            .collect()
    });
static WS_CANVAS_RAW_BODY_CACHE: LazyLock<Vec<CanvasRawBodyCacheShard>> = LazyLock::new(|| {
    (0..WS_CACHE_SHARD_COUNT)
        .map(|_| {
            StdMutex::new(VecDeque::with_capacity(per_shard_capacity(
                WS_CANVAS_BINARY_CACHE_CAPACITY,
            )))
        })
        .collect()
});
static WS_CANVAS_JPEG_BODY_CACHE: LazyLock<Vec<CanvasJpegBodyCacheShard>> = LazyLock::new(|| {
    (0..WS_CACHE_SHARD_COUNT)
        .map(|_| {
            StdMutex::new(VecDeque::with_capacity(per_shard_capacity(
                WS_CANVAS_BINARY_CACHE_CAPACITY,
            )))
        })
        .collect()
});
pub(super) static WS_FRAME_PAYLOAD_CACHE: LazyLock<Vec<FramePayloadCacheShard>> =
    LazyLock::new(|| {
        (0..WS_CACHE_SHARD_COUNT)
            .map(|_| {
                StdMutex::new(VecDeque::with_capacity(per_shard_capacity(
                    WS_FRAME_PAYLOAD_CACHE_CAPACITY,
                )))
            })
            .collect()
    });
pub(super) static WS_SPECTRUM_PAYLOAD_CACHE: LazyLock<Vec<SpectrumPayloadCacheShard>> =
    LazyLock::new(|| {
        (0..WS_CACHE_SHARD_COUNT)
            .map(|_| {
                StdMutex::new(VecDeque::with_capacity(per_shard_capacity(
                    WS_SPECTRUM_PAYLOAD_CACHE_CAPACITY,
                )))
            })
            .collect()
    });
static WS_PREVIEW_JPEG_ENCODERS: LazyLock<Vec<PreviewJpegEncoderShard>> = LazyLock::new(|| {
    (0..WS_CACHE_SHARD_COUNT)
        .map(|_| StdMutex::new(PreviewJpegEncoderState::Uninitialized))
        .collect()
});
static WS_PREVIEW_SCALE_LUT_CACHE: LazyLock<PreviewScaleLutCache> =
    LazyLock::new(|| StdMutex::new(VecDeque::with_capacity(WS_PREVIEW_SCALE_LUT_CACHE_CAPACITY)));

/// Single-slot cache of the router used for WebSocket command dispatch. Keyed by
/// the `AppState` pointer so parallel tests with distinct states invalidate the
/// entry instead of crossing wires.
pub(super) static WS_COMMAND_ROUTER_CACHE: LazyLock<CommandRouterCache> =
    LazyLock::new(|| StdMutex::new(None));

pub(super) struct WsClientGuard;

enum PreviewJpegEncoderState {
    Uninitialized,
    Ready(PreviewJpegEncoder),
    Failed,
}

impl WsClientGuard {
    pub(super) fn register() -> Self {
        WS_CLIENT_COUNT.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for WsClientGuard {
    fn drop(&mut self) {
        WS_CLIENT_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(super) fn track_ws_bytes_sent(sent_len: usize) {
    let sent_u64 = u64::try_from(sent_len).unwrap_or(u64::MAX);
    WS_TOTAL_BYTES_SENT.fetch_add(sent_u64, Ordering::Relaxed);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CanvasBinaryCacheKey {
    pub(super) generation: u64,
    pub(super) frame_number: u32,
    pub(super) timestamp_ms: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) header: u8,
    pub(super) format_tag: u8,
    pub(super) brightness_bits: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CanvasJpegBodyCacheKey {
    generation: u64,
    storage: PublishedSurfaceStorageIdentity,
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
    brightness_bits: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CanvasRawBodyCacheKey {
    generation: u64,
    storage: PublishedSurfaceStorageIdentity,
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
    format_tag: u8,
    brightness_bits: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CanvasOutputSize {
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FramePayloadCacheKey {
    pub(super) frame_number: u32,
    pub(super) timestamp_ms: u32,
    pub(super) selection_hash: u64,
    pub(super) format: FrameFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SpectrumPayloadCacheKey {
    pub(super) timestamp_ms: u32,
    pub(super) source_bin_count: u16,
    pub(super) requested_bins: u16,
    pub(super) level_bits: u32,
    pub(super) bass_bits: u32,
    pub(super) mid_bits: u32,
    pub(super) treble_bits: u32,
    pub(super) beat: bool,
    pub(super) beat_confidence_bits: u32,
    pub(super) bpm_bits: u32,
}

#[derive(Clone)]
pub(super) enum FrameRelayMessage {
    Json(Utf8Bytes),
    Binary(Bytes),
}

pub(super) fn cached_command_router(state: &Arc<AppState>) -> axum::Router {
    let key = Arc::as_ptr(state).addr();
    if let Ok(mut guard) = WS_COMMAND_ROUTER_CACHE.lock() {
        if let Some((cached_key, router)) = guard.as_ref()
            && *cached_key == key
        {
            return router.clone();
        }
        let router = crate::api::build_router(Arc::clone(state), None);
        *guard = Some((key, router.clone()));
        return router;
    }
    // Lock poisoned — fall back to a fresh build rather than panicking.
    crate::api::build_router(Arc::clone(state), None)
}

pub(super) fn cached_frame_payload(
    frame: &hypercolor_types::event::FrameData,
    config: &ActiveFramesConfig,
) -> FrameRelayMessage {
    let key = FramePayloadCacheKey {
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        selection_hash: config.selection_hash,
        format: config.config.format,
    };

    if let Some(cached) = frame_payload_cache_get(key) {
        WS_FRAME_PAYLOAD_CACHE_HIT_COUNT.fetch_add(1, Ordering::Relaxed);
        return cached;
    }

    let payload = match config.config.format {
        FrameFormat::Binary => FrameRelayMessage::Binary(Bytes::from(
            encode_frame_binary_selected(frame, &config.selection),
        )),
        FrameFormat::Json => {
            FrameRelayMessage::Json(encode_frame_json_selected(frame, &config.selection))
        }
    };
    WS_FRAME_PAYLOAD_BUILD_COUNT.fetch_add(1, Ordering::Relaxed);
    frame_payload_cache_put(key, payload.clone());
    payload
}

#[cfg(test)]
pub(super) fn encode_frame_binary(frame: &hypercolor_types::event::FrameData) -> Vec<u8> {
    encode_frame_binary_selected(frame, &FrameZoneSelection::All)
}

pub(super) fn encode_frame_binary_selected(
    frame: &hypercolor_types::event::FrameData,
    selection: &FrameZoneSelection,
) -> Vec<u8> {
    if matches!(selection, FrameZoneSelection::All) {
        return encode_frame_binary_all(frame);
    }

    encode_filtered_frame_binary(frame, selection)
}

fn encode_frame_binary_all(frame: &hypercolor_types::event::FrameData) -> Vec<u8> {
    let max_zone_count = usize::from(u8::MAX);
    let included_zones = frame.zones.len().min(max_zone_count);
    let payload_bytes = frame
        .zones
        .iter()
        .take(included_zones)
        .map(frame_zone_binary_len)
        .sum::<usize>();

    let mut out = vec![0; 10_usize.saturating_add(payload_bytes)];
    out[0] = 0x01;
    out[1..5].copy_from_slice(&frame.frame_number.to_le_bytes());
    out[5..9].copy_from_slice(&frame.timestamp_ms.to_le_bytes());
    out[9] = u8::try_from(included_zones).unwrap_or(u8::MAX);
    let mut offset = 10;

    for zone in frame.zones.iter().take(included_zones) {
        write_frame_zone_binary(&mut out, &mut offset, zone);
    }

    out
}

fn encode_filtered_frame_binary(
    frame: &hypercolor_types::event::FrameData,
    selection: &FrameZoneSelection,
) -> Vec<u8> {
    let max_zone_count = usize::from(u8::MAX);
    let mut encoded_zone_count = 0_usize;
    let payload_bytes = frame
        .zones
        .iter()
        .filter(|zone| selection.includes(zone.zone_id.as_str()))
        .take(max_zone_count)
        .map(frame_zone_binary_len)
        .sum::<usize>();
    let mut out = vec![0; 10_usize.saturating_add(payload_bytes)];
    out[0] = 0x01;
    out[1..5].copy_from_slice(&frame.frame_number.to_le_bytes());
    out[5..9].copy_from_slice(&frame.timestamp_ms.to_le_bytes());
    let mut offset = 10;

    for zone in &frame.zones {
        if encoded_zone_count >= max_zone_count || !selection.includes(zone.zone_id.as_str()) {
            continue;
        }

        write_frame_zone_binary(&mut out, &mut offset, zone);
        encoded_zone_count = encoded_zone_count.saturating_add(1);
    }
    out[9] = u8::try_from(encoded_zone_count).unwrap_or(u8::MAX);

    out
}

fn frame_zone_binary_len(zone: &hypercolor_types::event::ZoneColors) -> usize {
    let zone_id_len = zone.zone_id.len().min(usize::from(u16::MAX));
    let led_count = zone.colors.len().min(usize::from(u16::MAX));
    2_usize
        .saturating_add(zone_id_len)
        .saturating_add(2)
        .saturating_add(led_count.saturating_mul(3))
}

fn write_frame_zone_binary(
    out: &mut [u8],
    offset: &mut usize,
    zone: &hypercolor_types::event::ZoneColors,
) {
    let zone_id_bytes = zone.zone_id.as_bytes();
    let zone_id_len_u16 = u16::try_from(zone_id_bytes.len()).unwrap_or(u16::MAX);
    let zone_id_len = usize::from(zone_id_len_u16);
    out[*offset..*offset + 2].copy_from_slice(&zone_id_len_u16.to_le_bytes());
    *offset += 2;
    out[*offset..*offset + zone_id_len].copy_from_slice(&zone_id_bytes[..zone_id_len]);
    *offset += zone_id_len;

    let led_count_u16 = u16::try_from(zone.colors.len()).unwrap_or(u16::MAX);
    out[*offset..*offset + 2].copy_from_slice(&led_count_u16.to_le_bytes());
    *offset += 2;
    let led_count = usize::from(led_count_u16);
    for color in zone.colors.iter().take(led_count) {
        out[*offset..*offset + 3].copy_from_slice(color);
        *offset += 3;
    }
}

#[derive(Serialize)]
struct BorrowedFrameZone<'a> {
    zone_id: &'a str,
    colors: &'a [[u8; 3]],
}

struct SelectedFrameZones<'a> {
    zones: &'a [hypercolor_types::event::ZoneColors],
    selection: &'a FrameZoneSelection,
}

impl Serialize for SelectedFrameZones<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        for zone in self.zones {
            if !self.selection.includes(zone.zone_id.as_str()) {
                continue;
            }
            seq.serialize_element(&BorrowedFrameZone {
                zone_id: zone.zone_id.as_str(),
                colors: zone.colors.as_slice(),
            })?;
        }
        seq.end()
    }
}

#[derive(Serialize)]
struct BorrowedFrameMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    frame_number: u32,
    timestamp_ms: u32,
    zones: SelectedFrameZones<'a>,
}

fn encode_frame_json_selected(
    frame: &hypercolor_types::event::FrameData,
    selection: &FrameZoneSelection,
) -> Utf8Bytes {
    serde_json::to_string(&BorrowedFrameMessage {
        kind: "frame",
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        zones: SelectedFrameZones {
            zones: &frame.zones,
            selection,
        },
    })
    .unwrap_or_default()
    .into()
}

pub(super) fn encode_spectrum_binary(
    spectrum: &hypercolor_types::event::SpectrumData,
    requested_bins: u16,
) -> Vec<u8> {
    let source_bins = spectrum.bins.as_slice();
    let requested_bins = usize::from(requested_bins);
    let encoded_bin_count = if source_bins.is_empty() || requested_bins == 0 {
        0
    } else {
        requested_bins.min(source_bins.len())
    };
    let bin_count_u8 = u8::try_from(encoded_bin_count).unwrap_or(u8::MAX);
    let bin_count = usize::from(bin_count_u8);

    let mut out = vec![0; 27_usize.saturating_add(bin_count.saturating_mul(4))];
    out[0] = 0x02;
    out[1..5].copy_from_slice(&spectrum.timestamp_ms.to_le_bytes());
    out[5] = bin_count_u8;
    out[6..10].copy_from_slice(&sanitize_f32(spectrum.level).to_le_bytes());
    out[10..14].copy_from_slice(&sanitize_f32(spectrum.bass).to_le_bytes());
    out[14..18].copy_from_slice(&sanitize_f32(spectrum.mid).to_le_bytes());
    out[18..22].copy_from_slice(&sanitize_f32(spectrum.treble).to_le_bytes());
    out[22] = u8::from(spectrum.beat);
    out[23..27].copy_from_slice(&sanitize_f32(spectrum.beat_confidence).to_le_bytes());
    let mut offset = 27;

    if requested_bins >= source_bins.len() {
        for value in source_bins.iter().take(bin_count) {
            out[offset..offset + 4].copy_from_slice(&sanitize_f32(*value).to_le_bytes());
            offset += 4;
        }
    } else {
        for index in 0..bin_count {
            let start = index * source_bins.len() / requested_bins;
            let end = ((index + 1) * source_bins.len() / requested_bins).min(source_bins.len());
            let slice = &source_bins[start..end];
            #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
            let avg = slice.iter().sum::<f32>() / slice.len() as f32;
            out[offset..offset + 4].copy_from_slice(&sanitize_f32(avg).to_le_bytes());
            offset += 4;
        }
    }

    out
}

pub(super) fn cached_spectrum_payload(
    spectrum: &hypercolor_types::event::SpectrumData,
    requested_bins: u16,
) -> Bytes {
    let key = SpectrumPayloadCacheKey {
        timestamp_ms: spectrum.timestamp_ms,
        source_bin_count: u16::try_from(spectrum.bins.len()).unwrap_or(u16::MAX),
        requested_bins,
        level_bits: spectrum.level.to_bits(),
        bass_bits: spectrum.bass.to_bits(),
        mid_bits: spectrum.mid.to_bits(),
        treble_bits: spectrum.treble.to_bits(),
        beat: spectrum.beat,
        beat_confidence_bits: spectrum.beat_confidence.to_bits(),
        bpm_bits: spectrum.bpm.unwrap_or_default().to_bits(),
    };

    if let Some(cached) = spectrum_payload_cache_get(key) {
        WS_SPECTRUM_PAYLOAD_CACHE_HIT_COUNT.fetch_add(1, Ordering::Relaxed);
        return cached;
    }

    let payload = Bytes::from(encode_spectrum_binary(spectrum, requested_bins));
    WS_SPECTRUM_PAYLOAD_BUILD_COUNT.fetch_add(1, Ordering::Relaxed);
    spectrum_payload_cache_put(key, payload.clone());
    payload
}

#[cfg(test)]
pub(super) fn encode_canvas_preview_binary(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    brightness: f32,
) -> Vec<u8> {
    encode_canvas_binary_with_header_and_brightness(
        canvas,
        format,
        WS_CANVAS_HEADER,
        brightness,
        0,
        0,
    )
}

#[cfg(test)]
pub(super) fn encode_cached_canvas_preview_binary(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    brightness: f32,
) -> Bytes {
    try_encode_cached_canvas_preview_binary(canvas, format, brightness, 0, 0).unwrap_or_default()
}

pub(super) fn try_encode_cached_canvas_preview_binary(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    brightness: f32,
    requested_width: u32,
    requested_height: u32,
) -> Option<Bytes> {
    let output_size = resolve_canvas_output_size(
        canvas.width,
        canvas.height,
        requested_width,
        requested_height,
    );
    if format == CanvasFormat::Jpeg {
        return try_encode_cached_canvas_jpeg_binary(
            canvas,
            WS_CANVAS_HEADER,
            brightness,
            output_size,
        );
    }

    if let Some(payload) = try_encode_cached_canvas_binary_from_body(
        canvas,
        format,
        WS_CANVAS_HEADER,
        brightness,
        output_size,
    ) {
        return Some(payload);
    }

    cached_canvas_binary(
        canvas,
        format,
        WS_CANVAS_HEADER,
        brightness,
        output_size,
        || {
            Bytes::from(encode_canvas_binary_with_header_and_brightness(
                canvas,
                format,
                WS_CANVAS_HEADER,
                brightness,
                output_size.width,
                output_size.height,
            ))
        },
    )
    .into()
}

#[cfg(test)]
pub(super) fn encode_canvas_binary_with_header(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
) -> Vec<u8> {
    encode_canvas_binary_with_header_and_brightness(canvas, format, header, 1.0, 0, 0)
}

#[cfg(test)]
pub(super) fn try_encode_cached_canvas_binary_with_header(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
) -> Option<Bytes> {
    try_encode_cached_canvas_binary_with_header_scaled(canvas, format, header, 0, 0)
}

pub(super) fn try_encode_cached_canvas_binary_with_header_scaled(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
    requested_width: u32,
    requested_height: u32,
) -> Option<Bytes> {
    let output_size = resolve_canvas_output_size(
        canvas.width,
        canvas.height,
        requested_width,
        requested_height,
    );
    if format == CanvasFormat::Jpeg {
        return try_encode_cached_canvas_jpeg_binary(canvas, header, 1.0, output_size);
    }

    if let Some(payload) =
        try_encode_cached_canvas_binary_from_body(canvas, format, header, 1.0, output_size)
    {
        return Some(payload);
    }

    cached_canvas_binary(canvas, format, header, 1.0, output_size, || {
        Bytes::from(encode_canvas_binary_with_header_and_brightness(
            canvas,
            format,
            header,
            1.0,
            output_size.width,
            output_size.height,
        ))
    })
    .into()
}

fn encode_canvas_binary_with_header_and_brightness(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
    brightness: f32,
    requested_width: u32,
    requested_height: u32,
) -> Vec<u8> {
    let output_size = resolve_canvas_output_size(
        canvas.width,
        canvas.height,
        requested_width,
        requested_height,
    );
    if format == CanvasFormat::Jpeg {
        return encode_canvas_jpeg_payload_scaled_stateless(
            canvas,
            header,
            brightness,
            output_size.width,
            output_size.height,
        )
        .unwrap_or_default();
    }
    if format == CanvasFormat::Rgba
        && brightness.clamp(0.0, 1.0) >= 0.999
        && output_size.width == canvas.width
        && output_size.height == canvas.height
    {
        return build_canvas_rgba_payload_from_source(canvas, header, output_size);
    }

    let body = encode_canvas_body(canvas, format, brightness, output_size);
    build_canvas_binary_payload(canvas, header, format, &body, output_size)
}

fn encode_canvas_body(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    brightness: f32,
    output_size: CanvasOutputSize,
) -> Vec<u8> {
    let brightness = brightness.clamp(0.0, 1.0);
    let width_u16 = u16::try_from(output_size.width).unwrap_or(u16::MAX);
    let height_u16 = u16::try_from(output_size.height).unwrap_or(u16::MAX);
    let width = usize::from(width_u16);
    let height = usize::from(height_u16);
    let px_count = width.saturating_mul(height);
    let source_width = usize::try_from(canvas.width).unwrap_or(0);
    let source_height = usize::try_from(canvas.height).unwrap_or(0);

    let bpp = match format {
        CanvasFormat::Rgb => 3_usize,
        CanvasFormat::Rgba => 4_usize,
        CanvasFormat::Jpeg => 0_usize,
    };
    let payload_len = px_count.saturating_mul(bpp);
    let mut body = vec![0; payload_len];

    let rgba = canvas.rgba_bytes();
    let scale_lut = (brightness < 0.999).then(|| preview_scale_lut(brightness));
    match format {
        CanvasFormat::Rgb => {
            if brightness >= 0.999
                && output_size.width == canvas.width
                && output_size.height == canvas.height
            {
                for (pixel, out) in rgba
                    .chunks_exact(4)
                    .take(px_count)
                    .zip(body.chunks_exact_mut(3))
                {
                    out.copy_from_slice(&pixel[..3]);
                }
            } else {
                for y in 0..height {
                    let source_y = y
                        .saturating_mul(source_height)
                        .checked_div(height.max(1))
                        .unwrap_or(0);
                    for x in 0..width {
                        let source_x = x
                            .saturating_mul(source_width)
                            .checked_div(width.max(1))
                            .unwrap_or(0);
                        let source_offset = source_y
                            .saturating_mul(source_width)
                            .saturating_add(source_x)
                            .saturating_mul(4);
                        let out_offset =
                            y.saturating_mul(width).saturating_add(x).saturating_mul(3);
                        let pixel = &rgba[source_offset..source_offset + 4];
                        if let Some(scale_lut) = scale_lut.as_ref() {
                            body[out_offset] = scale_lut[usize::from(pixel[0])];
                            body[out_offset + 1] = scale_lut[usize::from(pixel[1])];
                            body[out_offset + 2] = scale_lut[usize::from(pixel[2])];
                        } else {
                            body[out_offset..out_offset + 3].copy_from_slice(&pixel[..3]);
                        }
                    }
                }
            }
        }
        CanvasFormat::Rgba => {
            if brightness >= 0.999
                && output_size.width == canvas.width
                && output_size.height == canvas.height
            {
                body.copy_from_slice(&rgba[..payload_len]);
            } else {
                for y in 0..height {
                    let source_y = y
                        .saturating_mul(source_height)
                        .checked_div(height.max(1))
                        .unwrap_or(0);
                    for x in 0..width {
                        let source_x = x
                            .saturating_mul(source_width)
                            .checked_div(width.max(1))
                            .unwrap_or(0);
                        let source_offset = source_y
                            .saturating_mul(source_width)
                            .saturating_add(source_x)
                            .saturating_mul(4);
                        let out_offset =
                            y.saturating_mul(width).saturating_add(x).saturating_mul(4);
                        let pixel = &rgba[source_offset..source_offset + 4];
                        if let Some(scale_lut) = scale_lut.as_ref() {
                            body[out_offset] = scale_lut[usize::from(pixel[0])];
                            body[out_offset + 1] = scale_lut[usize::from(pixel[1])];
                            body[out_offset + 2] = scale_lut[usize::from(pixel[2])];
                            body[out_offset + 3] = pixel[3];
                        } else {
                            body[out_offset..out_offset + 4].copy_from_slice(pixel);
                        }
                    }
                }
            }
        }
        CanvasFormat::Jpeg => unreachable!("JPEG previews return early"),
    }

    debug_assert_eq!(body.len(), payload_len);
    body
}

fn build_canvas_binary_payload(
    canvas: &hypercolor_core::bus::CanvasFrame,
    header: u8,
    format: CanvasFormat,
    body: &[u8],
    output_size: CanvasOutputSize,
) -> Vec<u8> {
    const CANVAS_HEADER_LEN: usize = 14;

    let width_u16 = u16::try_from(output_size.width).unwrap_or(u16::MAX);
    let height_u16 = u16::try_from(output_size.height).unwrap_or(u16::MAX);
    let mut payload = vec![0; CANVAS_HEADER_LEN.saturating_add(body.len())];
    write_canvas_payload_header(
        &mut payload[..CANVAS_HEADER_LEN],
        header,
        canvas,
        width_u16,
        height_u16,
        canvas_format_tag(format),
    );
    payload[CANVAS_HEADER_LEN..].copy_from_slice(body);
    payload
}

fn build_canvas_rgba_payload_from_source(
    canvas: &hypercolor_core::bus::CanvasFrame,
    header: u8,
    output_size: CanvasOutputSize,
) -> Vec<u8> {
    const CANVAS_HEADER_LEN: usize = 14;

    let rgba = canvas.rgba_bytes();
    let width_u16 = u16::try_from(output_size.width).unwrap_or(u16::MAX);
    let height_u16 = u16::try_from(output_size.height).unwrap_or(u16::MAX);
    let payload_len = CANVAS_HEADER_LEN.saturating_add(rgba.len());
    let mut payload = vec![0; payload_len];
    write_canvas_payload_header(
        &mut payload[..CANVAS_HEADER_LEN],
        header,
        canvas,
        width_u16,
        height_u16,
        canvas_format_tag(CanvasFormat::Rgba),
    );
    payload[CANVAS_HEADER_LEN..].copy_from_slice(rgba);
    payload
}

fn try_encode_cached_canvas_binary_from_body(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
    brightness: f32,
    output_size: CanvasOutputSize,
) -> Option<Bytes> {
    let brightness = brightness.clamp(0.0, 1.0);
    if !should_cache_canvas_raw_body(format, brightness) {
        return None;
    }

    let key = CanvasBinaryCacheKey {
        generation: canvas.surface().generation(),
        frame_number: canvas.frame_number,
        timestamp_ms: canvas.timestamp_ms,
        width: output_size.width,
        height: output_size.height,
        header,
        format_tag: canvas_format_tag(format),
        brightness_bits: brightness.to_bits(),
    };
    if let Some(cached) = canvas_binary_cache_get(key) {
        WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT.fetch_add(1, Ordering::Relaxed);
        return Some(cached);
    }

    let body = cached_canvas_raw_body(canvas, format, brightness, output_size)?;
    let payload = Bytes::from(build_canvas_binary_payload(
        canvas,
        header,
        format,
        body.as_ref(),
        output_size,
    ));
    WS_CANVAS_PAYLOAD_BUILD_COUNT.fetch_add(1, Ordering::Relaxed);
    canvas_binary_cache_put(key, payload.clone());
    Some(payload)
}

fn try_encode_cached_canvas_jpeg_binary(
    canvas: &hypercolor_core::bus::CanvasFrame,
    header: u8,
    brightness: f32,
    output_size: CanvasOutputSize,
) -> Option<Bytes> {
    let brightness = brightness.clamp(0.0, 1.0);
    let key = CanvasBinaryCacheKey {
        generation: canvas.surface().generation(),
        frame_number: canvas.frame_number,
        timestamp_ms: canvas.timestamp_ms,
        width: output_size.width,
        height: output_size.height,
        header,
        format_tag: canvas_format_tag(CanvasFormat::Jpeg),
        brightness_bits: brightness.to_bits(),
    };
    if let Some(cached) = canvas_binary_cache_get(key) {
        WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT.fetch_add(1, Ordering::Relaxed);
        return Some(cached);
    }

    let jpeg_body = cached_canvas_jpeg_body(canvas, brightness, output_size)?;
    let payload = Bytes::from(build_canvas_jpeg_payload(
        canvas,
        header,
        jpeg_body.as_ref(),
        output_size,
    ));
    WS_CANVAS_PAYLOAD_BUILD_COUNT.fetch_add(1, Ordering::Relaxed);
    canvas_binary_cache_put(key, payload.clone());
    Some(payload)
}

fn cached_canvas_jpeg_body(
    canvas: &hypercolor_core::bus::CanvasFrame,
    brightness: f32,
    output_size: CanvasOutputSize,
) -> Option<Bytes> {
    let brightness = brightness.clamp(0.0, 1.0);
    let key = CanvasJpegBodyCacheKey {
        generation: canvas.surface().generation(),
        storage: canvas.surface().storage_identity(),
        width: canvas.width,
        height: canvas.height,
        output_width: output_size.width,
        output_height: output_size.height,
        brightness_bits: brightness.to_bits(),
    };
    if let Some(cached) = canvas_jpeg_body_cache_get(key) {
        WS_CANVAS_JPEG_BODY_CACHE_HIT_COUNT.fetch_add(1, Ordering::Relaxed);
        return Some(cached);
    }

    let shard_index = cache_shard_index(&key);
    let jpeg_body =
        try_encode_canvas_jpeg_body_shared(canvas, brightness, output_size, shard_index)?;
    WS_CANVAS_JPEG_BODY_BUILD_COUNT.fetch_add(1, Ordering::Relaxed);
    canvas_jpeg_body_cache_put(key, jpeg_body.clone());
    Some(jpeg_body)
}

fn cached_canvas_raw_body(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    brightness: f32,
    output_size: CanvasOutputSize,
) -> Option<Bytes> {
    let brightness = brightness.clamp(0.0, 1.0);
    let key = CanvasRawBodyCacheKey {
        generation: canvas.surface().generation(),
        storage: canvas.surface().storage_identity(),
        width: canvas.width,
        height: canvas.height,
        output_width: output_size.width,
        output_height: output_size.height,
        format_tag: canvas_format_tag(format),
        brightness_bits: brightness.to_bits(),
    };
    if let Some(cached) = canvas_raw_body_cache_get(key) {
        WS_CANVAS_RAW_BODY_CACHE_HIT_COUNT.fetch_add(1, Ordering::Relaxed);
        return Some(cached);
    }

    let body = Bytes::from(encode_canvas_body(canvas, format, brightness, output_size));
    WS_CANVAS_RAW_BODY_BUILD_COUNT.fetch_add(1, Ordering::Relaxed);
    canvas_raw_body_cache_put(key, body.clone());
    Some(body)
}

fn try_encode_canvas_jpeg_body_shared(
    canvas: &hypercolor_core::bus::CanvasFrame,
    brightness: f32,
    output_size: CanvasOutputSize,
    shard_index: usize,
) -> Option<Bytes> {
    let mut encoder = WS_PREVIEW_JPEG_ENCODERS[shard_index]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    match &mut *encoder {
        PreviewJpegEncoderState::Ready(encoder) => encoder
            .encode_scaled_body(canvas, brightness, output_size.width, output_size.height)
            .ok()
            .map(Bytes::from),
        PreviewJpegEncoderState::Failed => None,
        PreviewJpegEncoderState::Uninitialized => match PreviewJpegEncoder::new() {
            Ok(mut fresh) => {
                let payload = fresh
                    .encode_scaled_body(canvas, brightness, output_size.width, output_size.height)
                    .ok()?;
                *encoder = PreviewJpegEncoderState::Ready(fresh);
                Some(Bytes::from(payload))
            }
            Err(error) => {
                warn!(?error, "preview JPEG encoder initialization failed");
                *encoder = PreviewJpegEncoderState::Failed;
                None
            }
        },
    }
}

fn build_canvas_jpeg_payload(
    canvas: &hypercolor_core::bus::CanvasFrame,
    header: u8,
    jpeg_body: &[u8],
    output_size: CanvasOutputSize,
) -> Vec<u8> {
    const CANVAS_HEADER_LEN: usize = 14;

    let width_u16 = u16::try_from(output_size.width).unwrap_or(u16::MAX);
    let height_u16 = u16::try_from(output_size.height).unwrap_or(u16::MAX);
    let mut payload = vec![0; CANVAS_HEADER_LEN.saturating_add(jpeg_body.len())];
    write_canvas_payload_header(
        &mut payload[..CANVAS_HEADER_LEN],
        header,
        canvas,
        width_u16,
        height_u16,
        canvas_format_tag(CanvasFormat::Jpeg),
    );
    payload[CANVAS_HEADER_LEN..].copy_from_slice(jpeg_body);
    payload
}

fn write_canvas_payload_header(
    header_bytes: &mut [u8],
    header: u8,
    canvas: &hypercolor_core::bus::CanvasFrame,
    width_u16: u16,
    height_u16: u16,
    format_tag: u8,
) {
    debug_assert_eq!(header_bytes.len(), 14);
    header_bytes[0] = header;
    header_bytes[1..5].copy_from_slice(&canvas.frame_number.to_le_bytes());
    header_bytes[5..9].copy_from_slice(&canvas.timestamp_ms.to_le_bytes());
    header_bytes[9..11].copy_from_slice(&width_u16.to_le_bytes());
    header_bytes[11..13].copy_from_slice(&height_u16.to_le_bytes());
    header_bytes[13] = format_tag;
}

#[cfg(test)]
pub(super) fn reset_preview_jpeg_encoders_for_tests() {
    for shard in WS_PREVIEW_JPEG_ENCODERS.iter() {
        *shard.lock().unwrap_or_else(PoisonError::into_inner) =
            PreviewJpegEncoderState::Uninitialized;
    }
}

#[cfg(test)]
pub(super) fn reset_canvas_jpeg_body_cache_for_tests() {
    for shard in WS_CANVAS_JPEG_BODY_CACHE.iter() {
        shard.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}

#[cfg(test)]
pub(super) fn reset_canvas_raw_body_cache_for_tests() {
    for shard in WS_CANVAS_RAW_BODY_CACHE.iter() {
        shard.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}

fn preview_scale_lut(brightness: f32) -> Arc<[u8; 256]> {
    let brightness_bits = brightness.to_bits();
    {
        let mut cache = WS_PREVIEW_SCALE_LUT_CACHE
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(index) = cache
            .iter()
            .position(|(cached_bits, _)| *cached_bits == brightness_bits)
        {
            let (cached_bits, lut) = cache
                .remove(index)
                .expect("cached preview LUT should exist");
            let cached_lut = Arc::clone(&lut);
            cache.push_front((cached_bits, lut));
            return cached_lut;
        }
    }

    let mut lut = [0_u8; 256];
    if brightness <= 0.0 {
        return remember_preview_scale_lut(brightness_bits, Arc::new(lut));
    }

    for channel in 0_u16..=255 {
        let channel_u8 = u8::try_from(channel).expect("preview LUT indices fit in u8");
        lut[usize::from(channel)] = linear_to_srgb_u8(srgb_u8_to_linear(channel_u8) * brightness);
    }

    remember_preview_scale_lut(brightness_bits, Arc::new(lut))
}

fn remember_preview_scale_lut(brightness_bits: u32, lut: Arc<[u8; 256]>) -> Arc<[u8; 256]> {
    let mut cache = WS_PREVIEW_SCALE_LUT_CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(index) = cache
        .iter()
        .position(|(cached_bits, _)| *cached_bits == brightness_bits)
    {
        let _ = cache.remove(index);
    }
    cache.push_front((brightness_bits, lut));
    while cache.len() > WS_PREVIEW_SCALE_LUT_CACHE_CAPACITY {
        let _ = cache.pop_back();
    }
    Arc::clone(
        &cache
            .front()
            .expect("preview LUT cache should contain the inserted entry")
            .1,
    )
}

fn cached_canvas_binary<F>(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
    brightness: f32,
    output_size: CanvasOutputSize,
    encode: F,
) -> Bytes
where
    F: FnOnce() -> Bytes,
{
    let key = CanvasBinaryCacheKey {
        generation: canvas.surface().generation(),
        frame_number: canvas.frame_number,
        timestamp_ms: canvas.timestamp_ms,
        width: output_size.width,
        height: output_size.height,
        header,
        format_tag: canvas_format_tag(format),
        brightness_bits: brightness.to_bits(),
    };

    if let Some(cached) = canvas_binary_cache_get(key) {
        WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT.fetch_add(1, Ordering::Relaxed);
        return cached;
    }

    let payload = encode();
    WS_CANVAS_PAYLOAD_BUILD_COUNT.fetch_add(1, Ordering::Relaxed);
    canvas_binary_cache_put(key, payload.clone());
    payload
}

fn frame_payload_cache_get(key: FramePayloadCacheKey) -> Option<FrameRelayMessage> {
    let mut cache = WS_FRAME_PAYLOAD_CACHE[cache_shard_index(&key)]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let index = cache.iter().position(|(candidate, _)| *candidate == key)?;
    let (candidate, payload) = cache.remove(index)?;
    let cached = payload.clone();
    cache.push_front((candidate, payload));
    Some(cached)
}

fn frame_payload_cache_put(key: FramePayloadCacheKey, payload: FrameRelayMessage) {
    let mut cache = WS_FRAME_PAYLOAD_CACHE[cache_shard_index(&key)]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(index) = cache.iter().position(|(candidate, _)| *candidate == key) {
        let _ = cache.remove(index);
    }
    cache.push_front((key, payload));
    while cache.len() > per_shard_capacity(WS_FRAME_PAYLOAD_CACHE_CAPACITY) {
        let _ = cache.pop_back();
    }
}

fn canvas_binary_cache_get(key: CanvasBinaryCacheKey) -> Option<Bytes> {
    let mut cache = WS_CANVAS_BINARY_CACHE[cache_shard_index(&key)]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let index = cache.iter().position(|(candidate, _)| *candidate == key)?;
    let (candidate, payload) = cache.remove(index)?;
    let cached = payload.clone();
    cache.push_front((candidate, payload));
    Some(cached)
}

fn canvas_binary_cache_put(key: CanvasBinaryCacheKey, payload: Bytes) {
    let mut cache = WS_CANVAS_BINARY_CACHE[cache_shard_index(&key)]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(index) = cache.iter().position(|(candidate, _)| *candidate == key) {
        let _ = cache.remove(index);
    }
    cache.push_front((key, payload));
    while cache.len() > per_shard_capacity(WS_CANVAS_BINARY_CACHE_CAPACITY) {
        let _ = cache.pop_back();
    }
}

fn canvas_raw_body_cache_get(key: CanvasRawBodyCacheKey) -> Option<Bytes> {
    let mut cache = WS_CANVAS_RAW_BODY_CACHE[cache_shard_index(&key)]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let index = cache.iter().position(|(candidate, _)| *candidate == key)?;
    let (candidate, payload) = cache.remove(index)?;
    let cached = payload.clone();
    cache.push_front((candidate, payload));
    Some(cached)
}

fn canvas_raw_body_cache_put(key: CanvasRawBodyCacheKey, payload: Bytes) {
    let mut cache = WS_CANVAS_RAW_BODY_CACHE[cache_shard_index(&key)]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(index) = cache.iter().position(|(candidate, _)| *candidate == key) {
        let _ = cache.remove(index);
    }
    cache.push_front((key, payload));
    while cache.len() > per_shard_capacity(WS_CANVAS_BINARY_CACHE_CAPACITY) {
        let _ = cache.pop_back();
    }
}

fn canvas_jpeg_body_cache_get(key: CanvasJpegBodyCacheKey) -> Option<Bytes> {
    let mut cache = WS_CANVAS_JPEG_BODY_CACHE[cache_shard_index(&key)]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let index = cache.iter().position(|(candidate, _)| *candidate == key)?;
    let (candidate, payload) = cache.remove(index)?;
    let cached = payload.clone();
    cache.push_front((candidate, payload));
    Some(cached)
}

fn canvas_jpeg_body_cache_put(key: CanvasJpegBodyCacheKey, payload: Bytes) {
    let mut cache = WS_CANVAS_JPEG_BODY_CACHE[cache_shard_index(&key)]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(index) = cache.iter().position(|(candidate, _)| *candidate == key) {
        let _ = cache.remove(index);
    }
    cache.push_front((key, payload));
    while cache.len() > per_shard_capacity(WS_CANVAS_BINARY_CACHE_CAPACITY) {
        let _ = cache.pop_back();
    }
}

fn spectrum_payload_cache_get(key: SpectrumPayloadCacheKey) -> Option<Bytes> {
    let mut cache = WS_SPECTRUM_PAYLOAD_CACHE[cache_shard_index(&key)]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let index = cache.iter().position(|(candidate, _)| *candidate == key)?;
    let (candidate, payload) = cache.remove(index)?;
    let cached = payload.clone();
    cache.push_front((candidate, payload));
    Some(cached)
}

fn spectrum_payload_cache_put(key: SpectrumPayloadCacheKey, payload: Bytes) {
    let mut cache = WS_SPECTRUM_PAYLOAD_CACHE[cache_shard_index(&key)]
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(index) = cache.iter().position(|(candidate, _)| *candidate == key) {
        let _ = cache.remove(index);
    }
    cache.push_front((key, payload));
    while cache.len() > per_shard_capacity(WS_SPECTRUM_PAYLOAD_CACHE_CAPACITY) {
        let _ = cache.pop_back();
    }
}

const fn per_shard_capacity(total_capacity: usize) -> usize {
    let shards = if WS_CACHE_SHARD_COUNT == 0 {
        1
    } else {
        WS_CACHE_SHARD_COUNT
    };
    let per_shard = total_capacity.div_ceil(shards);
    if per_shard == 0 { 1 } else { per_shard }
}

fn cache_shard_index(key: &impl Hash) -> usize {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    usize::try_from(hasher.finish() % u64::try_from(WS_CACHE_SHARD_COUNT).unwrap_or(1))
        .unwrap_or_default()
}

const fn canvas_format_tag(format: CanvasFormat) -> u8 {
    match format {
        CanvasFormat::Rgb => 0,
        CanvasFormat::Rgba => 1,
        CanvasFormat::Jpeg => 2,
    }
}

fn should_cache_canvas_raw_body(format: CanvasFormat, brightness: f32) -> bool {
    matches!(format, CanvasFormat::Rgb) || brightness < 0.999
}

fn resolve_canvas_output_size(
    source_width: u32,
    source_height: u32,
    requested_width: u32,
    requested_height: u32,
) -> CanvasOutputSize {
    if source_width == 0 || source_height == 0 {
        return CanvasOutputSize {
            width: source_width,
            height: source_height,
        };
    }
    if requested_width == 0 && requested_height == 0 {
        return CanvasOutputSize {
            width: source_width,
            height: source_height,
        };
    }
    if requested_width == 0 {
        let height = requested_height.max(1);
        let width = u32::try_from(
            (u64::from(source_width) * u64::from(height))
                .checked_div(u64::from(source_height))
                .unwrap_or(1),
        )
        .unwrap_or(u32::MAX)
        .max(1);
        return CanvasOutputSize { width, height };
    }
    if requested_height == 0 {
        let width = requested_width.max(1);
        let height = u32::try_from(
            (u64::from(source_height) * u64::from(width))
                .checked_div(u64::from(source_width))
                .unwrap_or(1),
        )
        .unwrap_or(u32::MAX)
        .max(1);
        return CanvasOutputSize { width, height };
    }
    CanvasOutputSize {
        width: requested_width.max(1),
        height: requested_height.max(1),
    }
}

fn sanitize_f32(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}
