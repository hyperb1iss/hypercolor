//! The Studio Stage — the center workspace for the selected surface.
//!
//! For a Light the Stage *is* the spatial layout editor: the live
//! effect renders under the device boxes, always on, with no view
//! toggle. Its header carries the now-playing chip and the zone-canvas
//! controls — undo / redo and Revert / Save — driven by the
//! context-provided `LayoutEditorContext` and `ZoneCanvasActions` from a
//! `ZoneLayoutProvider` mounted on a Studio ancestor.
//!
//! A Screen shows that device's live face via `DisplayPreviewSurface`.
//! The synthetic Unassigned entry (§9.4) is not a surface, so it shows
//! the scene-level unassigned-lights policy instead.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_types::scene::{UnassignedBehavior, ZoneId};

use crate::api;
use crate::api::zones::ZoneOutcome;
use crate::app::{CapabilitiesContext, DisplaysContext, WsContext};
use crate::components::display_preview_surface::DisplayPreviewSurface;
use crate::components::layout_builder::{LayoutEditorContext, LayoutWorkspace, ZoneCanvasActions};
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::silk_select::SilkSelect;
use crate::display_preview_state::use_display_preview_subscription;
use crate::display_utils::display_preview_shell_url;
use crate::icons::*;
use crate::toasts;
use crate::ws::CanvasFrame;
use crate::ws::messages::group_has_degraded_layer;

use super::surface::{Surface, SurfaceKind, UNASSIGNED_SURFACE_ID, surfaces_from_groups};
use super::zone_assignment::ZoneAssignment;
use super::zone_controls::unassigned_behavior_label;
use super::{StudioContext, hidden_outputs_storage_key};

/// Preview FPS ceiling for a Light Stage — the canvas is always live, so
/// it reserves the same headroom the retired `/layout` page did.
const LAYOUT_PREVIEW_FPS_CAP: u32 = 60;

/// The center Stage. Dispatches on the current selection: a real surface
/// renders its editor or preview, the synthetic Unassigned entry (§9.4)
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

