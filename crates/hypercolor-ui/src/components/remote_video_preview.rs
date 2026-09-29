//! The main canvas preview under a Remote bridge: the bridge's video track in
//! a muted, inline `<video>`, or a still with a note while no video plays.
//!
//! Each surface reports whether it is on screen and how many device pixels
//! it spans, and the page's demand reaches the bridge through
//! [`RemotePreviewContext`]. Pixel readers such as the ambient glow sample
//! the video on presented frames through `requestVideoFrameCallback`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use hypercolor_leptos_ext::canvas::create_canvas;
use js_sys::{Function, Reflect};
use leptos::html;
use leptos::prelude::*;
use leptos_use::{
    UseElementSizeReturn, use_device_pixel_ratio, use_element_size, use_element_visibility,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::app::EffectsContext;
use crate::color::{CanvasFrameAnalysis, analyze_rgba_samples};
use crate::media::{MediaUse, use_media_source};
use crate::preview_telemetry::{PreviewPresenterTelemetry, PreviewTelemetryContext};
use crate::remote_bridge::PreviewState;
use crate::remote_preview::{RemotePreviewContext, SurfaceDemand, device_pixel_extent};

/// Size of the frame drawn for an ambient sample.
const SAMPLE_WIDTH: u32 = 24;
const SAMPLE_HEIGHT: u32 = 18;
const TELEMETRY_INTERVAL_MS: f64 = 250.0;

/// The note a surface shows while no video frame is on screen, if any.
#[must_use]
pub const fn preview_note(state: PreviewState, presenting: bool) -> Option<&'static str> {
    match state {
        PreviewState::Live if presenting => None,
        PreviewState::Unsupported => Some("Live preview needs an update"),
        PreviewState::Live | PreviewState::Connecting => Some("Connecting live preview"),
    }
}

