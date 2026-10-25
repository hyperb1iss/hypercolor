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

use std::collections::HashSet;

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::{DisplaysContext, WsContext};
use crate::components::display_preview_surface::DisplayPreviewSurface;
use crate::components::layout_builder::{LayoutEditorContext, LayoutWorkspace, ZoneCanvasActions};
use crate::components::mounting_select::MountingSelect;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::display_preview_state::use_display_preview_subscription;
use crate::display_utils::display_preview_shell_url;
use crate::icons::*;
use crate::ws::messages::zone_has_degraded_layer;

use super::surface::{Surface, SurfaceKind, now_playing, surfaces_from_zones};
use super::{StudioContext, hidden_outputs_storage_key};

/// Preview FPS ceiling for a Light Stage — the canvas is always live, so
/// it reserves the same headroom the retired `/layout` page did.
const LAYOUT_PREVIEW_FPS_CAP: u32 = 60;

/// The center Stage for the selected surface. A Light renders the
/// always-on layout editor; a Screen renders its live face preview.
#[component]
pub fn Stage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let studio = expect_context::<StudioContext>();
    let displays = expect_context::<DisplaysContext>().displays_resource;
    let editor = expect_context::<LayoutEditorContext>();

    // The device pane lives outside the ZoneLayoutProvider, so these
    // one-way effects push its highlight, hover, and hide state into the
    // editor context the canvas reads. Clicking a device or channel lights
    // the matching boxes; the eye toggle dims them (it used to write a
    // `hidden_outputs` signal nothing consumed — a no-op control).
    Effect::new(move |_| {
        let next = studio.selected_output_ids.get();
        if editor
            .selected_zone_ids
            .with_untracked(|current| *current != next)
        {
            editor.set_selected_zone_ids.set(next);
        }
    });
    // And back: a click on the canvas selects a device compound, so the
    // rail highlights the same cards the canvas lifted. The equality guards
    // on both directions are what stop the two effects ping-ponging.
    Effect::new(move |_| {
        let next = editor.selected_zone_ids.get();
        if studio
            .selected_output_ids
            .with_untracked(|current| *current != next)
        {
            studio.selected_output_ids.set(next);
        }
    });
    Effect::new(move |_| {
        editor
            .set_hovered_zone_ids
            .set(studio.hovered_output_ids.get());
    });
    // Mirror the canvas's box-under-pointer out to the rail, so hovering
    // a box softly highlights the card that owns it.
    Effect::new(move |_| {
        let next = editor.pointer_zone_id.get();
        if studio
            .pointer_output_id
            .with_untracked(|current| *current != next)
        {
            studio.pointer_output_id.set(next);
        }
    });
    Effect::new(move |_| {
        let hidden = match (studio.active_scene.get(), studio.selected_surface_id.get()) {
            (Some(scene), Some(zone)) => {
                let key = hidden_outputs_storage_key(&scene.id.to_string(), &zone);
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
        surfaces_from_zones(&scene.zones)
            .into_iter()
            .find(|surface| surface.id == id)
    });

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
            zone_has_degraded_layer(map, &scene.id.to_string(), &surface.id, &surface.layer_ids)
        })
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

    // Display preview frames are keyed by device, so the selected screen
    // reads its own stream directly.
    let screen_frame = Signal::derive(move || {
        let device_id = display_device.get()?;
        ws.display_preview_frames
            .with(|frames| frames.get(&device_id).cloned())
    });

    // The display-preview stream carries no FPS, so the Screen caption is
    // resolution only.
    let caption = Memo::new(move |_| {
        selected_display
            .get()
            .map(|display| format!("{}×{}", display.width, display.height))
            .unwrap_or_else(|| "—".to_owned())
    });
    // The screen's mount is a device setting, surfaced here so a panel
    // installed upside down is fixed where its preview is visible.
    let mount_device_id = Signal::derive(move || selected_display.get().map(|display| display.id));
    let mount_rotation =
        Signal::derive(move || selected_display.get().map(|display| display.rotation));

    view! {
        <div class="flex h-full flex-col bg-surface-sunken/20">
            <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-5 py-3">
                <NowPlayingChip surface=selected_surface />
                {move || {
                    if is_screen.get() {
                        view! {
                            <div class="flex items-center gap-2">
                                <MountingSelect
                                    device_id=mount_device_id
                                    rotation=mount_rotation
                                    class="border border-edge-subtle/60 bg-surface-overlay/40 px-2.5 py-1 text-[11px] text-fg-secondary"
                                />
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
                                                    href=crate::route_ui::route_href(&display_preview_shell_url(&display.id))
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
                                                fallback_route=api::display_preview_route(
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
                    // Devices move between zones from the device pane's
                    // per-card controls, so no assignment panel docks here.
                    view! {
                        <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                            {move || {
                                surface_degraded.get().then(|| view! { <DegradedBanner /> })
                            }}
                            // Must be a flex column: LayoutWorkspace's body is
                            // a flex-1 child with no h-full, so a plain block
                            // here collapses the canvas to zero height.
                            <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                                <div class="contents" inert=move || studio.history.busy.get() aria-busy=move || studio.history.busy.get().to_string()>
                                    <LayoutWorkspace compact=true />
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

/// The Light Stage header's zone-canvas controls — undo / redo and
/// Revert / Save for the selected zone's layout. Save doubles as the
/// dirty indicator. Reads the [`LayoutEditorContext`] and
/// [`ZoneCanvasActions`] a `ZoneLayoutProvider` mounts on a Studio
/// ancestor.
#[component]
fn ZoneCanvasBar() -> impl IntoView {
    let editor = expect_context::<LayoutEditorContext>();
    let actions = expect_context::<ZoneCanvasActions>();
    let history = expect_context::<super::history::StudioHistory>();
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

                <span role="status" class="text-[11px] text-fg-tertiary" title="Device assignments save automatically; layout changes save when you choose Save.">
                    {move || if history.busy.get() { "Saving…" } else if is_dirty.get() { "Unsaved layout" } else { "Saved" }}
                </span>
                <button
                    type="button"
                    class="flex items-center gap-1 rounded-md border border-edge-subtle px-2 py-1 text-[11px] font-medium text-fg-secondary transition-colors hover:bg-surface-hover disabled:opacity-40 btn-press"
                    title="Revert unsaved layout changes"
                    on:click=move |_| revert.run(())
                    disabled=move || !is_dirty.get() || history.busy.get()
                >
                    <Icon icon=LuUndo2 width="12px" height="12px" />
                    "Revert"
                </button>
                <button
                    type="button"
                    class="flex items-center gap-1 rounded-md border border-accent-muted bg-accent-muted px-2 py-1 text-[11px] font-medium text-fg-primary transition-colors hover:bg-surface-hover disabled:opacity-40 btn-press"
                    title="Save layout changes"
                    on:click=move |_| save.run(())
                    disabled=move || !is_dirty.get() || history.busy.get()
                >
                    <Icon icon=LuSave width="12px" height="12px" />
                    "Save"
                </button>
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
    // A Screen painting its stored default names that face; the pill
    // says it is the display's default rather than a layer of this scene.
    let playing = Memo::new(move |_| {
        let surface = surface.get();
        let default_face = studio
            .screen_face
            .get()
            .filter(|face| face.live_scope == api::DisplayFaceScope::Default)
            .map(|face| face.effect.name);
        now_playing(surface.as_ref(), default_face.as_deref())
    });
    let label = move || playing.get().label;
    let is_default_face = move || playing.get().is_default_face;
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
            <Show when=is_default_face>
                <span
                    class="rounded-full border px-1.5 py-px text-[9px] font-semibold uppercase tracking-[0.14em]"
                    style="border-color: rgba(225, 53, 255, 0.35); color: rgba(225, 53, 255, 0.85)"
                    title="The display's default face, shown in every scene without its own face layer"
                >
                    "Default"
                </span>
            </Show>
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
