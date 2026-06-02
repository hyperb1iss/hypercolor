//! Kind-aware media preview + a Luminary video clip player.
//!
//! The catalog detail panel needs more than a static thumbnail: stills show
//! the full-resolution blob, GIFs animate, and video assets get a real
//! player with the clip operations you'd expect — scrub, frame-step, speed,
//! loop, and mute. Everything is client-side against the `/blob` endpoint;
//! the immutable asset itself is never mutated.
//!
//! Per Luminary §4, the player chrome is electric purple like every other
//! control surface; the per-kind category color only ever appears on identity
//! surfaces (the catalog card, the detail header, an unavailable-preview
//! placeholder), never on a button or slider.

use leptos::html;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::components::media_grid::asset_kind;
use crate::components::media_kind::{format_timecode, kind_accent, kind_icon, kind_label};
use crate::icons::*;

/// Loaded intrinsic metadata reported by the `<video>` element once it has
/// read the clip header — the daemon does not decode video, so the player is
/// the only place these are known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoMeta {
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
}

/// Picks the right preview surface for an asset's kind.
#[component]
pub fn MediaPreview(
    asset: api::MediaAssetRecord,
    on_video_loaded: Option<Callback<VideoMeta>>,
) -> impl IntoView {
    let kind = asset_kind(&asset);
    let blob_url = format!("/api/v1/assets/{}/blob", asset.id);

    match kind {
        "video" => view! {
            <VideoClipPlayer src=blob_url on_loaded=on_video_loaded />
        }
        .into_any(),
        "image" | "gif" => view! {
            <div class="overflow-hidden rounded-xl border border-edge-subtle">
                <img
                    src=blob_url
                    alt=""
                    class="aspect-video w-full bg-surface-sunken/60 object-contain"
                />
            </div>
        }
        .into_any(),
        _ => view! { <PreviewPlaceholder asset=asset /> }.into_any(),
    }
}

#[component]
fn PreviewPlaceholder(asset: api::MediaAssetRecord) -> impl IntoView {
    let kind = asset_kind(&asset);
    let accent = kind_accent(kind);
    let icon = kind_icon(kind);
    let label = kind_label(kind);

    view! {
        <div
            class="flex aspect-video w-full flex-col items-center justify-center gap-2 rounded-xl border border-edge-subtle"
            style=format!(
                "background: radial-gradient(120% 90% at 50% 20%, rgba({accent}, 0.18), rgba({accent}, 0.04) 55%, transparent 80%)"
            )
        >
            <Icon icon=icon width="40px" height="40px" style=format!("color: rgba({accent}, 0.55)") />
            <span class="text-xs font-medium text-fg-tertiary">{label}" preview unavailable"</span>
        </div>
    }
}