/// The Stage for a real surface. A Light renders the always-on layout
/// editor; a Screen renders its live face preview.
#[component]
fn SurfaceStage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let studio = expect_context::<StudioContext>();
    let displays = expect_context::<DisplaysContext>().displays_resource;
    let editor = expect_context::<LayoutEditorContext>();

    // The rail (ZoneTree) lives outside the ZoneLayoutProvider, so these
    // one-way effects push its highlight, hover, and hide state into the
    // editor context the canvas reads. Clicking a device or channel lights
    // the matching boxes; the eye toggle dims them (it used to write a
    // `hidden_outputs` signal nothing consumed — a no-op control).
    Effect::new(move |_| {
        editor
            .set_selected_zone_ids
            .set(studio.selected_output_ids.get());
    });
    Effect::new(move |_| {
        editor
            .set_hovered_zone_ids
            .set(studio.hovered_output_ids.get());
    });
    Effect::new(move |_| {
        let hidden = match (studio.active_scene.get(), studio.selected_surface_id.get()) {
            (Some(scene), Some(zone)) => {
                let key = hidden_outputs_storage_key(&scene.id, &zone);
                studio
                    .hidden_outputs
                    .with(|map| map.get(&key).cloned().unwrap_or_default())
            }
            _ => HashSet::new(),
        };
        editor.set_hidden_zones.set(hidden);
    });

    let selected_surface = Memo::new(move |_| {
        let id = studio.selected_surface_id.get()?;
        let scene = studio.active_scene.get()?;
        surfaces_from_groups(&scene.groups)
            .into_iter()
            .find(|surface| surface.id == id)
    });

    let is_screen =
        Memo::new(move |_| selected_surface.get().map(|s| s.kind) == Some(SurfaceKind::Screen));
    let multi_zone = Memo::new(move |_| {
        studio
            .active_scene
            .get()
            .is_some_and(|scene| super::surface::led_zone_count(&scene.groups) > 1)
    });

    // The selected surface flags itself when its layer stack has a failed or
    // asset-missing layer — the §6.7 Stage-side degraded indicator.
    let surface_degraded = Memo::new(move |_| {
        let (Some(surface), Some(scene)) = (selected_surface.get(), studio.active_scene.get())
        else {
            return false;
        };
        ws.layer_health
            .with(|map| group_has_degraded_layer(map, &scene.id, &surface.id, &surface.layer_ids))
    });

    // A Light keeps the canvas live, so it reserves the same preview
    // headroom the `/layout` page did; a Screen uses the shared default.
    Effect::new(move |_| {
        let cap = if is_screen.get() {
            crate::ws::DEFAULT_PREVIEW_FPS_CAP
        } else {
            LAYOUT_PREVIEW_FPS_CAP
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
    Effect::new(move |previous_device: Option<Option<String>>| {
        let current_device = display_device.get();
        if previous_device.as_ref() != Some(&current_device) {
            screen_frame.set(None);
        }
        current_device
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

    // The display-preview stream carries no FPS, so the Screen caption is
    // resolution only.
    let caption = Memo::new(move |_| {
        selected_display
            .get()
            .map(|display| format!("{}×{}", display.width, display.height))
            .unwrap_or_else(|| "—".to_owned())
    });

    view! {
        <div class="flex h-full flex-col bg-surface-sunken/20">
            <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-5 py-3">
                <NowPlayingChip surface=selected_surface />
                {move || {
                    if is_screen.get() {
                        view! {
                            <div class="flex items-center gap-2">
                                <span class=label_class(
                                    LabelSize::Micro,
                                    LabelTone::Default,
                                )>"Preview"</span>
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
                        view! { <ZoneCanvasBar /> }.into_any()
                    }
                }}
            </div>

            {move || {
                if is_screen.get() {
                    // Screen surface — the live device face.
                    view! {
                        <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                            {move || {
                                surface_degraded.get().then(|| view! { <DegradedBanner /> })
                            }}
                            <div class="flex flex-1 items-center justify-center overflow-hidden p-6">
                                <div class="flex max-w-full flex-col items-center gap-3">
                                    {move || {
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
                                    }}
                                    <div class="font-mono text-[11px] tabular-nums text-fg-tertiary/70">
                                        {move || caption.get()}
                                    </div>
                                </div>
                            </div>
                        </div>
                    }
                        .into_any()
                } else {
                    // Light surface — the always-on spatial layout editor.
                    // In a multi-zone scene the zone-assignment panel docks
                    // below it (§9.3).
                    view! {
                        <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                            {move || {
                                surface_degraded.get().then(|| view! { <DegradedBanner /> })
                            }}
                            // Must be a flex column: LayoutWorkspace's body is
                            // a flex-1 child with no h-full, so a plain block
                            // here collapses the canvas to zero height.
                            <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                                <LayoutWorkspace compact=true />
                            </div>
                            {move || multi_zone.get().then(|| view! { <ZoneAssignment /> })}
                        </div>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// The Light Stage header's zone-canvas controls — undo / redo and
/// Revert / Save for the selected zone's layout. Save doubles as the
/// dirty indicator. Reads the [`LayoutEditorContext`] and
/// [`ZoneCanvasActions`] a `ZoneLayoutProvider` mounts on a Studio
/// ancestor.
#[component]
fn ZoneCanvasBar() -> impl IntoView {
    let editor = expect_context::<LayoutEditorContext>();
    let actions = expect_context::<ZoneCanvasActions>();
    let write = editor.set_layout;
    let can_undo = editor.can_undo;
    let can_redo = editor.can_redo;
    let is_dirty = actions.is_dirty;
    let has_layout = actions.has_layout;
    let save = actions.save;
    let revert = actions.revert;

    view! {
        <Show when=move || has_layout.get()>
            <div class="flex items-center gap-1.5">
                <button
                    type="button"
                    class="rounded-md p-1.5 text-fg-tertiary transition-colors hover:bg-surface-hover/40 hover:text-fg-primary disabled:pointer-events-none disabled:opacity-30"
                    title="Undo (Ctrl+Z)"
                    on:click=move |_| write.undo()
                    disabled=move || !can_undo.get()
                >
                    <Icon icon=LuUndo2 width="14px" height="14px" />
                </button>
                <button
                    type="button"
                    class="rounded-md p-1.5 text-fg-tertiary transition-colors hover:bg-surface-hover/40 hover:text-fg-primary disabled:pointer-events-none disabled:opacity-30"
                    title="Redo (Ctrl+Shift+Z)"
                    on:click=move |_| write.redo()
                    disabled=move || !can_redo.get()
                >
                    <Icon icon=LuRedo2 width="14px" height="14px" />
                </button>

                <div class="mx-0.5 h-5 w-px bg-edge-subtle/40" />

                // Revert / Save — Save doubles as the dirty indicator.
                {move || {
                    let dirty = is_dirty.get();
                    let save_style = if dirty {
                        "background: rgba(80, 250, 123, 0.14); border-color: rgba(80, 250, 123, 0.35); color: rgb(80, 250, 123); box-shadow: 0 0 12px rgba(80, 250, 123, 0.16)"
                    } else {
                        "background: var(--color-surface-overlay); border-color: var(--color-border-subtle); color: var(--color-text-tertiary); opacity: 0.4; pointer-events: none"
                    };
                    let revert_style = if dirty {
                        "background: rgba(241, 250, 140, 0.08); border-color: rgba(241, 250, 140, 0.25); color: rgb(241, 250, 140)"
                    } else {
                        "background: var(--color-surface-overlay); border-color: var(--color-border-subtle); color: var(--color-text-tertiary); opacity: 0.4; pointer-events: none"
                    };
                    view! {
                        <button
                            type="button"
                            class="flex items-center gap-1 rounded-md border px-2 py-1 text-[11px] font-medium transition-all btn-press"
                            style=revert_style
                            on:click=move |_| revert.run(())
                            disabled=move || !is_dirty.get()
                        >
                            <Icon icon=LuUndo2 width="12px" height="12px" />
                            "Revert"
                        </button>
                        <button
                            type="button"
                            class="flex items-center gap-1 rounded-md border px-2 py-1 text-[11px] font-medium transition-all btn-press"
                            style=save_style
                            on:click=move |_| save.run(())
                            disabled=move || !is_dirty.get()
                        >
                            <Icon icon=LuSave width="12px" height="12px" />
                            "Save"
                        </button>
                    }
                }}
            </div>
        </Show>
    }
}

/// The Stage header's now-playing chip. Names the selected surface's
/// top layer and, on click, toggles the composition slide-over — the
/// only way layer editing is summoned in the two-column workspace.
/// Rendered for every surface, Light and Screen alike: both carry a
/// layer stack, so both need the composition trigger.
#[component]
fn NowPlayingChip(#[prop(into)] surface: Signal<Option<Surface>>) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let label = move || {
        surface
            .get()
            .and_then(|surface| surface.top_layer)
            .unwrap_or_else(|| "No layers".to_owned())
    };
    view! {
        <button
            type="button"
            class="group flex items-center gap-2 rounded-lg border border-edge-subtle/60 bg-surface-overlay/40 px-3 py-1.5 transition-colors hover:border-accent-muted hover:bg-surface-overlay/70"
            title="Open the composition panel"
            on:click=move |_| studio.composition_open.update(|open| *open = !*open)
        >
            <Icon
                icon=LuLayers
                width="13px"
                height="13px"
                style="color: rgba(225, 53, 255, 0.75)"
            />
            <span class="max-w-[200px] truncate text-[12px] font-medium text-fg-secondary group-hover:text-fg-primary">
                {label}
            </span>
            <Icon
                icon=LuChevronRight
                width="12px"
                height="12px"
                style="color: rgba(139, 133, 160, 0.55)"
            />
        </button>
    }
}

/// The §6.7 degraded indicator for the Stage, shown when the selected
/// surface has a failed or asset-missing layer. The layer rail's
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
                <span class="text-sm font-semibold text-fg-primary">"Unassigned lights"</span>
            </div>
            <div class="flex flex-1 items-center justify-center overflow-hidden p-6">
                <div class="w-full max-w-[28rem] text-center">
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
                        "Assign these outputs to a zone with the zone-assignment panel below the canvas."
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
            .map(|uuid| UnassignedBehavior::Fallback(ZoneId(uuid))),
    }
}
