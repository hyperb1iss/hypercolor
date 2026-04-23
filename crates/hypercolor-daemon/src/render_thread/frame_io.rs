use std::time::Instant;

use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{Canvas, PublishedSurface, PublishedSurfaceStorageIdentity};
use hypercolor_core::types::event::{FrameData, FrameTiming, HypercolorEvent, SpectrumData};
use hypercolor_types::scene::RenderGroupId;
use tokio::sync::watch;

use super::pipeline_runtime::PublicationCadenceState;
use super::render_groups::GroupCanvasFrame;
use super::{RenderThreadState, micros_u32, usize_to_u32};

pub(crate) struct PublishFrameStats {
    pub(crate) elapsed_us: u32,
    pub(crate) full_frame_copy_count: u32,
    pub(crate) full_frame_copy_bytes: u32,
    pub(crate) frame_data_us: u32,
    pub(crate) group_canvas_us: u32,
    pub(crate) preview_us: u32,
    pub(crate) events_us: u32,
}

pub(crate) struct FramePublicationSurfaces {
    pub(crate) canvas: Option<Canvas>,
    pub(crate) frame_surface: Option<PublishedSurface>,
    pub(crate) preview_surface: Option<PublishedSurface>,
    pub(crate) screen_capture_surface: Option<PublishedSurface>,
    pub(crate) web_viewport_preview_canvas: Option<Canvas>,
    pub(crate) effect_running: bool,
    pub(crate) screen_capture_active: bool,
}

impl FramePublicationSurfaces {
    fn authoritative_scene_surface(&self) -> Option<&PublishedSurface> {
        authoritative_scene_surface(self.frame_surface.as_ref(), self.preview_surface.as_ref())
    }

    fn canvas_preview_surface(&self) -> Option<&PublishedSurface> {
        self.preview_surface
            .as_ref()
            .or(self.frame_surface.as_ref())
    }

    fn screen_watch_surface(&self) -> Option<&PublishedSurface> {
        if !self.effect_running && self.screen_capture_active {
            self.preview_surface
                .as_ref()
                .or(self.frame_surface.as_ref())
                .or(self.screen_capture_surface.as_ref())
        } else {
            self.preview_surface
                .as_ref()
                .or(self.screen_capture_surface.as_ref())
        }
    }
}

pub(crate) struct FramePublicationRequest<'a> {
    pub(crate) recycled_frame: &'a mut FrameData,
    pub(crate) audio: &'a AudioData,
    pub(crate) surfaces: FramePublicationSurfaces,
    pub(crate) group_canvases: &'a [(RenderGroupId, GroupCanvasFrame)],
    pub(crate) active_group_canvas_ids: &'a [RenderGroupId],
    pub(crate) frame_number: u32,
    pub(crate) elapsed_ms: u32,
    pub(crate) reuse_existing_frame: bool,
    pub(crate) refresh_existing_frame_metadata: bool,
    pub(crate) timing: FrameTiming,
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

