//! The phone now-playing strip and its sheet.
//!
//! Below `md` the sidebar is gone, and with it the Now Playing panel,
//! global brightness, and a scene switcher every page can reach. The strip
//! docks above the bottom tab bar on every page, like a mini player: what
//! is playing, play or pause, and next, all in thumb reach. Tapping it
//! opens a sheet with the full transport, global brightness, the zones of
//! a multi-zone scene, and the scene list.
//!
//! Both read the shared [`NowPlaying`] model, so they steer exactly what
//! the desktop sidebar steers.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::components::A;

use crate::app::EffectsContext;
use crate::components::bottom_sheet::BottomSheet;
use crate::components::canvas_preview::CanvasPreview;
use crate::components::now_playing::{
    CaptionInputs, NowPlaying, now_playing_caption, use_now_playing,
};
use crate::components::scene_picker::{SceneRow, scene_rows};
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::sidebar::SidebarAudioToggle;
use crate::components::zone_now_playing::set_zone_enabled;
use crate::components::zone_tabs::{PAUSED_DIM, ZoneDot};
use crate::icons::*;
use crate::route_ui::NowPlayingCanvasMode;
use crate::zones::{ScenesContext, ZoneEffectState, ZonesContext};
use hypercolor_leptos_ext::events::Input;

/// The strip's palette swatch: the `--np-*` triplets as one gradient,
/// standing in for the live thumbnail on pages that show their own preview.
const SWATCH_STYLE: &str = "background: linear-gradient(135deg, rgb(var(--np-primary)) 0%, \
                            rgb(var(--np-secondary)) 55%, rgb(var(--np-tertiary)) 100%)";

/// The ambient tint shared by the strip and the sheet's heading.
const AMBIENT_GLOW: &str = "inset 3px 0 0 rgb(var(--np-primary)), \
                            inset 4px 0 14px rgba(var(--np-primary), 0.14), \
                            0 0 24px rgba(var(--np-primary), 0.08)";

/// The strip and, while open, its sheet. Mount it only on phones.
#[component]
pub fn PhoneNowPlaying() -> impl IntoView {
    let now_playing = use_now_playing();
    let (sheet_open, set_sheet_open) = signal(false);

    view! {
        <div
            class="phone-now-playing pointer-events-none fixed inset-x-0 z-40 px-2"
            style:--np-primary=move || now_playing.primary_rgb.get()
            style:--np-secondary=move || now_playing.secondary_rgb.get()
            style:--np-tertiary=move || now_playing.tertiary_rgb.get()
        >
            <NowPlayingStrip
                sheet_open=sheet_open
                on_open=Callback::new(move |()| set_sheet_open.set(true))
            />
        </div>
        <Show when=move || sheet_open.get()>
            <NowPlayingSheet on_close=Callback::new(move |()| set_sheet_open.set(false)) />
        </Show>
    }
}

/// The docked mini player.
#[component]
fn NowPlayingStrip(sheet_open: ReadSignal<bool>, on_open: Callback<()>) -> impl IntoView {
    let now_playing = use_now_playing();
    let fx = expect_context::<EffectsContext>();
    let zones_ctx = expect_context::<ZonesContext>();
    let caption = Memo::new(move |_| {
        let multi_zone = zones_ctx.multi_zone.get();
        let zone_count = zones_ctx.led_zones.with(Vec::len);
        let name = fx.active_effect_name.get();
        let category = fx.active_effect_category.get();
        now_playing.shown_zone_effect.with(|shown| {
            now_playing_caption(CaptionInputs {
                has_active: now_playing.has_active.get(),
                paused: now_playing.output_paused.get(),
                multi_zone,
                zone_count,
                shown: shown.as_ref(),
                effect_name: name.as_deref(),
                category: &category,
            })
        })
    });

    view! {
        <div
            class="pointer-events-auto flex h-[var(--phone-strip-h)] items-center gap-1 \
                   rounded-xl border border-edge-subtle glass-dense pr-1"
            style:box-shadow=AMBIENT_GLOW
        >
            <button
                type="button"
                class="flex h-full min-w-0 flex-1 items-center gap-3 rounded-xl pl-2.5 text-left \
                       active:bg-surface-hover/30"
                aria-haspopup="dialog"
                aria-expanded=move || sheet_open.get().to_string()
                aria-label=move || {
                    let caption = caption.get();
                    format!("Now playing: {}. Open player", caption.title)
                }
                on:click=move |_| on_open.run(())
            >
                <StripThumbnail sheet_open=sheet_open />
                <span class="min-w-0 flex-1">
                    <span class="block truncate text-[13px] font-medium leading-tight text-fg-primary">
                        {move || caption.get().title}
                    </span>
                    <span class="mt-0.5 block truncate text-[11px] leading-tight text-fg-tertiary">
                        {move || caption.get().detail}
                    </span>
                </span>
            </button>
            {move || now_playing.has_active.get().then(|| view! {
                <PlayPauseButton large=false />
                <button
                    type="button"
                    class="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg \
                           text-fg-secondary btn-press"
                    aria-label="Next effect"
                    on:click=move |_| now_playing.step(1)
                >
                    <Icon icon=LuSkipForward width="18px" height="18px" />
                </button>
            })}
        </div>
    }
}

