use std::time::Instant;

use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::input::{InputData, InteractionData, ScreenData};
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{Canvas, PublishedSurface, PublishedSurfaceStorageIdentity};
use hypercolor_core::types::event::{FrameData, FrameTiming, HypercolorEvent, SpectrumData};
use hypercolor_types::sensor::SystemSnapshot;
use std::sync::Arc;

use super::pipeline_runtime::FrameInputs;
use super::{RenderThreadState, micros_u32, usize_to_u32};

const AUDIO_LEVEL_EVENT_INTERVAL_MS: u32 = 100;

pub(crate) struct PublishFrameStats {
    pub(crate) elapsed_us: u32,
    pub(crate) full_frame_copy_count: u32,
    pub(crate) full_frame_copy_bytes: u32,
}

#[derive(Clone, Copy)]
struct AudioSignalSnapshot {
    level: f32,
    bass: f32,
    mid: f32,
    treble: f32,
    beat: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StableCanvasFrameIdentity {
    generation: u64,
    storage: PublishedSurfaceStorageIdentity,
    width: u32,
    height: u32,
}

pub(crate) async fn sample_inputs(state: &RenderThreadState, delta_secs: f32) -> FrameInputs {
    let (samples, events) = {
        let mut input_manager = state.input_manager.lock().await;
        (
            input_manager.sample_all_with_delta_secs(delta_secs),
            input_manager.drain_events(),
        )
    };

    for event in events {
        state
            .event_bus
            .publish(HypercolorEvent::InputEventReceived { event });
    }

    let mut audio = AudioData::silence();
    let mut interaction = InteractionData::default();
    let mut screen_data: Option<ScreenData> = None;
    let mut sensors = Arc::new(SystemSnapshot::empty());
    for sample in samples {
        match sample {
            InputData::Audio(snapshot) => audio = snapshot,
            InputData::Interaction(snapshot) => interaction = snapshot,
            InputData::Screen(snapshot) => screen_data = Some(snapshot),
            InputData::Sensors(snapshot) => sensors = snapshot,
            InputData::None => {}
        }
    }

    FrameInputs {
        audio,
        interaction,
        screen_data,
        sensors,
        screen_canvas: None,
        screen_sector_grid: Vec::new(),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "frame publishing needs state + all frame components"
)]
pub(crate) fn publish_frame_updates(
    state: &RenderThreadState,
    recycled_frame: &mut FrameData,
    audio: &AudioData,
    canvas: Option<Canvas>,
    frame_surface: Option<PublishedSurface>,
    preview_surface: Option<PublishedSurface>,
    screen_preview_surface: Option<PublishedSurface>,
    frame_number: u32,
    elapsed_ms: u32,
    last_audio_level_update_ms: &mut Option<u32>,
    reuse_existing_frame: bool,
    timing: FrameTiming,
) -> PublishFrameStats {
    let publish_start = Instant::now();
    let event_subscribers = state.event_bus.subscriber_count();
    let spectrum_receivers = state.event_bus.spectrum_receiver_count();
    let publish_audio_level = should_publish_audio_level_event(
        elapsed_ms,
        *last_audio_level_update_ms,
        event_subscribers > 0,
    );
    let audio_signal = (spectrum_receivers > 0 || publish_audio_level)
        .then(|| AudioSignalSnapshot::from_audio(audio));
    let mut full_frame_copy_count = 0_u32;
    let mut full_frame_copy_bytes = 0_u32;
    if !reuse_existing_frame {
        state
            .event_bus
            .frame_sender()
            .send_modify(|published_frame| {
                std::mem::swap(published_frame, recycled_frame);
                published_frame.frame_number = frame_number;
                published_frame.timestamp_ms = elapsed_ms;
            });
    }
    if spectrum_receivers > 0 {
        let audio_signal = audio_signal.as_ref().expect("audio signal should exist");
        state
            .event_bus
            .spectrum_sender()
            .send_modify(|published_spectrum| {
                update_spectrum_from_audio(published_spectrum, audio, audio_signal, elapsed_ms);
            });
    }
    maybe_publish_audio_level_event(
        state,
        audio,
        audio_signal.as_ref(),
        elapsed_ms,
        last_audio_level_update_ms,
        publish_audio_level,
    );
    state
        .preview_runtime
        .note_canvas_frame(frame_number, elapsed_ms);
    let canvas_receivers = state.preview_canvas_receiver_count();
    if canvas_receivers > 0 {
        let canvas_frame = if let Some(surface) = preview_surface.or(frame_surface) {
            CanvasFrame::from_surface(surface.with_frame_metadata(frame_number, elapsed_ms))
        } else if let Some(canvas) = canvas {
            let canvas_rgba_len = usize_to_u32(canvas.rgba_len());
            let (frame, copied) =
                CanvasFrame::from_owned_canvas_with_copy_info(canvas, frame_number, elapsed_ms);
            if copied {
                full_frame_copy_count = full_frame_copy_count.saturating_add(1);
                full_frame_copy_bytes = full_frame_copy_bytes.saturating_add(canvas_rgba_len);
            }
            frame
        } else {
            CanvasFrame::empty()
        };
        let publish_canvas = {
            let current = state.event_bus.canvas_sender().borrow();
            should_publish_canvas_frame(&current, &canvas_frame)
        };
        if publish_canvas {
            state
                .preview_runtime
                .record_canvas_publication(frame_number, elapsed_ms);
            let _ = state.event_bus.canvas_sender().send(canvas_frame);
        }
    }
    state
        .preview_runtime
        .note_screen_canvas_frame(frame_number, elapsed_ms);
    let screen_canvas_receivers = state.event_bus.screen_canvas_receiver_count();
    if screen_canvas_receivers > 0 {
        let screen_frame = if let Some(surface) = screen_preview_surface {
            CanvasFrame::from_surface(surface.with_frame_metadata(frame_number, elapsed_ms))
        } else {
            CanvasFrame::empty()
        };
        let publish_screen = {
            let current = state.event_bus.screen_canvas_sender().borrow();
            should_publish_canvas_frame(&current, &screen_frame)
        };
        if publish_screen {
            state
                .preview_runtime
                .record_screen_canvas_publication(frame_number, elapsed_ms);
            let _ = state.event_bus.screen_canvas_sender().send(screen_frame);
        }
    }
    if event_subscribers > 0 {
        state.event_bus.publish(HypercolorEvent::FrameRendered {
            frame_number,
            timing,
        });
    }
    PublishFrameStats {
        elapsed_us: micros_u32(publish_start.elapsed()),
        full_frame_copy_count,
        full_frame_copy_bytes,
    }
}

fn should_publish_canvas_frame(current: &CanvasFrame, next: &CanvasFrame) -> bool {
    stable_canvas_frame_identity(current) != stable_canvas_frame_identity(next)
}

fn stable_canvas_frame_identity(frame: &CanvasFrame) -> Option<StableCanvasFrameIdentity> {
    (frame.width > 0 && frame.height > 0).then(|| StableCanvasFrameIdentity {
        generation: frame.surface().generation(),
        storage: frame.surface().storage_identity(),
        width: frame.width,
        height: frame.height,
    })
}

fn maybe_publish_audio_level_event(
    state: &RenderThreadState,
    audio: &AudioData,
    signal: Option<&AudioSignalSnapshot>,
    elapsed_ms: u32,
    last_audio_level_update_ms: &mut Option<u32>,
    should_publish: bool,
) {
    if !should_publish {
        return;
    }

    *last_audio_level_update_ms = Some(elapsed_ms);
    let signal = signal
        .copied()
        .unwrap_or_else(|| AudioSignalSnapshot::from_audio(audio));
    state.event_bus.publish(HypercolorEvent::AudioLevelUpdate {
        level: signal.level,
        bass: signal.bass,
        mid: signal.mid,
        treble: signal.treble,
        beat: signal.beat,
    });
}

fn should_publish_audio_level_event(
    elapsed_ms: u32,
    last_audio_level_update_ms: Option<u32>,
    has_event_subscribers: bool,
) -> bool {
    has_event_subscribers
        && !last_audio_level_update_ms.is_some_and(|last_sent| {
            elapsed_ms.saturating_sub(last_sent) < AUDIO_LEVEL_EVENT_INTERVAL_MS
        })
}

fn update_spectrum_from_audio(
    spectrum: &mut SpectrumData,
    audio: &AudioData,
    signal: &AudioSignalSnapshot,
    timestamp_ms: u32,
) {
    spectrum.timestamp_ms = timestamp_ms;
    spectrum.level = signal.level;
    spectrum.bass = signal.bass;
    spectrum.mid = signal.mid;
    spectrum.treble = signal.treble;
    spectrum.beat = signal.beat;
    spectrum.beat_confidence = audio.beat_confidence;
    spectrum.bpm = (audio.bpm > 0.0).then_some(audio.bpm);
    spectrum.bins.clone_from(&audio.spectrum);
}

impl AudioSignalSnapshot {
    fn from_audio(audio: &AudioData) -> Self {
        Self {
            level: audio.rms_level,
            bass: audio.bass(),
            mid: audio.mid(),
            treble: audio.treble(),
            beat: audio.beat_detected,
        }
    }
}

pub(crate) fn screen_data_to_canvas(
    screen_data: &ScreenData,
    canvas_width: u32,
    canvas_height: u32,
    sector_grid: &mut Vec<[u8; 3]>,
) -> Option<Canvas> {
    if let Some(surface) = &screen_data.canvas_downscale
        && surface.width() == canvas_width
        && surface.height() == canvas_height
    {
        return Some(Canvas::from_published_surface(surface));
    }

    if canvas_width == 0 || canvas_height == 0 {
        return None;
    }

    let mut max_row = 0_u32;
    let mut max_col = 0_u32;
    let mut saw_sector = false;

    for zone in &screen_data.zone_colors {
        let Some((row, col)) = parse_sector_zone_id(&zone.zone_id) else {
            continue;
        };
        let _color = zone.colors.first().copied().unwrap_or([0, 0, 0]);
        max_row = max_row.max(row);
        max_col = max_col.max(col);
        saw_sector = true;
    }

    if !saw_sector {
        return None;
    }

    let rows = max_row.saturating_add(1);
    let cols = max_col.saturating_add(1);
    let cell_count = usize::try_from(rows).ok().and_then(|row_count| {
        usize::try_from(cols)
            .ok()
            .and_then(|col_count| row_count.checked_mul(col_count))
    })?;

    if sector_grid.len() == cell_count {
        sector_grid.fill([0, 0, 0]);
    } else {
        sector_grid.resize(cell_count, [0, 0, 0]);
    }
    for zone in &screen_data.zone_colors {
        let Some((row, col)) = parse_sector_zone_id(&zone.zone_id) else {
            continue;
        };
        let color = zone.colors.first().copied().unwrap_or([0, 0, 0]);
        let idx_u64 = u64::from(row)
            .checked_mul(u64::from(cols))
            .and_then(|base| base.checked_add(u64::from(col)))?;
        let idx = usize::try_from(idx_u64).ok()?;
        if let Some(cell) = sector_grid.get_mut(idx) {
            *cell = color;
        }
    }

    let mut canvas = Canvas::new(canvas_width, canvas_height);
    let pixels = canvas.as_rgba_bytes_mut();
    let width_u64 = u64::from(canvas_width);
    let height_u64 = u64::from(canvas_height);
    let grid_cols_u64 = u64::from(cols);
    let grid_rows_u64 = u64::from(rows);
    let canvas_width_usize = usize::try_from(canvas_width).ok()?;

    for y in 0..canvas_height {
        let mapped_row_u64 = (u64::from(y) * grid_rows_u64) / height_u64;
        let row = u32::try_from(mapped_row_u64)
            .unwrap_or_default()
            .min(rows.saturating_sub(1));
        let row_offset = usize::try_from(y)
            .ok()?
            .checked_mul(canvas_width_usize)?
            .checked_mul(4)?;

        for x in 0..canvas_width {
            let mapped_col_u64 = (u64::from(x) * grid_cols_u64) / width_u64;
            let col = u32::try_from(mapped_col_u64)
                .unwrap_or_default()
                .min(cols.saturating_sub(1));

            let idx_u64 = u64::from(row)
                .checked_mul(grid_cols_u64)
                .and_then(|base| base.checked_add(u64::from(col)))
                .unwrap_or_default();
            let idx = usize::try_from(idx_u64).unwrap_or_default();
            let [r, g, b] = sector_grid.get(idx).copied().unwrap_or([0, 0, 0]);
            let pixel_offset = row_offset.checked_add(usize::try_from(x).ok()?.checked_mul(4)?)?;
            pixels[pixel_offset] = r;
            pixels[pixel_offset + 1] = g;
            pixels[pixel_offset + 2] = b;
            pixels[pixel_offset + 3] = 255;
        }
    }

    Some(canvas)
}

pub(crate) fn parse_sector_zone_id(zone_id: &str) -> Option<(u32, u32)> {
    let coords = zone_id.strip_prefix("screen:sector_")?;
    let (row_raw, col_raw) = coords.split_once('_')?;
    let row = row_raw.parse().ok()?;
    let col = col_raw.parse().ok()?;
    Some((row, col))
}