pub(crate) fn publish_frame_updates(
    state: &RenderThreadState,
    publication_cadence: &mut PublicationCadenceState,
    request: FramePublicationRequest<'_>,
) -> PublishFrameStats {
    let FramePublicationRequest {
        recycled_frame,
        audio,
        mut surfaces,
        group_canvases,
        active_group_canvas_ids,
        frame_number,
        elapsed_ms,
        reuse_existing_frame,
        refresh_existing_frame_metadata,
        timing,
    } = request;
    let publish_start = Instant::now();
    let event_subscribers = state.event_bus.subscriber_count();
    let spectrum_receivers = state.event_bus.spectrum_receiver_count();
    let publish_audio_level =
        publication_cadence.should_publish_audio_level(elapsed_ms, event_subscribers > 0);
    let audio_signal = (spectrum_receivers > 0 || publish_audio_level)
        .then(|| AudioSignalSnapshot::from_audio(audio));
    let mut full_frame_copy_count = 0_u32;
    let mut full_frame_copy_bytes = 0_u32;
    let frame_data_start = Instant::now();
    update_published_frame(
        state.event_bus.frame_sender(),
        recycled_frame,
        frame_number,
        elapsed_ms,
        reuse_existing_frame,
        refresh_existing_frame_metadata,
    );
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
        publication_cadence,
        publish_audio_level,
    );
    let frame_data_us = micros_u32(frame_data_start.elapsed());
    let group_canvas_start = Instant::now();
    let group_canvas_senders = state
        .event_bus
        .retain_group_canvases_and_collect_senders(active_group_canvas_ids)
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    for (group_id, group_canvas) in group_canvases {
        state
            .event_bus
            .upsert_display_group_target(*group_id, (&group_canvas.display_target).into());
        let Some(sender) = group_canvas_senders.get(group_id) else {
            continue;
        };
        let surface = group_canvas
            .surface
            .with_frame_metadata(frame_number, elapsed_ms);
        let publish_group_canvas = {
            let current = sender.borrow();
            should_publish_surface_frame(&current, &surface)
        };
        if publish_group_canvas {
            sender.send_replace(CanvasFrame::from_surface(surface));
        }
    }
    let group_canvas_us = micros_u32(group_canvas_start.elapsed());
    let preview_start = Instant::now();
    let scene_canvas_receivers = state.scene_canvas_receiver_count();
    if scene_canvas_receivers > 0 {
        let publish_scene_canvas = {
            let current = state.event_bus.scene_canvas_sender().borrow();
            if let Some(surface) = surfaces.authoritative_scene_surface() {
                should_publish_surface_frame(&current, surface)
            } else {
                should_publish_canvas_frame(&current, &CanvasFrame::empty())
            }
        };
        if publish_scene_canvas {
            let scene_frame = if let Some(surface) = surfaces.authoritative_scene_surface() {
                CanvasFrame::from_surface(
                    surface
                        .clone()
                        .with_frame_metadata(frame_number, elapsed_ms),
                )
            } else {
                CanvasFrame::empty()
            };
            let _ = state.event_bus.scene_canvas_sender().send(scene_frame);
        }
    }
    let screen_canvas_receivers = state.event_bus.screen_canvas_receiver_count();
    let screen_preview_surface = if screen_canvas_receivers > 0 {
        surfaces.screen_watch_surface().cloned()
    } else {
        None
    };
    state
        .preview_runtime
        .note_canvas_frame(frame_number, elapsed_ms);
    let canvas_receivers = state.preview_canvas_receiver_count();
    if canvas_receivers > 0 {
        let tracked_canvas_receivers = state.preview_runtime.tracked_canvas_receiver_count();
        let publish_canvas = {
            let current = state.event_bus.canvas_sender().borrow();
            let changed = if let Some(surface) = surfaces.canvas_preview_surface() {
                should_publish_surface_frame(&current, surface)
            } else if let Some(canvas) = surfaces.canvas.as_ref() {
                should_publish_canvas_storage(&current, canvas)
            } else {
                should_publish_canvas_frame(&current, &CanvasFrame::empty())
            };
            changed
                && publication_cadence.canvas_preview_due(
                    elapsed_ms,
                    canvas_receivers,
                    tracked_canvas_receivers,
                    state.preview_runtime.tracked_canvas_demand().max_fps,
                )
        };
        if publish_canvas {
            let canvas_frame = if let Some(surface) = surfaces
                .preview_surface
                .take()
                .or_else(|| surfaces.frame_surface.take())
            {
                CanvasFrame::from_surface(surface.with_frame_metadata(frame_number, elapsed_ms))
            } else if let Some(canvas) = surfaces.canvas.take() {
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
            publication_cadence.record_canvas_publication(elapsed_ms);
            state
                .preview_runtime
                .record_canvas_publication(frame_number, elapsed_ms);
            let _ = state.event_bus.canvas_sender().send(canvas_frame);
        }
    }
    state
        .preview_runtime
        .note_screen_canvas_frame(frame_number, elapsed_ms);
    if screen_canvas_receivers > 0 {
        let tracked_screen_canvas_receivers = state.preview_runtime.screen_canvas_receiver_count();
        let publish_screen = {
            let current = state.event_bus.screen_canvas_sender().borrow();
            let changed = if let Some(surface) = screen_preview_surface.as_ref() {
                should_publish_surface_frame(&current, surface)
            } else {
                should_publish_canvas_frame(&current, &CanvasFrame::empty())
            };
            changed
                && publication_cadence.screen_canvas_preview_due(
                    elapsed_ms,
                    screen_canvas_receivers,
                    tracked_screen_canvas_receivers,
                    state.preview_runtime.screen_canvas_demand().max_fps,
                )
        };
        if publish_screen {
            let screen_frame = if let Some(surface) = screen_preview_surface {
                CanvasFrame::from_surface(surface.with_frame_metadata(frame_number, elapsed_ms))
            } else {
                CanvasFrame::empty()
            };
            publication_cadence.record_screen_canvas_publication(elapsed_ms);
            state
                .preview_runtime
                .record_screen_canvas_publication(frame_number, elapsed_ms);
            let _ = state.event_bus.screen_canvas_sender().send(screen_frame);
        }
    }
    state
        .preview_runtime
        .note_web_viewport_canvas_frame(frame_number, elapsed_ms);
    let web_viewport_canvas_receivers = state.event_bus.web_viewport_canvas_receiver_count();
    if web_viewport_canvas_receivers > 0 {
        let tracked_receivers = state.preview_runtime.web_viewport_canvas_receiver_count();
        let publish_web_viewport = {
            let current = state.event_bus.web_viewport_canvas_sender().borrow();
            let changed = if let Some(canvas) = surfaces.web_viewport_preview_canvas.as_ref() {
                should_publish_canvas_storage(&current, canvas)
            } else {
                should_publish_canvas_frame(&current, &CanvasFrame::empty())
            };
            changed
                && publication_cadence.web_viewport_preview_due(
                    elapsed_ms,
                    web_viewport_canvas_receivers,
                    tracked_receivers,
                    state.preview_runtime.web_viewport_canvas_demand().max_fps,
                )
        };
        if publish_web_viewport {
            let preview_frame = if let Some(canvas) = surfaces.web_viewport_preview_canvas {
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
            publication_cadence.record_web_viewport_publication(elapsed_ms);
            state
                .preview_runtime
                .record_web_viewport_canvas_publication(frame_number, elapsed_ms);
            let _ = state
                .event_bus
                .web_viewport_canvas_sender()
                .send(preview_frame);
        }
    }
    let preview_us = micros_u32(preview_start.elapsed());
    let events_start = Instant::now();
    if event_subscribers > 0 {
        state.event_bus.publish(HypercolorEvent::FrameRendered {
            frame_number,
            timing,
        });
    }
    let events_us = micros_u32(events_start.elapsed());
    PublishFrameStats {
        elapsed_us: micros_u32(publish_start.elapsed()),
        full_frame_copy_count,
        full_frame_copy_bytes,
        frame_data_us,
        group_canvas_us,
        preview_us,
        events_us,
    }
}

fn update_published_frame(
    frame_sender: &watch::Sender<FrameData>,
    recycled_frame: &mut FrameData,
    frame_number: u32,
    elapsed_ms: u32,
    reuse_existing_frame: bool,
    refresh_existing_frame_metadata: bool,
) {
    if !reuse_existing_frame {
        frame_sender.send_modify(|published_frame| {
            std::mem::swap(published_frame, recycled_frame);
            published_frame.frame_number = frame_number;
            published_frame.timestamp_ms = elapsed_ms;
        });
        return;
    }

    if refresh_existing_frame_metadata {
        frame_sender.send_modify(|published_frame| {
            published_frame.frame_number = frame_number;
            published_frame.timestamp_ms = elapsed_ms;
        });
    }
}

fn should_publish_canvas_frame(current: &CanvasFrame, next: &CanvasFrame) -> bool {
    stable_canvas_frame_identity(current) != stable_canvas_frame_identity(next)
}

fn should_publish_surface_frame(current: &CanvasFrame, next: &PublishedSurface) -> bool {
    stable_canvas_frame_identity(current) != stable_published_surface_identity(next)
}

fn should_publish_canvas_storage(current: &CanvasFrame, next: &Canvas) -> bool {
    stable_canvas_frame_identity(current) != stable_canvas_identity(next)
}

fn authoritative_scene_surface<'a>(
    frame_surface: Option<&'a PublishedSurface>,
    preview_surface: Option<&'a PublishedSurface>,
) -> Option<&'a PublishedSurface> {
    // The GPU compositor can satisfy authoritative scene-canvas consumers
    // from the composed preview surface without forcing a CPU sampling readback.
    frame_surface.or(preview_surface)
}