/// The strip's leading art: the live canvas on pages without their own
/// preview, the palette swatch elsewhere, a quiet glyph when idle. The
/// live canvas yields while the sheet is open; the sheet shows its own.
#[component]
fn StripThumbnail(sheet_open: ReadSignal<bool>) -> impl IntoView {
    let now_playing = use_now_playing();
    let NowPlaying {
        has_active,
        canvas_mode,
        canvas_frame,
        preview_fps,
        preview_target_fps,
        ..
    } = now_playing;
    let live = Memo::new(move |_| {
        has_active.get() && canvas_mode.get() == NowPlayingCanvasMode::Preview && !sheet_open.get()
    });

    view! {
        <span class="relative flex h-10 w-10 shrink-0 items-center justify-center overflow-hidden \
                     rounded-lg bg-black/40">
            {move || {
                if live.get() {
                    view! {
                        <span class="w-full">
                            <CanvasPreview
                                frame=canvas_frame
                                fps=preview_fps
                                fps_target=preview_target_fps
                                max_width="100%".to_string()
                                main_canvas=true
                                aria_label="Live effect thumbnail".to_string()
                            />
                        </span>
                    }
                    .into_any()
                } else if has_active.get() {
                    view! { <span class="absolute inset-0" style=SWATCH_STYLE /> }.into_any()
                } else {
                    view! {
                        <span class="text-fg-tertiary">
                            <Icon icon=LuLayers width="16px" height="16px" />
                        </span>
                    }
                    .into_any()
                }
            }}
        </span>
    }
}

/// Pause or resume all output.
#[component]
fn PlayPauseButton(large: bool) -> impl IntoView {
    let now_playing = use_now_playing();
    let paused = now_playing.output_paused;
    let class = if large {
        "flex h-14 w-14 shrink-0 items-center justify-center rounded-full bg-accent \
         text-white btn-press accent-purple edge-glow-accent"
    } else {
        "flex h-11 w-11 shrink-0 items-center justify-center rounded-lg text-fg-primary btn-press"
    };
    let glyph = if large { "22px" } else { "18px" };

    view! {
        <button
            type="button"
            class=class
            aria-label=move || if paused.get() { "Resume all output" } else { "Pause all output" }
            on:click=move |_| now_playing.toggle_output()
        >
            {move || if paused.get() {
                view! { <Icon icon=LuPlay width=glyph height=glyph /> }.into_any()
            } else {
                view! { <Icon icon=LuPause width=glyph height=glyph /> }.into_any()
            }}
        </button>
    }
}

