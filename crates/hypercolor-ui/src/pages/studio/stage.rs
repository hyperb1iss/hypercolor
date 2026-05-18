//! The Studio Stage — the center workspace for the selected surface.
//!
//! The Stage has two views. **Output** is the live preview: a Light shows
//! the composited LED canvas via `CanvasPreview`, a Screen shows that
//! device's face via `DisplayPreviewSurface`. **Layout** embeds the
//! spatial device-placement editor lifted from the retired `/layout`
//! page. The Output/Layout toggle is hidden for Screens — a single LCD
//! has no spatial placement. Wave 10 adds per-zone preview frames.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_types::scene::{RenderGroupId, UnassignedBehavior};

use crate::api;
use crate::api::zones::ZoneOutcome;
use crate::app::{CapabilitiesContext, DisplaysContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::components::display_preview_surface::DisplayPreviewSurface;
use crate::components::layout_builder::LayoutBuilder;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::silk_select::SilkSelect;
use crate::toasts;
use crate::display_preview_state::use_display_preview_subscription;
use crate::display_utils::display_preview_shell_url;
use crate::icons::*;
use crate::ws::CanvasFrame;
use crate::ws::messages::group_has_degraded_layer;

use super::StudioContext;
use super::stage_view::{StageView, resolve_stage_view};
use super::surface::{SurfaceKind, UNASSIGNED_SURFACE_ID, surfaces_from_groups};
use super::surface_rail::unassigned_behavior_label;

/// Preview FPS ceiling while the Layout editor is on the Stage, matching
/// the retired `/layout` page so spatial editing stays smooth.
const LAYOUT_PREVIEW_FPS_CAP: u32 = 60;

/// The center Stage. Dispatches on the current selection: a real surface
/// renders its Output/Layout views, the synthetic Unassigned entry (§9.4)
/// renders the unassigned-lights panel instead.
#[component]
pub fn Stage() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let is_unassigned = Memo::new(move |_| {
        studio.selected_surface_id.get().as_deref() == Some(UNASSIGNED_SURFACE_ID)
    });
    view! {
        {move || {
            if is_unassigned.get() {
                view! { <UnassignedStage /> }.into_any()
            } else {
                view! { <SurfaceStage /> }.into_any()
            }
        }}
    }
}