fn stable_canvas_frame_identity(frame: &CanvasFrame) -> Option<StableCanvasFrameIdentity> {
    (frame.width > 0 && frame.height > 0).then(|| StableCanvasFrameIdentity {
        generation: frame.surface().generation(),
        storage: frame.surface().storage_identity(),
        width: frame.width,
        height: frame.height,
    })
}

fn stable_published_surface_identity(
    surface: &PublishedSurface,
) -> Option<StableCanvasFrameIdentity> {
    (surface.width() > 0 && surface.height() > 0).then(|| StableCanvasFrameIdentity {
        generation: surface.generation(),
        storage: surface.storage_identity(),
        width: surface.width(),
        height: surface.height(),
    })
}

fn stable_canvas_identity(canvas: &Canvas) -> Option<StableCanvasFrameIdentity> {
    (canvas.width() > 0 && canvas.height() > 0).then(|| StableCanvasFrameIdentity {
        generation: 0,
        storage: canvas.storage_identity(),
        width: canvas.width(),
        height: canvas.height(),
    })
}

fn maybe_publish_audio_level_event(
    state: &RenderThreadState,
    audio: &AudioData,
    signal: Option<&AudioSignalSnapshot>,
    elapsed_ms: u32,
    publication_cadence: &mut PublicationCadenceState,
    should_publish: bool,
) {
    if !should_publish {
        return;
    }

    publication_cadence.record_audio_level_update(elapsed_ms);
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

#[cfg(test)]
mod tests {
    use hypercolor_core::types::canvas::{Canvas, PublishedSurface};
    use tokio::sync::watch;

    use hypercolor_core::types::event::{FrameData, ZoneColors};

    use super::{FramePublicationSurfaces, authoritative_scene_surface, update_published_frame};

    fn sample_frame(
        zone_id: &str,
        color: [u8; 3],
        frame_number: u32,
        timestamp_ms: u32,
    ) -> FrameData {
        FrameData::new(
            vec![ZoneColors {
                zone_id: zone_id.to_owned(),
                colors: vec![color],
            }],
            frame_number,
            timestamp_ms,
        )
    }

    #[test]
    fn reused_frame_metadata_refresh_notifies_without_replacing_zones() {
        let (sender, mut receiver) = watch::channel(sample_frame("zone", [1, 2, 3], 1, 10));
        let mut recycled_frame = sample_frame("new-zone", [9, 9, 9], 99, 99);

        update_published_frame(&sender, &mut recycled_frame, 2, 20, true, true);

        assert!(
            receiver
                .has_changed()
                .expect("receiver should remain connected")
        );
        let frame = receiver.borrow_and_update().clone();
        assert_eq!(frame.frame_number, 2);
        assert_eq!(frame.timestamp_ms, 20);
        assert_eq!(frame.zones[0].zone_id, "zone");
        assert_eq!(frame.zones[0].colors, vec![[1, 2, 3]]);
        assert_eq!(recycled_frame.frame_number, 99);
        assert_eq!(recycled_frame.zones[0].zone_id, "new-zone");
    }

    #[test]
    fn reused_frame_without_metadata_refresh_stays_quiet() {
        let (sender, receiver) = watch::channel(sample_frame("zone", [1, 2, 3], 1, 10));
        let mut recycled_frame = sample_frame("new-zone", [9, 9, 9], 99, 99);

        update_published_frame(&sender, &mut recycled_frame, 2, 20, true, false);

        assert!(
            !receiver
                .has_changed()
                .expect("receiver should remain connected")
        );
        let frame = receiver.borrow().clone();
        assert_eq!(frame.frame_number, 1);
        assert_eq!(frame.timestamp_ms, 10);
        assert_eq!(frame.zones[0].zone_id, "zone");
    }

    #[test]
    fn authoritative_scene_surface_prefers_frame_surface() {
        let frame_surface = PublishedSurface::from_owned_canvas(Canvas::new(4, 4), 1, 16);
        let frame_surface_option = Some(frame_surface.clone());

        let selected = authoritative_scene_surface(frame_surface_option.as_ref(), None)
            .expect("frame surface should be authoritative");

        assert_eq!(
            selected.storage_identity(),
            frame_surface.storage_identity()
        );
    }

    #[test]
    fn authoritative_scene_surface_falls_back_to_preview_surface() {
        let preview_surface = PublishedSurface::from_owned_canvas(Canvas::new(4, 4), 2, 32);

        let selected = authoritative_scene_surface(None, Some(&preview_surface))
            .expect("preview surface should back authoritative consumers");

        assert_eq!(
            selected.storage_identity(),
            preview_surface.storage_identity()
        );
    }

    #[test]
    fn authoritative_scene_surface_requires_any_surface() {
        let selected = authoritative_scene_surface(None, None);

        assert!(selected.is_none());
    }

    #[test]
    fn screen_watch_surface_prefers_preview_for_passthrough_capture() {
        let preview_surface = PublishedSurface::from_owned_canvas(Canvas::new(4, 4), 1, 16);
        let frame_surface = PublishedSurface::from_owned_canvas(Canvas::new(4, 4), 2, 32);
        let capture_surface = PublishedSurface::from_owned_canvas(Canvas::new(4, 4), 3, 48);
        let surfaces = FramePublicationSurfaces {
            canvas: None,
            frame_surface: Some(frame_surface),
            preview_surface: Some(preview_surface.clone()),
            screen_capture_surface: Some(capture_surface),
            web_viewport_preview_canvas: None,
            effect_running: false,
            screen_capture_active: true,
        };

        let selected = surfaces
            .screen_watch_surface()
            .expect("preview surface should win");

        assert_eq!(
            selected.storage_identity(),
            preview_surface.storage_identity()
        );
    }

    #[test]
    fn screen_watch_surface_uses_frame_surface_for_passthrough_capture_without_preview() {
        let frame_surface = PublishedSurface::from_owned_canvas(Canvas::new(4, 4), 2, 32);
        let capture_surface = PublishedSurface::from_owned_canvas(Canvas::new(4, 4), 3, 48);
        let surfaces = FramePublicationSurfaces {
            canvas: None,
            frame_surface: Some(frame_surface.clone()),
            preview_surface: None,
            screen_capture_surface: Some(capture_surface),
            web_viewport_preview_canvas: None,
            effect_running: false,
            screen_capture_active: true,
        };

        let selected = surfaces
            .screen_watch_surface()
            .expect("frame surface should back passthrough capture");

        assert_eq!(
            selected.storage_identity(),
            frame_surface.storage_identity()
        );
    }

    #[test]
    fn screen_watch_surface_skips_frame_surface_when_effect_is_running() {
        let frame_surface = PublishedSurface::from_owned_canvas(Canvas::new(4, 4), 2, 32);
        let capture_surface = PublishedSurface::from_owned_canvas(Canvas::new(4, 4), 3, 48);
        let surfaces = FramePublicationSurfaces {
            canvas: None,
            frame_surface: Some(frame_surface),
            preview_surface: None,
            screen_capture_surface: Some(capture_surface.clone()),
            web_viewport_preview_canvas: None,
            effect_running: true,
            screen_capture_active: true,
        };

        let selected = surfaces
            .screen_watch_surface()
            .expect("capture surface should back active effects");

        assert_eq!(
            selected.storage_identity(),
            capture_surface.storage_identity()
        );
    }
}