/// Main canvas preview for a bridged page.
#[component]
pub fn RemoteVideoPreview(
    context: RemotePreviewContext,
    #[prop(default = "100%".to_string())] max_width: String,
    #[prop(into, optional)] aspect_ratio: MaybeProp<String>,
    #[prop(default = "Live effect canvas preview".to_string())] aria_label: String,
    #[prop(default = false)] show_fps: bool,
    #[prop(default = "Preview".to_string())] fps_label: String,
    #[prop(default = false)] report_presenter_telemetry: bool,
) -> impl IntoView {
    let wrapper_ref = NodeRef::<html::Div>::new();
    let video_ref = NodeRef::<html::Video>::new();

    // ── Demand ─────────────────────────────────────────────────────────
    let surface = context.register_surface();
    let visible = use_element_visibility(wrapper_ref);
    let UseElementSizeReturn { width, height } = use_element_size(wrapper_ref);
    let pixel_ratio = use_device_pixel_ratio();
    Effect::new(move |_| {
        context.update_surface(
            surface,
            SurfaceDemand {
                visible: visible.get(),
                longest_edge_px: device_pixel_extent(width.get(), height.get(), pixel_ratio.get()),
            },
        );
    });
    on_cleanup(move || context.release_surface(surface));

    // ── Stream ─────────────────────────────────────────────────────────
    let state = Signal::derive(move || context.state());
    // A frame of the attached stream has reached the screen. The element
    // keeps showing its last frame when the track stalls, so this stays set
    // until another stream replaces the current one.
    let presenting = RwSignal::new(false);
    let video_size = RwSignal::new(None::<(u32, u32)>);
    let presented_fps = RwSignal::new(0.0_f32);

    Effect::new(move |_| {
        let stream = context.stream();
        let Some(video) = video_ref.get() else {
            return;
        };
        let attached = video.src_object();
        let same = match (&attached, &stream) {
            (Some(attached), Some(stream)) => js_sys::Object::is(attached, stream),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        presenting.set(false);
        video.set_muted(true);
        video.set_src_object(stream.as_ref());
        if stream.is_some() {
            play(&video);
        }
    });

    // ── Presented frames ───────────────────────────────────────────────
    let preview_telemetry = use_context::<PreviewTelemetryContext>()
        .filter(|_| report_presenter_telemetry)
        .map(|telemetry| telemetry.set_presenter);
    let measure_fps = show_fps || preview_telemetry.is_some();
    let frame_loop = StoredValue::new_local(None::<VideoFrameLoop>);
    Effect::new(move |_| {
        let Some(video) = video_ref.get() else {
            return;
        };
        if frame_loop.with_value(Option::is_some) {
            return;
        }
        let sampler = Rc::new(RefCell::new(None::<AmbientSampler>));
        let last_frame_at = Cell::new(None::<f64>);
        let last_telemetry_at = Cell::new(None::<f64>);
        let started = VideoFrameLoop::start(&video, move |now, video| {
            if !presenting.get_untracked() {
                presenting.set(true);
            }
            if measure_fps {
                let fps = smoothed_fps(
                    presented_fps.get_untracked(),
                    last_frame_at.replace(Some(now)),
                    now,
                );
                if show_fps {
                    presented_fps.set(fps);
                }
                if let Some(telemetry) = preview_telemetry
                    && last_telemetry_at
                        .get()
                        .is_none_or(|last| now - last >= TELEMETRY_INTERVAL_MS)
                {
                    last_telemetry_at.set(Some(now));
                    telemetry.set(PreviewPresenterTelemetry {
                        runtime_mode: Some("video"),
                        present_fps: (fps * 10.0).round() / 10.0,
                        ..PreviewPresenterTelemetry::default()
                    });
                }
            }
            if context.claim_analysis(now) {
                let mut sampler = sampler.borrow_mut();
                if sampler.is_none() {
                    *sampler = AmbientSampler::new();
                }
                if let Some(analysis) = sampler.as_ref().and_then(|sampler| sampler.sample(video)) {
                    context.publish_analysis(analysis);
                }
            }
        });
        frame_loop.set_value(started);
    });
    on_cleanup(move || {
        let _ = frame_loop.try_with_value(|frame_loop| {
            if let Some(frame_loop) = frame_loop {
                frame_loop.stop();
            }
        });
        if let Some(telemetry) = preview_telemetry {
            let _ = telemetry.try_set(PreviewPresenterTelemetry::default());
        }
    });

    // ── Still ──────────────────────────────────────────────────────────
    // Until the video shows a frame, the active effect's cover stands in.
    let effects = use_context::<EffectsContext>();
    let still_route = Signal::derive(move || {
        if presenting.get() {
            return None;
        }
        let effects = effects?;
        let active = effects.active_effect_id.get()?;
        effects.effects_index.with(|index| {
            index
                .iter()
                .find(|entry| entry.effect.id == active)
                .and_then(|entry| entry.effect.cover_image_url.clone())
        })
    });
    let still = use_media_source(still_route, MediaUse::Artwork);

    // ── Layout ─────────────────────────────────────────────────────────
    let resolved_aspect_ratio = Memo::new(move |_| {
        aspect_ratio.get().unwrap_or_else(|| {
            video_size
                .get()
                .map(|(width, height)| format!("{width} / {height}"))
                .unwrap_or_else(|| {
                    crate::render_canvas::aspect_ratio_css(
                        crate::render_canvas::DEFAULT_RENDER_CANVAS,
                    )
                })
        })
    });
    let wrapper_style = move || {
        format!(
            "max-width: {max_width}; width: 100%; max-height: 100%; aspect-ratio: {};",
            resolved_aspect_ratio.get()
        )
    };
    let record_size = move || {
        if let Some(video) = video_ref.get_untracked() {
            let size = (video.video_width(), video.video_height());
            let next = (size.0 > 0 && size.1 > 0).then_some(size);
            if video_size.get_untracked() != next {
                video_size.set(next);
            }
        }
    };
    let live = move || state.get() == PreviewState::Live && presenting.get();

    view! {
        <div
            node_ref=wrapper_ref
            class="relative overflow-hidden bg-black"
            style=wrapper_style
            data-preview-runtime=move || if live() { "video" } else { "still" }
            data-remote-preview-state=move || state.get().as_str()
        >
            {move || still.get().url().map(|url| view! {
                <img
                    class="absolute inset-0 h-full w-full object-cover"
                    src=url
                    alt=""
                    decoding="async"
                    draggable="false"
                />
            })}
            <video
                node_ref=video_ref
                class="absolute inset-0 block h-full w-full object-fill transition-opacity duration-300"
                class:opacity-0=move || !presenting.get()
                role="img"
                aria-label=aria_label
                autoplay=""
                muted=""
                playsinline=""
                disablepictureinpicture=""
                disableremoteplayback=""
                prop:muted=true
                on:loadedmetadata=move |_| record_size()
                on:resize=move |_| record_size()
                on:loadeddata=move |_| presenting.set(true)
            ></video>
            {move || preview_note(state.get(), presenting.get()).map(|note| view! {
                <div class="pointer-events-none absolute bottom-2 left-1/2 z-10 max-w-[90%] -translate-x-1/2 \
                            truncate rounded-full bg-black/60 px-2 py-0.5 font-mono text-[10px] \
                            uppercase tracking-wider text-fg-tertiary backdrop-blur-sm">
                    {note}
                </div>
            })}
            {show_fps.then(|| view! {
                <div class="absolute top-2 right-2 rounded bg-black/70 px-2 py-0.5 font-mono text-[10px] \
                            text-fg-tertiary backdrop-blur-sm transition-opacity duration-300 animate-enter-fade">
                    {move || format!("{fps_label} {:.0} fps [video]", presented_fps.get())}
                </div>
            })}
        </div>
    }
}

fn play(video: &web_sys::HtmlVideoElement) {
    if let Ok(promise) = video.play() {
        // Browsers let a muted inline video play without a gesture, and the
        // `autoplay` attribute retries on its own, so a rejection here (an
        // interrupted load, say) needs no handling beyond settling.
        wasm_bindgen_futures::spawn_local(async move {
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        });
    }
}

fn smoothed_fps(previous_fps: f32, previous_frame_at: Option<f64>, now: f64) -> f32 {
    let Some(previous_frame_at) = previous_frame_at else {
        return previous_fps;
    };
    let elapsed = now - previous_frame_at;
    if elapsed <= 0.0 {
        return previous_fps;
    }
    let instant = (1000.0 / elapsed).clamp(0.0, 120.0);
    let previous = f64::from(previous_fps);
    let next = if previous <= 0.0 {
        instant
    } else {
        previous * 0.82 + instant * 0.18
    };
    #[allow(
        clippy::cast_possible_truncation,
        reason = "presented fps is clamped to 120"
    )]
    let next = next as f32;
    next
}

