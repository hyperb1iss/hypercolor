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

use hypercolor_types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

use super::protocol::{ActiveFramesConfig, CanvasFormat, FrameFormat, FrameZoneSelection};
use crate::api::AppState;

/// Maximum number of events that can be buffered per WebSocket client.
pub(super) const WS_BUFFER_SIZE: usize = 64;
pub(super) const WS_CANVAS_BYTES_PER_PIXEL_RGBA: u64 = 4;
pub(super) const WS_CANVAS_HEADER: u8 = 0x03;
pub(super) const WS_SCREEN_CANVAS_HEADER: u8 = 0x05;
const WS_CANVAS_BINARY_CACHE_CAPACITY: usize = 32;
const WS_FRAME_PAYLOAD_CACHE_CAPACITY: usize = 64;
const WS_SPECTRUM_PAYLOAD_CACHE_CAPACITY: usize = 32;
const WS_PREVIEW_SCALE_LUT_CACHE_CAPACITY: usize = 8;
const WS_CACHE_SHARD_COUNT: usize = 8;

pub(super) static WS_CLIENT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(super) static WS_TOTAL_BYTES_SENT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_CANVAS_PAYLOAD_BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_FRAME_PAYLOAD_BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_FRAME_PAYLOAD_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_SPECTRUM_PAYLOAD_BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_SPECTRUM_PAYLOAD_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);

type CanvasBinaryCacheShard = StdMutex<VecDeque<(CanvasBinaryCacheKey, Bytes)>>;
type FramePayloadCacheShard = StdMutex<VecDeque<(FramePayloadCacheKey, FrameRelayMessage)>>;
type SpectrumPayloadCacheShard = StdMutex<VecDeque<(SpectrumPayloadCacheKey, Bytes)>>;
type PreviewScaleLutCache = StdMutex<VecDeque<(u32, [u8; 256])>>;
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
static WS_PREVIEW_SCALE_LUT_CACHE: LazyLock<PreviewScaleLutCache> =
    LazyLock::new(|| StdMutex::new(VecDeque::with_capacity(WS_PREVIEW_SCALE_LUT_CACHE_CAPACITY)));

/// Single-slot cache of the router used for WebSocket command dispatch. Keyed by
/// the `AppState` pointer so parallel tests with distinct states invalidate the
/// entry instead of crossing wires.
pub(super) static WS_COMMAND_ROUTER_CACHE: LazyLock<CommandRouterCache> =
    LazyLock::new(|| StdMutex::new(None));

pub(super) struct WsClientGuard;

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
    let payload_bytes =
        frame
            .zones
            .iter()
            .take(included_zones)
            .fold(0_usize, |payload_bytes, zone| {
                let zone_id_len = zone.zone_id.len().min(usize::from(u16::MAX));
                let led_count = zone.colors.len().min(usize::from(u16::MAX));
                payload_bytes.saturating_add(
                    2_usize
                        .saturating_add(zone_id_len)
                        .saturating_add(2)
                        .saturating_add(led_count.saturating_mul(3)),
                )
            });

    let mut out = Vec::with_capacity(10_usize.saturating_add(payload_bytes));
    out.push(0x01);
    out.extend_from_slice(&frame.frame_number.to_le_bytes());
    out.extend_from_slice(&frame.timestamp_ms.to_le_bytes());
    out.push(u8::try_from(included_zones).unwrap_or(u8::MAX));

    for zone in frame.zones.iter().take(included_zones) {
        encode_frame_zone_binary(&mut out, zone);
    }

    out
}

fn encode_filtered_frame_binary(
    frame: &hypercolor_types::event::FrameData,
    selection: &FrameZoneSelection,
) -> Vec<u8> {
    let max_zone_count = usize::from(u8::MAX);
    let mut out = Vec::with_capacity(
        10_usize.saturating_add(frame.zones.len().min(max_zone_count).saturating_mul(16)),
    );
    out.push(0x01);
    out.extend_from_slice(&frame.frame_number.to_le_bytes());
    out.extend_from_slice(&frame.timestamp_ms.to_le_bytes());
    let zone_count_index = out.len();
    out.push(0);

    let mut encoded_zone_count = 0_u8;
    for zone in &frame.zones {
        if usize::from(encoded_zone_count) >= max_zone_count
            || !selection.includes(zone.zone_id.as_str())
        {
            continue;
        }

        encode_frame_zone_binary(&mut out, zone);
        encoded_zone_count = encoded_zone_count.saturating_add(1);
    }
    out[zone_count_index] = encoded_zone_count;

    out
}

