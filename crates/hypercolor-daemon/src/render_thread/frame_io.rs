use std::time::Instant;

use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::input::{InputData, InteractionData, ScreenData};
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{Canvas, PublishedSurface, Rgba};
use hypercolor_core::types::event::{FrameData, FrameTiming, HypercolorEvent, SpectrumData};

use super::pipeline_runtime::FrameInputs;
use super::{RenderThreadState, micros_u32, usize_to_u32};

const AUDIO_LEVEL_EVENT_INTERVAL_MS: u32 = 100;

pub(crate) struct PublishFrameStats {
    pub(crate) elapsed_us: u32,
    pub(crate) full_frame_copy_count: u32,
    pub(crate) full_frame_copy_bytes: u32,
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
    for sample in samples {
        match sample {
            InputData::Audio(snapshot) => audio = snapshot,
            InputData::Interaction(snapshot) => interaction = snapshot,
            InputData::Screen(snapshot) => screen_data = Some(snapshot),
            InputData::None => {}
        }
    }

    let screen_canvas = screen_data
        .as_ref()
        .and_then(|data| screen_data_to_canvas(data, state.canvas_width, state.canvas_height));
    let screen_preview_surface = screen_data
        .as_ref()
        .and_then(|data| data.canvas_downscale.clone());

    FrameInputs {
        audio,
        interaction,
        screen_data,
        screen_canvas,
        screen_preview_surface,
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
    canvas: Canvas,
    frame_surface: Option<PublishedSurface>,
    screen_preview_surface: Option<PublishedSurface>,
    frame_number: u32,
    elapsed_ms: u32,
    last_audio_level_update_ms: &mut Option<u32>,
    timing: FrameTiming,
) -> PublishFrameStats {
    let publish_start = Instant::now();
    let event_subscribers = state.event_bus.subscriber_count();
    let mut full_frame_copy_count = 0_u32;
    let mut full_frame_copy_bytes = 0_u32;
    recycled_frame.frame_number = frame_number;
    recycled_frame.timestamp_ms = elapsed_ms;
    let published_frame = FrameData::new(
        std::mem::take(&mut recycled_frame.zones),
        frame_number,
        elapsed_ms,
    );
    *recycled_frame = state.event_bus.frame_sender().send_replace(published_frame);
    let _ = state
        .event_bus
        .spectrum_sender()
        .send(spectrum_from_audio(audio, elapsed_ms));
    maybe_publish_audio_level_event(
        state,
        audio,
        elapsed_ms,
        last_audio_level_update_ms,
        event_subscribers > 0,
    );
    let canvas_frame = if let Some(surface) = frame_surface {
        CanvasFrame::from_surface(surface.with_frame_metadata(frame_number, elapsed_ms))
    } else {
        let canvas_rgba_len = usize_to_u32(canvas.rgba_len());
        let (frame, copied) =
            CanvasFrame::from_owned_canvas_with_copy_info(canvas, frame_number, elapsed_ms);
        if copied {
            full_frame_copy_count = full_frame_copy_count.saturating_add(1);
            full_frame_copy_bytes = full_frame_copy_bytes.saturating_add(canvas_rgba_len);
        }
        frame
    };
    state
        .preview_runtime
        .record_canvas_publication(&canvas_frame);
    let _ = state.event_bus.canvas_sender().send(canvas_frame);
    let screen_frame = if let Some(surface) = screen_preview_surface {
        CanvasFrame::from_surface(surface.with_frame_metadata(frame_number, elapsed_ms))
    } else {
        CanvasFrame::empty()
    };
    state
        .preview_runtime
        .record_screen_canvas_publication(&screen_frame);
    let _ = state.event_bus.screen_canvas_sender().send(screen_frame);
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

fn maybe_publish_audio_level_event(
    state: &RenderThreadState,
    audio: &AudioData,
    elapsed_ms: u32,
    last_audio_level_update_ms: &mut Option<u32>,
    has_event_subscribers: bool,
) {
    if !has_event_subscribers {
        return;
    }

    if last_audio_level_update_ms.is_some_and(|last_sent| {
        elapsed_ms.saturating_sub(last_sent) < AUDIO_LEVEL_EVENT_INTERVAL_MS
    }) {
        return;
    }

    *last_audio_level_update_ms = Some(elapsed_ms);
    state.event_bus.publish(HypercolorEvent::AudioLevelUpdate {
        level: audio.rms_level,
        bass: audio.bass(),
        mid: audio.mid(),
        treble: audio.treble(),
        beat: audio.beat_detected,
    });
}

fn spectrum_from_audio(audio: &AudioData, timestamp_ms: u32) -> SpectrumData {
    SpectrumData {
        timestamp_ms,
        level: audio.rms_level,
        bass: audio.bass(),
        mid: audio.mid(),
        treble: audio.treble(),
        beat: audio.beat_detected,
        beat_confidence: audio.beat_confidence,
        bpm: if audio.bpm > 0.0 {
            Some(audio.bpm)
        } else {
            None
        },
        bins: audio.spectrum.clone(),
    }
}

pub(crate) fn screen_data_to_canvas(
    screen_data: &ScreenData,
    canvas_width: u32,
    canvas_height: u32,
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

    let mut sectors: Vec<(u32, u32, [u8; 3])> = Vec::new();
    let mut max_row = 0_u32;
    let mut max_col = 0_u32;

    for zone in &screen_data.zone_colors {
        let Some((row, col)) = parse_sector_zone_id(&zone.zone_id) else {
            continue;
        };
        let color = zone.colors.first().copied().unwrap_or([0, 0, 0]);
        max_row = max_row.max(row);
        max_col = max_col.max(col);
        sectors.push((row, col, color));
    }

    if sectors.is_empty() {
        return None;
    }

    let rows = max_row.saturating_add(1);
    let cols = max_col.saturating_add(1);
    let cell_count = usize::try_from(rows).ok().and_then(|row_count| {
        usize::try_from(cols)
            .ok()
            .and_then(|col_count| row_count.checked_mul(col_count))
    })?;

    let mut grid = vec![[0, 0, 0]; cell_count];
    for (row, col, color) in sectors {
        let idx_u64 = u64::from(row)
            .checked_mul(u64::from(cols))
            .and_then(|base| base.checked_add(u64::from(col)))?;
        let idx = usize::try_from(idx_u64).ok()?;
        if let Some(cell) = grid.get_mut(idx) {
            *cell = color;
        }
    }

    let mut canvas = Canvas::new(canvas_width, canvas_height);
    let width_u64 = u64::from(canvas_width);
    let height_u64 = u64::from(canvas_height);
    let grid_cols_u64 = u64::from(cols);
    let grid_rows_u64 = u64::from(rows);

    for y in 0..canvas_height {
        let mapped_row_u64 = (u64::from(y) * grid_rows_u64) / height_u64;
        let row = u32::try_from(mapped_row_u64)
            .unwrap_or_default()
            .min(rows.saturating_sub(1));

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
            let [r, g, b] = grid.get(idx).copied().unwrap_or([0, 0, 0]);
            canvas.set_pixel(x, y, Rgba::new(r, g, b, 255));
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