/// Draws presented video frames into a small canvas to read their colors.
struct AmbientSampler {
    context: web_sys::CanvasRenderingContext2d,
}

impl AmbientSampler {
    fn new() -> Option<Self> {
        let canvas = create_canvas().ok()?;
        canvas.set_width(SAMPLE_WIDTH);
        canvas.set_height(SAMPLE_HEIGHT);
        let options = js_sys::Object::new();
        Reflect::set(
            &options,
            &JsValue::from_str("willReadFrequently"),
            &JsValue::TRUE,
        )
        .ok()?;
        let context = canvas
            .get_context_with_context_options("2d", &options)
            .ok()
            .flatten()?
            .dyn_into::<web_sys::CanvasRenderingContext2d>()
            .ok()?;
        Some(Self { context })
    }

    fn sample(&self, video: &web_sys::HtmlVideoElement) -> Option<CanvasFrameAnalysis> {
        self.context
            .draw_image_with_html_video_element_and_dw_and_dh(
                video,
                0.0,
                0.0,
                f64::from(SAMPLE_WIDTH),
                f64::from(SAMPLE_HEIGHT),
            )
            .ok()?;
        let pixels = self
            .context
            .get_image_data(0.0, 0.0, f64::from(SAMPLE_WIDTH), f64::from(SAMPLE_HEIGHT))
            .ok()?
            .data();
        analyze_rgba_samples(
            pixels
                .chunks_exact(4)
                .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]]),
        )
    }
}

type FrameCallback = Closure<dyn FnMut(f64, JsValue)>;

/// A `requestVideoFrameCallback` loop that runs on every presented frame
/// until stopped. Browsers without the API never start one.
struct VideoFrameLoop {
    video: web_sys::HtmlVideoElement,
    callback: Rc<RefCell<Option<FrameCallback>>>,
    handle: Rc<Cell<Option<f64>>>,
    stopped: Rc<Cell<bool>>,
}

impl VideoFrameLoop {
    fn start(
        video: &web_sys::HtmlVideoElement,
        mut on_frame: impl FnMut(f64, &web_sys::HtmlVideoElement) + 'static,
    ) -> Option<Self> {
        let request = Reflect::get(video, &JsValue::from_str("requestVideoFrameCallback"))
            .ok()?
            .dyn_into::<Function>()
            .ok()?;
        let callback = Rc::new(RefCell::new(None::<FrameCallback>));
        let handle = Rc::new(Cell::new(None::<f64>));
        let stopped = Rc::new(Cell::new(false));
        let closure = Closure::<dyn FnMut(f64, JsValue)>::new({
            let video = video.clone();
            let request = request.clone();
            let callback = Rc::clone(&callback);
            let handle = Rc::clone(&handle);
            let stopped = Rc::clone(&stopped);
            move |now: f64, _metadata: JsValue| {
                if stopped.get() {
                    return;
                }
                on_frame(now, &video);
                if let Some(callback) = callback.borrow().as_ref() {
                    handle.set(
                        request
                            .call1(&video, callback.as_ref())
                            .ok()
                            .and_then(|handle| handle.as_f64()),
                    );
                }
            }
        });
        handle.set(request.call1(video, closure.as_ref()).ok()?.as_f64());
        callback.replace(Some(closure));
        Some(Self {
            video: video.clone(),
            callback,
            handle,
            stopped,
        })
    }

    fn stop(&self) {
        self.stopped.set(true);
        let cancelled = self.handle.take().is_none_or(|handle| {
            Reflect::get(&self.video, &JsValue::from_str("cancelVideoFrameCallback"))
                .ok()
                .and_then(|cancel| cancel.dyn_into::<Function>().ok())
                .is_some_and(|cancel| {
                    cancel
                        .call1(&self.video, &JsValue::from_f64(handle))
                        .is_ok()
                })
        });
        if let Some(callback) = self.callback.replace(None)
            && !cancelled
        {
            // A request still pending would call a dropped closure; leave
            // this one alive, since `stopped` already makes it a no-op.
            callback.forget();
        }
    }
}