fn encode_frame_zone_binary(out: &mut Vec<u8>, zone: &hypercolor_types::event::ZoneColors) {
    let zone_id_bytes = zone.zone_id.as_bytes();
    let zone_id_len_u16 = u16::try_from(zone_id_bytes.len()).unwrap_or(u16::MAX);
    let zone_id_len = usize::from(zone_id_len_u16);
    out.extend_from_slice(&zone_id_len_u16.to_le_bytes());
    out.extend_from_slice(&zone_id_bytes[..zone_id_len]);

    let led_count_u16 = u16::try_from(zone.colors.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&led_count_u16.to_le_bytes());
    let led_count = usize::from(led_count_u16);
    for color in zone.colors.iter().take(led_count) {
        out.extend_from_slice(color);
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

    let mut out = Vec::with_capacity(27_usize.saturating_add(bin_count.saturating_mul(4)));
    out.push(0x02);
    out.extend_from_slice(&spectrum.timestamp_ms.to_le_bytes());
    out.push(bin_count_u8);
    out.extend_from_slice(&sanitize_f32(spectrum.level).to_le_bytes());
    out.extend_from_slice(&sanitize_f32(spectrum.bass).to_le_bytes());
    out.extend_from_slice(&sanitize_f32(spectrum.mid).to_le_bytes());
    out.extend_from_slice(&sanitize_f32(spectrum.treble).to_le_bytes());
    out.push(u8::from(spectrum.beat));
    out.extend_from_slice(&sanitize_f32(spectrum.beat_confidence).to_le_bytes());

    if requested_bins >= source_bins.len() {
        for value in source_bins.iter().take(bin_count) {
            out.extend_from_slice(&sanitize_f32(*value).to_le_bytes());
        }
    } else {
        for index in 0..bin_count {
            let start = index * source_bins.len() / requested_bins;
            let end = ((index + 1) * source_bins.len() / requested_bins).min(source_bins.len());
            let slice = &source_bins[start..end];
            #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
            let avg = slice.iter().sum::<f32>() / slice.len() as f32;
            out.extend_from_slice(&sanitize_f32(avg).to_le_bytes());
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

pub(super) fn encode_canvas_preview_binary(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    brightness: f32,
) -> Vec<u8> {
    encode_canvas_binary_with_header_and_brightness(canvas, format, WS_CANVAS_HEADER, brightness)
}

pub(super) fn encode_cached_canvas_preview_binary(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    brightness: f32,
) -> Bytes {
    cached_canvas_binary(canvas, format, WS_CANVAS_HEADER, brightness, || {
        Bytes::from(encode_canvas_preview_binary(canvas, format, brightness))
    })
}

pub(super) fn encode_canvas_binary_with_header(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
) -> Vec<u8> {
    encode_canvas_binary_with_header_and_brightness(canvas, format, header, 1.0)
}

pub(super) fn encode_cached_canvas_binary_with_header(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
) -> Bytes {
    cached_canvas_binary(canvas, format, header, 1.0, || {
        Bytes::from(encode_canvas_binary_with_header(canvas, format, header))
    })
}

fn encode_canvas_binary_with_header_and_brightness(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
    brightness: f32,
) -> Vec<u8> {
    const CANVAS_HEADER_LEN: usize = 14;

    let brightness = brightness.clamp(0.0, 1.0);
    let width_u16 = u16::try_from(canvas.width).unwrap_or(u16::MAX);
    let height_u16 = u16::try_from(canvas.height).unwrap_or(u16::MAX);
    let width = usize::from(width_u16);
    let height = usize::from(height_u16);
    let px_count = width.saturating_mul(height);

    let bpp = match format {
        CanvasFormat::Rgb => 3_usize,
        CanvasFormat::Rgba => 4_usize,
    };
    let payload_len = px_count.saturating_mul(bpp);
    let mut out = Vec::with_capacity(CANVAS_HEADER_LEN.saturating_add(payload_len));
    out.push(header);
    out.extend_from_slice(&canvas.frame_number.to_le_bytes());
    out.extend_from_slice(&canvas.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&width_u16.to_le_bytes());
    out.extend_from_slice(&height_u16.to_le_bytes());
    out.push(match format {
        CanvasFormat::Rgb => 0,
        CanvasFormat::Rgba => 1,
    });

    let rgba = canvas.rgba_bytes();
    let scale_lut = (brightness < 0.999).then(|| preview_scale_lut(brightness));
    match format {
        CanvasFormat::Rgb => {
            if brightness >= 0.999 {
                for pixel in rgba.chunks_exact(4).take(px_count) {
                    out.extend_from_slice(&pixel[..3]);
                }
            } else {
                let scale_lut = scale_lut
                    .as_ref()
                    .expect("dimmed preview path should precompute scale table");
                for pixel in rgba.chunks_exact(4).take(px_count) {
                    out.push(scale_lut[usize::from(pixel[0])]);
                    out.push(scale_lut[usize::from(pixel[1])]);
                    out.push(scale_lut[usize::from(pixel[2])]);
                }
            }
        }
        CanvasFormat::Rgba => {
            if brightness >= 0.999 {
                out.extend_from_slice(&rgba[..payload_len]);
            } else {
                let scale_lut = scale_lut
                    .as_ref()
                    .expect("dimmed preview path should precompute scale table");
                for pixel in rgba.chunks_exact(4).take(px_count) {
                    out.push(scale_lut[usize::from(pixel[0])]);
                    out.push(scale_lut[usize::from(pixel[1])]);
                    out.push(scale_lut[usize::from(pixel[2])]);
                    out.push(pixel[3]);
                }
            }
        }
    }

    debug_assert_eq!(out.len(), CANVAS_HEADER_LEN.saturating_add(payload_len));
    out
}

fn preview_scale_lut(brightness: f32) -> [u8; 256] {
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
            let cached_lut = lut;
            cache.push_front((cached_bits, lut));
            return cached_lut;
        }
    }

    let mut lut = [0_u8; 256];
    if brightness <= 0.0 {
        return remember_preview_scale_lut(brightness_bits, lut);
    }

    for channel in 0_u16..=255 {
        let channel_u8 = u8::try_from(channel).expect("preview LUT indices fit in u8");
        lut[usize::from(channel)] = linear_to_srgb_u8(srgb_u8_to_linear(channel_u8) * brightness);
    }

    remember_preview_scale_lut(brightness_bits, lut)
}

fn remember_preview_scale_lut(brightness_bits: u32, lut: [u8; 256]) -> [u8; 256] {
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
    lut
}

fn cached_canvas_binary<F>(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
    brightness: f32,
    encode: F,
) -> Bytes
where
    F: FnOnce() -> Bytes,
{
    let key = CanvasBinaryCacheKey {
        generation: canvas.surface().generation(),
        frame_number: canvas.frame_number,
        timestamp_ms: canvas.timestamp_ms,
        width: canvas.width,
        height: canvas.height,
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
    }
}

fn sanitize_f32(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}