/// The full player: preview, transport, brightness, zones, and scenes.
#[component]
fn NowPlayingSheet(on_close: Callback<()>) -> impl IntoView {
    let now_playing = use_now_playing();
    let fx = expect_context::<EffectsContext>();
    let zones_ctx = expect_context::<ZonesContext>();
    let NowPlaying {
        has_active,
        canvas_frame,
        preview_fps,
        preview_target_fps,
        global_brightness,
        ..
    } = now_playing;
    let caption = Memo::new(move |_| {
        let multi_zone = zones_ctx.multi_zone.get();
        let zone_count = zones_ctx.led_zones.with(Vec::len);
        let name = fx.active_effect_name.get();
        let category = fx.active_effect_category.get();
        now_playing.shown_zone_effect.with(|shown| {
            now_playing_caption(CaptionInputs {
                has_active: has_active.get(),
                paused: now_playing.output_paused.get(),
                multi_zone,
                zone_count,
                shown: shown.as_ref(),
                effect_name: name.as_deref(),
                category: &category,
            })
        })
    });

    view! {
        <BottomSheet on_close=on_close title="Now playing" hide_title=true>
            <div
                class="space-y-5"
                style:--np-primary=move || now_playing.primary_rgb.get()
                style:--np-secondary=move || now_playing.secondary_rgb.get()
                style:--np-tertiary=move || now_playing.tertiary_rgb.get()
            >
                // Live preview and what it is.
                {move || has_active.get().then(|| view! {
                    <div
                        class="overflow-hidden rounded-lg bg-black/40"
                        style:box-shadow="0 4px 24px rgba(var(--np-primary), 0.22), \
                                          0 0 40px rgba(var(--np-secondary), 0.08)"
                    >
                        <CanvasPreview
                            frame=canvas_frame
                            fps=preview_fps
                            fps_target=preview_target_fps
                            max_width="100%".to_string()
                            main_canvas=true
                        />
                    </div>
                })}
                <div class="flex items-center gap-2">
                    <div class="min-w-0 flex-1">
                        <div class="truncate text-base font-semibold leading-tight text-fg-primary">
                            {move || caption.get().title}
                        </div>
                        <div class="mt-1 truncate text-xs text-fg-secondary">
                            {move || caption.get().detail}
                        </div>
                    </div>
                    <SidebarAudioToggle touch=true />
                </div>

                // Transport.
                {move || has_active.get().then(|| view! {
                    <div class="flex items-center justify-between px-1">
                        <button
                            type="button"
                            class="flex h-11 w-11 items-center justify-center rounded-lg text-fg-secondary btn-press"
                            aria-label="Random effect"
                            on:click=move |_| now_playing.shuffle()
                        >
                            <Icon icon=LuShuffle width="18px" height="18px" />
                        </button>
                        <button
                            type="button"
                            class="flex h-11 w-11 items-center justify-center rounded-lg text-fg-primary btn-press"
                            aria-label="Previous effect"
                            on:click=move |_| now_playing.step(-1)
                        >
                            <Icon icon=LuSkipBack width="22px" height="22px" />
                        </button>
                        <PlayPauseButton large=true />
                        <button
                            type="button"
                            class="flex h-11 w-11 items-center justify-center rounded-lg text-fg-primary btn-press"
                            aria-label="Next effect"
                            on:click=move |_| now_playing.step(1)
                        >
                            <Icon icon=LuSkipForward width="22px" height="22px" />
                        </button>
                        <A
                            href=crate::route_ui::route_href("/effects")
                            attr:class="flex h-11 w-11 items-center justify-center rounded-lg text-fg-secondary btn-press"
                            attr:aria-label="Effect controls"
                            on:click=move |_| on_close.run(())
                        >
                            <Icon icon=LuSlidersHorizontal width="18px" height="18px" />
                        </A>
                    </div>
                })}

                // Global brightness.
                <div class="flex items-center gap-3">
                    <span class="flex shrink-0 text-fg-tertiary">
                        <Icon icon=LuSun width="18px" height="18px" />
                    </span>
                    <input
                        type="range"
                        min="0"
                        max="100"
                        step="1"
                        aria-label="Global brightness"
                        class="min-w-0 flex-1 cursor-pointer"
                        style:accent-color="rgb(var(--np-primary))"
                        prop:value=move || global_brightness.get().to_string()
                        on:input=move |ev| {
                            if let Some(brightness) = Input::from_event(ev).value::<u8>() {
                                now_playing.set_brightness(brightness);
                            }
                        }
                    />
                    <span class="w-11 shrink-0 text-right text-sm font-medium tabular-nums text-fg-primary">
                        {move || format!("{}%", global_brightness.get())}
                    </span>
                </div>

                {move || zones_ctx.multi_zone.get().then(|| view! { <SheetZones /> })}
                <SheetScenes />
            </div>
        </BottomSheet>
    }
}

/// Every LED zone with what it shows and a pause toggle.
#[component]
fn SheetZones() -> impl IntoView {
    let fx = expect_context::<EffectsContext>();

    view! {
        <section aria-label="Zones">
            <h3 class=format!("{} mb-2", label_class(LabelSize::Small, LabelTone::Default))>"Zones"</h3>
            <div class="divide-y divide-edge-subtle/40 rounded-lg border border-edge-subtle/60 bg-surface-sunken/40">
                <For
                    each=move || fx.zone_effects.get()
                    key=|state| (state.zone.id.clone(), state.zone.enabled, state.display_label())
                    children=move |state| view! { <SheetZoneRow state=state /> }
                />
            </div>
        </section>
    }
}