/// Luminary video player with real per-clip operations. Playback state is
/// sourced from the element's own events so the controls never drift out of
/// sync with what the `<video>` is actually doing.
#[component]
pub fn VideoClipPlayer(
    #[prop(into)] src: String,
    on_loaded: Option<Callback<VideoMeta>>,
) -> impl IntoView {
    let video_ref = NodeRef::<html::Video>::new();
    let playing = RwSignal::new(false);
    let current = RwSignal::new(0.0_f64);
    let duration = RwSignal::new(0.0_f64);
    let muted = RwSignal::new(true);
    let looping = RwSignal::new(true);
    let speed = RwSignal::new(1.0_f64);

    // Mirror toggle/rate signals onto the element; re-runs once the ref mounts
    // and whenever any of the three change.
    Effect::new(move |_| {
        let (is_muted, is_looping, rate) = (muted.get(), looping.get(), speed.get());
        if let Some(video) = video_ref.get() {
            video.set_muted(is_muted);
            video.set_loop(is_looping);
            video.set_playback_rate(rate);
        }
    });

    let toggle_play = move |_| {
        if let Some(video) = video_ref.get_untracked() {
            if video.paused() {
                let _ = video.play();
            } else {
                let _ = video.pause();
            }
        }
    };

    let step_frame = move |delta: f64| {
        if let Some(video) = video_ref.get_untracked() {
            let _ = video.pause();
            let target = (video.current_time() + delta).clamp(0.0, video.duration().max(0.0));
            video.set_current_time(target);
            current.set(target);
        }
    };

    let seek = move |ev| {
        let Ok(value) = event_target_value(&ev).parse::<f64>() else {
            return;
        };
        if let Some(video) = video_ref.get_untracked() {
            video.set_current_time(value);
        }
        current.set(value);
    };

    let on_loaded_meta = move |_| {
        if let Some(video) = video_ref.get_untracked() {
            let meta = VideoMeta {
                duration_secs: video.duration(),
                width: video.video_width(),
                height: video.video_height(),
            };
            duration.set(meta.duration_secs);
            if let Some(callback) = on_loaded {
                callback.run(meta);
            }
        }
    };

    let on_time = move |_| {
        if let Some(video) = video_ref.get_untracked() {
            current.set(video.current_time());
        }
    };

    let cycle_speed = move |_| {
        speed.update(|value| {
            *value = match *value {
                v if v < 0.75 => 1.0,
                v if v < 1.25 => 1.5,
                v if v < 1.75 => 2.0,
                _ => 0.5,
            };
        });
    };

    let secondary = "flex h-7 w-7 items-center justify-center rounded-md player-btn";
    let toggle_class = move |active: bool| {
        if active {
            format!("{secondary} text-accent")
        } else {
            format!("{secondary} text-fg-tertiary hover:text-fg-secondary")
        }
    };

    view! {
        <div class="space-y-2.5">
            <div class="relative overflow-hidden rounded-xl border border-edge-subtle bg-black">
                <video
                    node_ref=video_ref
                    src=src
                    class="aspect-video w-full bg-black object-contain"
                    playsinline=""
                    preload="metadata"
                    on:loadedmetadata=on_loaded_meta
                    on:timeupdate=on_time
                    on:play=move |_| playing.set(true)
                    on:pause=move |_| playing.set(false)
                    on:ended=move |_| playing.set(false)
                ></video>
            </div>

            <div class="flex items-center gap-2.5">
                <button
                    type="button"
                    class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-accent-muted/30 bg-accent/12 text-accent btn-press"
                    on:click=toggle_play
                >
                    {move || {
                        if playing.get() {
                            view! { <Icon icon=LuPause width="16px" height="16px" /> }.into_any()
                        } else {
                            view! {
                                <Icon icon=LuPlay width="16px" height="16px" style="margin-left: 1px" />
                            }
                            .into_any()
                        }
                    }}
                </button>
                <input
                    type="range"
                    min="0"
                    max=move || duration.get().max(0.001)
                    step="0.01"
                    prop:value=move || current.get()
                    class="slider-silk h-1.5 flex-1"
                    on:input=seek
                />
                <span class="shrink-0 font-mono text-[11px] tabular-nums text-fg-tertiary">
                    {move || {
                        format!("{} / {}", format_timecode(current.get()), format_timecode(duration.get()))
                    }}
                </span>
            </div>

            <div class="flex items-center justify-between gap-2">
                <div class="flex items-center gap-1">
                    <button
                        type="button"
                        class=format!("{secondary} text-fg-tertiary hover:text-fg-secondary")
                        title="Step back one frame"
                        on:click=move |_| step_frame(-1.0 / 30.0)
                    >
                        <Icon icon=LuSkipBack width="13px" height="13px" />
                    </button>
                    <button
                        type="button"
                        class=format!("{secondary} text-fg-tertiary hover:text-fg-secondary")
                        title="Step forward one frame"
                        on:click=move |_| step_frame(1.0 / 30.0)
                    >
                        <Icon icon=LuSkipForward width="13px" height="13px" />
                    </button>
                </div>
                <div class="flex items-center gap-1">
                    <button
                        type="button"
                        class="flex h-7 items-center justify-center rounded-md px-2 font-mono text-[11px] tabular-nums text-fg-tertiary player-btn hover:text-fg-secondary"
                        title="Playback speed"
                        on:click=cycle_speed
                    >
                        {move || format!("{}×", speed.get())}
                    </button>
                    <button
                        type="button"
                        class=move || toggle_class(looping.get())
                        title="Loop"
                        on:click=move |_| looping.update(|value| *value = !*value)
                    >
                        <Icon icon=LuRepeat width="14px" height="14px" />
                    </button>
                    <button
                        type="button"
                        class=move || toggle_class(!muted.get())
                        title="Mute"
                        on:click=move |_| muted.update(|value| *value = !*value)
                    >
                        {move || {
                            if muted.get() {
                                view! { <Icon icon=LuVolumeX width="14px" height="14px" /> }.into_any()
                            } else {
                                view! { <Icon icon=LuVolume2 width="14px" height="14px" /> }.into_any()
                            }
                        }}
                    </button>
                </div>
            </div>
        </div>
    }
}