/// The Stage for a real surface. Reads the selected surface from
/// [`StudioContext`] and the live preview streams from [`WsContext`].
#[component]
fn SurfaceStage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let studio = expect_context::<StudioContext>();
    let displays = expect_context::<DisplaysContext>().displays_resource;

    let selected_surface = Memo::new(move |_| {
        let id = studio.selected_surface_id.get()?;
        let scene = studio.active_scene.get()?;
        surfaces_from_groups(&scene.groups)
            .into_iter()
            .find(|surface| surface.id == id)
    });

    let surface_name = Memo::new(move |_| selected_surface.get().map(|surface| surface.name));
    let is_screen =
        Memo::new(move |_| selected_surface.get().map(|s| s.kind) == Some(SurfaceKind::Screen));

    // The selected surface flags itself when its layer stack has a failed or
    // asset-missing layer — the §6.7 Stage-side degraded indicator.
    let surface_degraded = Memo::new(move |_| {
        let (Some(surface), Some(scene)) = (selected_surface.get(), studio.active_scene.get())
        else {
            return false;
        };
        ws.layer_health.with(|map| {
            group_has_degraded_layer(map, &scene.id, &surface.id, &surface.layer_ids)
        })
    });

    // The toggle latches the last requested view; `resolved_view` applies
    // the §6.3 rule that a Screen has no Layout view.
    let requested_view = RwSignal::new(StageView::default());
    let resolved_view =
        Memo::new(move |_| resolve_stage_view(requested_view.get(), is_screen.get()));

    // The Layout editor wants the same preview headroom the `/layout`
    // page reserved; Output falls back to the shared default.
    Effect::new(move |_| {
        let cap = if resolved_view.get() == StageView::Layout {
            LAYOUT_PREVIEW_FPS_CAP
        } else {
            crate::ws::DEFAULT_PREVIEW_FPS_CAP
        };
        ws.set_preview_cap.set(cap);
        ws.set_preview_width_cap.set(0);
    });
    on_cleanup(move || {
        ws.set_preview_cap.set(crate::ws::DEFAULT_PREVIEW_FPS_CAP);
        ws.set_preview_width_cap.set(0);
    });

    // A Screen surface drives the per-display face-preview stream; a Light
    // leaves the target `None`, which unsubscribes. The subscription
    // retargets reactively and clears on unmount.
    let display_device = Signal::derive(move || {
        selected_surface
            .get()
            .filter(|surface| surface.kind == SurfaceKind::Screen)
            .and_then(|surface| surface.display_device_id)
    });
    use_display_preview_subscription(ws, display_device);

    // The selected screen's device record — its dimensions and shape size
    // the preview frame.
    let selected_display = Memo::new(move |_| {
        let device_id = display_device.get()?;
        let snapshot = displays.get();
        let items = snapshot.as_ref()?.as_ref().ok()?;
        items
            .iter()
            .find(|display| display.id == device_id)
            .cloned()
    });

    let screen_frame = RwSignal::new(None::<CanvasFrame>);
    Effect::new(move |_| {
        display_device.track();
        screen_frame.set(None);
    });
    Effect::new(move |_| {
        let frame = ws.display_preview_frame.get();
        // The channel carries no device id, so accept a frame only when
        // its resolution matches the selected screen. That rejects an
        // in-flight frame from the previously selected screen; two
        // identically sized screens still need daemon-side frame tagging
        // to be fully distinguishable.
        let belongs_to_target = match (&frame, selected_display.get()) {
            (Some(frame), Some(display)) => {
                frame.width == display.width && frame.height == display.height
            }
            (None, _) => true,
            (Some(_), None) => false,
        };
        if belongs_to_target {
            screen_frame.set(frame);
        }
    });

    // The LED Output frame: in a multi-zone scene the selected zone has
    // its own composited preview (§9.5), keyed by zone id; until the first
    // per-zone frame arrives — or in a single-zone scene where the zone
    // canvas *is* the whole canvas — it falls back to the composited
    // scene canvas.
    let led_output_frame = Signal::derive(move || {
        if let Some(surface) = selected_surface.get()
            && surface.kind == SurfaceKind::Light
            && let Some(frame) = ws
                .zone_preview_frames
                .with(|frames| frames.get(&surface.id).cloned())
        {
            return Some(frame);
        }
        ws.canvas_frame.get()
    });

    // The display-preview stream carries no FPS, so the Screen caption is
    // resolution only; the LED canvas reports both.
    let caption = Memo::new(move |_| {
        if is_screen.get() {
            selected_display
                .get()
                .map(|display| format!("{}×{}", display.width, display.height))
                .unwrap_or_else(|| "—".to_owned())
        } else {
            let resolution = ws
                .canvas_frame
                .get()
                .map(|frame| format!("{}×{}", frame.width, frame.height))
                .unwrap_or_else(|| "—".to_owned());
            format!("{resolution} · {:.0} fps", ws.preview_fps.get())
        }
    });

    view! {
        <div class="flex h-full flex-col bg-surface-sunken/20">
            <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-5 py-3">
                <div class="flex items-baseline gap-2">
                    <span class=label_class(LabelSize::Small, LabelTone::Default)>"Stage"</span>
                    <span class="text-sm font-semibold text-fg-primary">
                        {move || surface_name.get().unwrap_or_else(|| "No surface".to_owned())}
                    </span>
                </div>
                {move || {
                    if is_screen.get() {
                        view! {
                            <div class="flex items-center gap-2">
                                <span class=label_class(
                                    LabelSize::Micro,
                                    LabelTone::Default,
                                )>"Output"</span>
                                {move || {
                                    selected_display
                                        .get()
                                        .map(|display| {
                                            view! {
                                                <a
                                                    href=display_preview_shell_url(&display.id)
                                                    target="_blank"
                                                    rel="noopener"
                                                    class="rounded-md p-1 text-fg-tertiary transition-colors hover:text-fg-primary"
                                                    title="Open full-screen preview"
                                                >
                                                    <Icon
                                                        icon=LuExternalLink
                                                        width="12px"
                                                        height="12px"
                                                    />
                                                </a>
                                            }
                                        })
                                }}
                            </div>
                        }
                            .into_any()
                    } else {
                        view! { <StageViewToggle requested=requested_view /> }.into_any()
                    }
                }}
            </div>

            {move || match resolved_view.get() {
                StageView::Layout => {
                    view! {
                        <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                            <LayoutBuilder />
                        </div>
                    }
                        .into_any()
                }
                StageView::Output => {
                    view! {
                        <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                            {move || {
                                surface_degraded.get().then(|| view! { <DegradedBanner /> })
                            }}
                            <div class="flex flex-1 items-center justify-center overflow-hidden p-6">
                                <div class="flex max-w-full flex-col items-center gap-3">
                                    {move || {
                                        if is_screen.get() {
                                            let Some(display) = selected_display.get() else {
                                                return view! {
                                                    <div class="flex h-64 w-64 items-center justify-center rounded-xl border border-dashed border-edge-subtle/45 text-[11px] text-fg-tertiary/55">
                                                        "Preparing screen preview…"
                                                    </div>
                                                }
                                                    .into_any();
                                            };
                                            let aspect = format!(
                                                "{} / {}",
                                                display.width.max(1),
                                                display.height.max(1),
                                            );
                                            let shape = if display.circular {
                                                "rounded-full"
                                            } else {
                                                "rounded-xl"
                                            };
                                            let container_class = format!(
                                                "w-full max-w-[520px] overflow-hidden border \
                                                 border-edge-subtle/70 bg-black edge-glow-accent \
                                                 {shape}",
                                            );
                                            view! {
                                                <DisplayPreviewSurface
                                                    frame=screen_frame
                                                    fallback_src=api::display_preview_url(
                                                        &display.id,
                                                        None,
                                                    )
                                                    aspect_ratio=aspect
                                                    aria_label=format!(
                                                        "Studio stage preview of {}",
                                                        display.name,
                                                    )
                                                    container_class=container_class
                                                />
                                            }
                                                .into_any()
                                        } else {
                                            view! {
                                                <div
                                                    class="overflow-hidden rounded-xl border border-edge-subtle/70 bg-black/45"
                                                    style="box-shadow: 0 0 44px rgba(225, 53, 255, 0.09)"
                                                >
                                                    <CanvasPreview
                                                        frame=led_output_frame
                                                        fps=ws.preview_fps
                                                        fps_target=ws.preview_target_fps
                                                        max_width="min(640px, 100%)".to_string()
                                                        aria_label="Studio stage live output"
                                                            .to_string()
                                                    />
                                                </div>
                                            }
                                                .into_any()
                                        }
                                    }}
                                    <div class="font-mono text-[11px] tabular-nums text-fg-tertiary/70">
                                        {move || caption.get()}
                                    </div>
                                </div>
                            </div>
                        </div>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// The Output/Layout segmented toggle in the Stage header. Shown only for
/// Light surfaces; a Screen has no Layout view.
#[component]
fn StageViewToggle(requested: RwSignal<StageView>) -> impl IntoView {
    view! {
        <div class="flex items-center gap-0.5 rounded-lg border border-edge-subtle/60 bg-surface-sunken/40 p-0.5">
            <StageTab label="Output" value=StageView::Output requested=requested />
            <StageTab label="Layout" value=StageView::Layout requested=requested />
        </div>
    }
}

#[component]
fn StageTab(
    label: &'static str,
    value: StageView,
    requested: RwSignal<StageView>,
) -> impl IntoView {
    let selected = move || requested.get() == value;
    view! {
        <button
            type="button"
            class="rounded-md px-2.5 py-1 text-[11px] font-medium uppercase tracking-wide transition-colors"
            class=("bg-accent/12", selected)
            class=("text-fg-primary", selected)
            class=("text-fg-tertiary/65", move || !selected())
            on:click=move |_| requested.set(value)
        >
            {label}
        </button>
    }
}

/// The §6.7 degraded indicator for the Stage Output view, shown when the
/// selected surface has a failed or asset-missing layer. The layer rail's
/// per-layer health pill (Wave 6) names the offending layer; this banner
/// is the surface-level alarm so trouble is visible without scanning rows.
#[component]
fn DegradedBanner() -> impl IntoView {
    view! {
        <div class="px-6 pt-4">
            <div class="flex items-start gap-2.5 rounded-xl border border-[rgba(255,99,99,0.28)] bg-[rgba(255,99,99,0.1)] px-4 py-3">
                <span class="mt-0.5 shrink-0 text-[rgba(255,99,99,0.94)]">
                    <Icon icon=LuTriangleAlert width="14px" height="14px" />
                </span>
                <div class="min-w-0">
                    <div class="text-[11px] font-semibold uppercase tracking-[0.16em] text-[rgba(255,99,99,0.84)]">
                        "Degraded"
                    </div>
                    <div class="mt-1 text-sm leading-5 text-fg-secondary">
                        "A layer on this surface failed to render or is missing its asset. Open the layer stack to see which."
                    </div>
                </div>
            </div>
        </div>
    }
}

/// The Stage shown while the synthetic Unassigned entry is selected. It is
/// not a surface (§9.4) — it has no composited output and no layer stack —
/// so the Stage shows the scene-level policy for device outputs claimed by
/// no zone. The policy is editable when the daemon advertises
/// `scene-unassigned-behavior-write`, and read-only otherwise.
#[component]
fn UnassignedStage() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let caps = expect_context::<CapabilitiesContext>();
    let writable = Memo::new(move |_| caps.has("scene-unassigned-behavior-write"));

    // The current behavior, encoded as a `SilkSelect` value: `off`,
    // `hold`, or `fallback:<zone id>`.
    let current_value = Memo::new(move |_| {
        studio
            .active_scene
            .get()
            .map(|scene| unassigned_behavior_value(&scene.unassigned_behavior))
            .unwrap_or_default()
    });
    let behavior_label = Memo::new(move |_| {
        studio
            .active_scene
            .get()
            .map(|scene| unassigned_behavior_label(&scene.unassigned_behavior))
            .unwrap_or_else(|| "—".to_owned())
    });

    // "Follow <zone>" needs one option per LED zone; the fallback target
    // cannot be the Unassigned entry itself, so only real zones list.
    let options = Memo::new(move |_| {
        let mut options = vec![
            ("off".to_owned(), "Turn off".to_owned()),
            ("hold".to_owned(), "Hold last colors".to_owned()),
        ];
        if let Some(scene) = studio.active_scene.get() {
            for surface in surfaces_from_groups(&scene.groups)
                .into_iter()
                .filter(|surface| surface.kind == SurfaceKind::Light)
            {
                options.push((
                    format!("fallback:{}", surface.id),
                    format!("Follow {}", surface.name),
                ));
            }
        }
        options
    });

    let on_change = Callback::new(move |value: String| {
        let Some(behavior) = parse_unassigned_behavior(&value) else {
            toasts::toast_error("Unrecognized unassigned-lights option");
            return;
        };
        let Some(scene) = studio.active_scene.get_untracked() else {
            toasts::toast_error("No active scene is available");
            return;
        };
        spawn_local(async move {
            match api::zones::update_unassigned_behavior(
                &scene.id,
                &behavior,
                Some(scene.groups_revision),
            )
            .await
            {
                Ok(ZoneOutcome::Applied(_)) => {
                    toasts::toast_success("Unassigned-lights policy updated");
                    studio.refresh_scene.run(());
                }
                Ok(ZoneOutcome::Stale { .. }) => {
                    toasts::toast_error("Scene changed elsewhere — reloaded, try again");
                    studio.refresh_scene.run(());
                }
                Err(error) => {
                    toasts::toast_error(&format!("Policy update failed: {error}"));
                }
            }
        });
    });

    view! {
        <div class="flex h-full flex-col bg-surface-sunken/20">
            <div class="flex items-center gap-2 border-b border-edge-subtle/60 px-5 py-3">
                <span class=label_class(LabelSize::Small, LabelTone::Default)>"Stage"</span>
                <span class="text-sm font-semibold text-fg-primary">"Unassigned lights"</span>
            </div>
            <div class="flex flex-1 items-center justify-center overflow-hidden p-6">
                <div class="max-w-md text-center">
                    <div class="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-xl bg-surface-sunken/70">
                        <Icon
                            icon=LuBan
                            width="22px"
                            height="22px"
                            style="color: rgba(241, 250, 140, 0.75)"
                        />
                    </div>
                    <div class="text-sm leading-5 text-fg-secondary">
                        "Device outputs in no zone follow the scene's unassigned-lights
                         policy."
                    </div>
                    <div class="mt-4">
                        <span class=label_class(LabelSize::Micro, LabelTone::Default)>
                            "Unassigned lights"
                        </span>
                        <div class="mt-1.5">
                            {move || {
                                if writable.get() {
                                    view! {
                                        <SilkSelect
                                            value=Signal::derive(move || current_value.get())
                                            options=Signal::derive(move || options.get())
                                            on_change=on_change
                                            class="border border-edge-subtle/70 bg-surface-overlay/40 px-3 py-2 text-sm"
                                        />
                                    }
                                        .into_any()
                                } else {
                                    view! {
                                        <span class="inline-flex items-center rounded-lg border border-edge-subtle/70 bg-surface-overlay/40 px-3 py-2 text-sm font-medium text-fg-primary">
                                            {move || behavior_label.get()}
                                        </span>
                                    }
                                        .into_any()
                                }
                            }}
                        </div>
                    </div>
                    <div class="mt-3 text-[12px] leading-5 text-fg-tertiary/65">
                        "Assign these outputs to a zone in a zone's Stage Layout view."
                    </div>
                </div>
            </div>
        </div>
    }
}

/// Encode an `UnassignedBehavior` as a `SilkSelect` option value.
#[must_use]
fn unassigned_behavior_value(behavior: &UnassignedBehavior) -> String {
    match behavior {
        UnassignedBehavior::Off => "off".to_owned(),
        UnassignedBehavior::Hold => "hold".to_owned(),
        UnassignedBehavior::Fallback(zone_id) => format!("fallback:{zone_id}"),
    }
}

/// Decode a `SilkSelect` option value back into an `UnassignedBehavior`.
#[must_use]
fn parse_unassigned_behavior(value: &str) -> Option<UnassignedBehavior> {
    match value {
        "off" => Some(UnassignedBehavior::Off),
        "hold" => Some(UnassignedBehavior::Hold),
        other => other
            .strip_prefix("fallback:")
            .and_then(|raw| raw.parse::<uuid::Uuid>().ok())
            .map(|uuid| UnassignedBehavior::Fallback(RenderGroupId(uuid))),
    }
}