#[component]
fn SheetZoneRow(state: ZoneEffectState) -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let zone_id = state.zone.id.clone();
    let enabled = state.zone.enabled;
    let label = state
        .display_label()
        .unwrap_or_else(|| "No effect".to_owned());
    let action = if enabled {
        format!("Pause {}", state.zone.name)
    } else {
        format!("Resume {}", state.zone.name)
    };

    view! {
        <div class="flex min-h-13 items-center gap-3 pl-3 pr-1" class=(PAUSED_DIM, !enabled)>
            <span class="flex shrink-0 text-sm">
                <ZoneDot color=Signal::stored(state.zone.color.clone()) />
            </span>
            <div class="min-w-0 flex-1 py-2">
                <div class="truncate text-sm font-medium text-fg-primary">{state.zone.name.clone()}</div>
                <div class="truncate text-xs text-fg-tertiary">{label}</div>
            </div>
            <button
                type="button"
                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg text-fg-secondary btn-press"
                aria-label=action
                on:click=move |_| set_zone_enabled(zones_ctx, zone_id.clone(), !enabled)
            >
                {if enabled {
                    view! { <Icon icon=LuPause width="16px" height="16px" /> }.into_any()
                } else {
                    view! { <Icon icon=LuPlay width="16px" height="16px" /> }.into_any()
                }}
            </button>
        </div>
    }
}

/// Default and every saved scene; a tap switches. Creating, renaming, and
/// deleting scenes stay in the page header's scene picker.
#[component]
fn SheetScenes() -> impl IntoView {
    let scenes_ctx = expect_context::<ScenesContext>();
    let switching = scenes_ctx.switching;
    let rows = Memo::new(move |_| {
        scenes_ctx.scenes.with(|scenes| {
            scenes_ctx
                .active
                .with(|active| scene_rows(scenes, active.as_ref()))
        })
    });
    let choose = move |row: SceneRow| {
        if row.active || switching.get_untracked().is_some() {
            return;
        }
        match row.id {
            Some(id) => scenes_ctx.activate.run(id),
            None => scenes_ctx.deactivate.run(()),
        }
    };

    view! {
        <section aria-label="Scenes">
            <h3 class=format!("{} mb-2", label_class(LabelSize::Small, LabelTone::Default))>"Scene"</h3>
            <div
                class="divide-y divide-edge-subtle/40 rounded-lg border border-edge-subtle/60 bg-surface-sunken/40"
                role="radiogroup"
                aria-label="Scene"
            >
                {move || rows.get().into_iter().map(|row| {
                    // Deactivation reports `switching == Some("")`, which the
                    // Default row's empty token matches.
                    let token = row.id.clone().unwrap_or_default();
                    let is_switching = Memo::new(move |_| switching.get().as_deref() == Some(token.as_str()));
                    let active = row.active;
                    let locked = row.locked;
                    let label = row.label.clone();
                    view! {
                        <button
                            type="button"
                            role="radio"
                            aria-checked=active.to_string()
                            class="flex min-h-12 w-full items-center gap-3 px-3 text-left text-sm \
                                   active:bg-surface-hover/30 disabled:opacity-50"
                            class=("text-fg-primary", active)
                            class=("text-fg-secondary", !active)
                            disabled=move || switching.get().is_some() && !is_switching.get()
                            on:click=move |_| choose(row.clone())
                        >
                            <span class="flex h-4 w-4 shrink-0 items-center justify-center">
                                {move || if is_switching.get() {
                                    Some(view! {
                                        <span class="flex animate-spin text-fg-tertiary">
                                            <Icon icon=LuLoader width="14px" height="14px" />
                                            <span class="sr-only">"Switching"</span>
                                        </span>
                                    }.into_any())
                                } else if active {
                                    Some(view! {
                                        <span class="flex text-accent">
                                            <Icon icon=LuCheck width="14px" height="14px" />
                                        </span>
                                    }.into_any())
                                } else {
                                    None
                                }}
                            </span>
                            <span class="min-w-0 flex-1 truncate">{label}</span>
                            {locked.then(|| view! {
                                <span class="flex shrink-0 text-status-warning" title="Snapshot-locked scene">
                                    <Icon icon=LuLock width="13px" height="13px" />
                                    <span class="sr-only">"Snapshot-locked"</span>
                                </span>
                            })}
                        </button>
                    }
                }).collect_view()}
            </div>
        </section>
    }
}
