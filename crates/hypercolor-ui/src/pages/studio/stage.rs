//! The Studio Stage: the center workspace for the open zone.
//!
//! The Stage *is* the spatial layout editor: the live effect renders
//! under the device boxes, always on, with no view toggle. Its header
//! carries the now-playing chip and the zone-canvas controls (undo, redo,
//! and Fit), driven by the context-provided `LayoutEditorContext` and
//! `ZoneCanvasActions` from a `ZoneLayoutProvider` mounted on a Studio
//! ancestor. Layout edits commit as each gesture ends, so there is
//! nothing to save.
//!
//! A screen is one of the boxes, and its box shows the panel's live
//! picture. With screens alone selected the chip names their face and
//! opens its settings, and with one selected the header offers Focus: the
//! Stage filled with that panel at full size, for close work.

use std::collections::{BTreeSet, HashMap, HashSet};

use hypercolor_leptos_ext::events::target_is_text_entry;
use hypercolor_types::layer::LayerSource;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::{DisplaysContext, WsContext};
use crate::components::display_preview_surface::DisplayPreviewSurface;
use crate::components::layer_panel::scope::Target;
use crate::components::layout_builder::{
    LayoutEditorContext, LayoutWorkspace, ScreenTile, ZoneCanvasActions,
};
use crate::components::mounting_select::MountingSelect;
use crate::components::preview_box::contained_width;
use crate::display_preview_state::{display_preview_frame, use_display_previews};
use crate::display_rotation::{mounted_footprint, mounted_preview};
use crate::display_utils::display_preview_shell_url;
use crate::icons::*;
use crate::ws::messages::zone_has_degraded_layer;

use super::composition_panel::{COMPOSITION_PANEL_ID, COMPOSITION_TRIGGER_ID};
use super::layer_target::PickBanner;
use super::screen_model::screen_chip;
use super::surface::{Surface, layer_label, now_playing, surfaces_from_zones, zone_featured_layer};
use super::{StudioContext, hidden_outputs_storage_key};

/// The longer side of a focused screen on the Stage, in CSS pixels.
const FOCUS_EXTENT: f64 = 640.0;

/// Preview FPS ceiling for the Stage: the canvas is always live, so it
/// reserves the same headroom the retired `/layout` page did.
const LAYOUT_PREVIEW_FPS_CAP: u32 = 60;

/// The center Stage for the open zone: the always-on layout editor, or
/// the one selected screen at full size while it is in focus.
#[component]
pub fn Stage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let studio = expect_context::<StudioContext>();
    let displays = expect_context::<DisplaysContext>().displays_resource;
    let editor = expect_context::<LayoutEditorContext>();
    let screens = studio.screens;

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
    // What a layer row or a checklist entry points at takes the accent
    // outline on the canvas.
    Effect::new(move |_| editor.layer_focus.set(studio.targets.lit.get()));
    // A pick hands the canvas what is picked so far and takes each toggle
    // back. The equality guards on both directions stop the two effects
    // ping-ponging.
    Effect::new(move |_| {
        let next = studio
            .targets
            .pick
            .with(|pick| pick.as_ref().map(|pick| pick.draft.ids()));
        if editor.picking.with_untracked(|current| *current != next) {
            editor.picking.set(next);
        }
    });
    Effect::new(move |_| {
        let Some(picked) = editor.picking.get() else {
            return;
        };
        let stale = studio
            .targets
            .pick
            .with_untracked(|pick| pick.as_ref().is_some_and(|pick| pick.draft.ids() != picked));
        if stale {
            studio.targets.pick.update(|pick| {
                if let Some(pick) = pick {
                    pick.draft = Target::Outputs(picked.into_iter().collect());
                }
            });
        }
    });
    // What a target cannot name, a pick cannot take.
    Effect::new(move |_| {
        let blocked = studio.targets.outputs.with(|outputs| {
            outputs
                .iter()
                .filter(|output| !output.pickable)
                .map(|output| output.id.clone())
                .collect()
        });
        editor.pick_blocked.set(blocked);
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

    // The open zone's screens, with the panels behind them: each box
    // shows its panel's live picture.
    let tiles = Memo::new(move |_| {
        let zone_id = studio.selected_surface_id.get()?;
        let panels = displays.get().and_then(Result::ok)?;
        Some(screens.placed.with(|placed| {
            placed
                .iter()
                .filter(|screen| screen.zone_id == zone_id)
                .filter_map(|screen| {
                    let device_id = screen.device_id.as_ref()?;
                    let panel = panels.iter().find(|panel| panel.id == *device_id)?;
                    Some((
                        screen.output_id.clone(),
                        ScreenTile {
                            device_id: device_id.clone(),
                            name: screen.name.clone(),
                            width: panel.width,
                            height: panel.height,
                            circular: panel.circular,
                            rotation: panel.rotation,
                        },
                    ))
                })
                .collect::<HashMap<_, _>>()
        }))
    });
    Effect::new(move |_| {
        let next = tiles.get().unwrap_or_default();
        if editor
            .screen_tiles
            .with_untracked(|current| *current != next)
        {
            editor.screen_tiles.set(next);
        }
    });
    // Every screen on the canvas rides its own preview stream.
    use_display_previews(
        ws,
        Signal::derive(move || {
            tiles.with(|tiles| {
                tiles
                    .iter()
                    .flat_map(HashMap::values)
                    .map(|tile| tile.device_id.clone())
                    .collect::<BTreeSet<_>>()
            })
        }),
    );

    let selected_surface = Memo::new(move |_| {
        let id = studio.selected_surface_id.get()?;
        let scene = studio.active_scene.get()?;
        surfaces_from_zones(&scene.zones)
            .into_iter()
            .find(|surface| surface.id == id)
    });

    // The zone flags itself when its stack has a failed or asset-missing
    // layer. A screen's own layers report under the screen's view, so
    // those are read too.
    let surface_degraded = Memo::new(move |_| {
        let (Some(surface), Some(scene)) = (selected_surface.get(), studio.active_scene.get())
        else {
            return false;
        };
        let scene_id = scene.id.to_string();
        ws.layer_health.with(|map| {
            scene
                .zones
                .iter()
                .filter(|zone| {
                    zone.id.to_string() == surface.id
                        || zone
                            .view_of
                            .as_ref()
                            .is_some_and(|view| view.zone.to_string() == surface.id)
                })
                .any(|zone| {
                    zone_has_degraded_layer(
                        map,
                        &scene_id,
                        &zone.id.to_string(),
                        &surface.layer_ids,
                    )
                })
        })
    });

    // The canvas stays live, so it reserves the preview headroom the
    // `/layout` page did.
    Effect::new(move |_| {
        ws.set_preview_cap.set(LAYOUT_PREVIEW_FPS_CAP);
        ws.set_preview_width_cap.set(0);
    });
    on_cleanup(move || {
        ws.set_preview_cap.set(crate::ws::DEFAULT_PREVIEW_FPS_CAP);
        ws.set_preview_width_cap.set(0);
    });

    // Escape leaves focus as it would any mode, unless a field has it or
    // something opened over the Stage takes the key first.
    let keydown = window_event_listener(leptos::ev::keydown, move |event| {
        if event.key() == "Escape"
            && screens.focus.get_untracked()
            && !event.default_prevented()
            && !target_is_text_entry(event.target())
            && studio.targets.editor.with_untracked(Option::is_none)
            && studio.targets.pick.with_untracked(Option::is_none)
        {
            event.prevent_default();
            screens.focus.set(false);
        }
    });
    on_cleanup(move || keydown.remove());

    view! {
        <div class="studio-zone-content flex h-full flex-col bg-surface-sunken/20">
            <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-5 py-3">
                <NowPlayingChip surface=selected_surface />
                <div class="flex items-center gap-1.5">
                    <FocusToggle />
                    <Show when=move || !screens.focus.get()>
                        <ZoneCanvasBar />
                    </Show>
                </div>
            </div>

            <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                {move || surface_degraded.get().then(|| view! { <DegradedBanner /> })}
                <PickBanner />
                <Show when=move || screens.focus.get()>
                    <FocusedScreen />
                </Show>
                // The canvas stays mounted under a focused screen, hidden,
                // so leaving focus brings it back as it was. It must be a
                // flex column: LayoutWorkspace's body is a flex-1 child with
                // no h-full, so a plain block here collapses the canvas to
                // zero height.
                <div
                    class="flex min-h-0 flex-1 flex-col overflow-hidden"
                    class=("hidden", move || screens.focus.get())
                >
                    <div class="contents" inert=move || studio.history.busy.get() aria-busy=move || studio.history.busy.get().to_string()>
                        <LayoutWorkspace />
                    </div>
                </div>
            </div>
        </div>
    }
}

/// The one selected screen's panel, as the rig knows it.
fn selected_panel(studio: StudioContext) -> Memo<Option<(api::DisplaySummary, String)>> {
    let displays = expect_context::<DisplaysContext>().displays_resource;
    Memo::new(move |_| {
        let (device_id, name) =
            studio
                .screens
                .selected
                .with(|selected| match selected.as_slice() {
                    [one] => Some((one.device_id.clone()?, one.name.clone())),
                    _ => None,
                })?;
        displays
            .get()
            .and_then(Result::ok)?
            .into_iter()
            .find(|panel| panel.id == device_id)
            .map(|panel| (panel, name))
    })
}

/// The Stage toolbar's Focus toggle, offered while one screen is
/// selected: fill the Stage with that panel, and back.
#[component]
fn FocusToggle() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let focus = studio.screens.focus;
    let panel = selected_panel(studio);
    view! {
        <Show when=move || panel.with(Option::is_some)>
            <button
                type="button"
                id="studio-screen-focus"
                class="flex items-center gap-1 rounded-md border px-2 py-1 text-[11px] font-medium transition-colors btn-press"
                class=("border-accent-muted", move || focus.get())
                class=("bg-accent/10", move || focus.get())
                class=("text-fg-primary", move || focus.get())
                class=("border-edge-subtle", move || !focus.get())
                class=("text-fg-secondary", move || !focus.get())
                class=("hover:bg-surface-hover", move || !focus.get())
                aria-pressed=move || focus.get().to_string()
                title=move || {
                    if focus.get() {
                        "Back to the canvas (Esc)"
                    } else {
                        "Fill the Stage with this screen"
                    }
                }
                on:click=move |_| focus.update(|on| *on = !*on)
            >
                <Icon icon=LuScan width="12px" height="12px" />
                "Focus"
            </button>
        </Show>
    }
}

/// The selected screen filling the Stage: the panel at full size, round
/// where the panel is round and turned the way it is mounted, with its
/// mounting and the full-screen preview beside it.
#[component]
fn FocusedScreen() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let ws = expect_context::<WsContext>();
    let panel = selected_panel(studio);
    let device_id = Signal::derive(move || {
        panel.with(|panel| panel.as_ref().map(|(panel, _)| panel.id.clone()))
    });
    let rotation =
        Signal::derive(move || panel.with(|panel| panel.as_ref().map(|(panel, _)| panel.rotation)));
    let frame = display_preview_frame(ws, device_id);
    // Keyed on what lays the panel out, so a frame never rebuilds it.
    let shape = Memo::new(move |_| {
        panel.with(|panel| {
            panel.as_ref().map(|(panel, name)| {
                (
                    panel.id.clone(),
                    name.clone(),
                    panel.width,
                    panel.height,
                    panel.circular,
                    panel.rotation,
                )
            })
        })
    });
    view! {
        <div class="flex min-h-0 flex-1 flex-col overflow-hidden animate-enter-fade" data-studio-focus="">
            <div class="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-edge-subtle/60 px-5 py-2 max-md:px-3">
                <div class="flex min-w-0 items-center gap-2 text-[12px]">
                    <span class="flex shrink-0 text-fg-tertiary">
                        <Icon icon=LuMonitor width="13px" height="13px" />
                    </span>
                    <span class="min-w-0 truncate font-medium text-fg-primary">
                        {move || shape.get().map(|(_, name, ..)| name)}
                    </span>
                    <span class="shrink-0 font-mono text-[11px] tabular-nums text-fg-tertiary">
                        {move || shape.get().map(|(_, _, width, height, ..)| format!("{width}\u{d7}{height}"))}
                    </span>
                </div>
                <div class="flex items-center gap-2">
                    <MountingSelect
                        device_id=device_id
                        rotation=rotation
                        class="border border-edge-subtle/60 bg-surface-overlay/40 px-2.5 py-1 text-[11px] text-fg-secondary"
                    />
                    {move || device_id.get().map(|id| view! { <PreviewLink device_id=id /> })}
                </div>
            </div>
            <div class="flex min-h-0 flex-1 items-center justify-center overflow-hidden p-6 max-md:p-4">
                {move || {
                    let (id, name, width, height, circular, mount) = shape.get()?;
                    let container_class = format!(
                        "w-full overflow-hidden border border-edge-subtle/70 bg-black {}",
                        if circular { "rounded-full" } else { "rounded-xl" },
                    );
                    // Bounded on its longer side, so a panel standing on its
                    // end is no taller than a square one.
                    let max_width = contained_width(
                        mounted_footprint(width, height, mount),
                        FOCUS_EXTENT,
                        FOCUS_EXTENT,
                    );
                    Some(view! {
                        <div class="max-h-full w-full" style=format!("max-width: {max_width:.0}px;")>
                            <DisplayPreviewSurface
                                frame=frame
                                fallback_route=api::display_preview_route(&id, None)
                                aspect_ratio=format!("{} / {}", width.max(1), height.max(1))
                                mounted=mounted_preview(width, height, mount)
                                aria_label=format!("{name} in focus")
                                container_class=container_class
                            />
                        </div>
                    })
                }}
            </div>
        </div>
    }
}

/// The link that opens a screen's live face full screen in a new tab.
#[component]
pub(crate) fn PreviewLink(device_id: String) -> impl IntoView {
    view! {
        <a
            href=crate::route_ui::route_href(&display_preview_shell_url(&device_id))
            target="_blank"
            rel="noopener"
            class="flex h-7 shrink-0 items-center gap-1 rounded-md border border-edge-subtle px-2 text-[11px] font-medium text-fg-secondary transition-colors hover:border-accent-muted hover:text-fg-primary"
            title="Open this screen's live face full screen, in a new tab"
            aria-label="Open full-screen preview"
        >
            <Icon icon=LuExternalLink width="12px" height="12px" />
            "Preview"
        </a>
    }
}

/// The Stage header's zone-canvas controls: undo, redo, and Fit for
/// the selected zone's layout. Reads the [`LayoutEditorContext`] and
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
    let has_layout = actions.has_layout;
    let fit = actions.fit;
    let can_fit = actions.can_fit;

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
                <button
                    type="button"
                    class="flex items-center gap-1 rounded-md border border-edge-subtle px-2 py-1 text-[11px] font-medium text-fg-secondary transition-colors hover:bg-surface-hover disabled:opacity-40 btn-press"
                    title="Fit every output to the canvas"
                    on:click=move |_| fit.run(())
                    disabled=move || !can_fit.get() || history.busy.get()
                >
                    <Icon icon=LuMaximize width="12px" height="12px" />
                    "Fit"
                </button>
            </div>
        </Show>
    }
}

/// The Stage header's now-playing chip. Names the open zone's layer and
/// opens its layers; with screens alone selected it names their face and
/// opens its settings, or offers a face when they show none. Toggles the
/// composition slide-over.
#[component]
fn NowPlayingChip(#[prop(into)] surface: Signal<Option<Surface>>) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let fx = expect_context::<crate::app::EffectsContext>();
    let screens = studio.screens;
    // An effect layer without a stored name takes the effect's own name
    // from the index, as the dashboard caption does, rather than the bare
    // word "Effect".
    let effect_name = move |effect_id: &str| {
        fx.effects_index.with(|effects| {
            effects
                .iter()
                .find(|entry| entry.effect.id == effect_id)
                .map(|entry| entry.effect.name.clone())
        })
    };
    let playing = Memo::new(move |_| {
        let mut surface = surface.get();
        if let Some(current) = surface.as_mut()
            && let Some(caption) = studio.active_scene.with(|scene| {
                let top = zone_featured_layer(
                    scene
                        .as_ref()?
                        .zones
                        .iter()
                        .find(|zone| zone.id.to_string() == current.id)?,
                )?;
                let effect = match &top.source {
                    LayerSource::Effect { effect_id, .. } => effect_name(&effect_id.to_string()),
                    _ => None,
                };
                Some(layer_label(top, effect.as_deref()))
            })
        {
            current.top_layer = Some(caption);
        }
        now_playing(surface.as_ref())
    });
    let on_screens = Memo::new(move |_| screens.selected.with(|selected| !selected.is_empty()));
    let chip = Memo::new(move |_| {
        on_screens.get().then(|| {
            let face = screens.face_name(studio, effect_name);
            screen_chip(face.as_deref(), None)
        })
    });
    let offers_face =
        Memo::new(move |_| chip.with(|chip| chip.as_ref().is_some_and(|chip| chip.face.is_none())));
    let label = move || match chip.get() {
        Some(chip) => chip.face,
        None => Some(playing.get()),
    };
    let action = move || chip.get().map_or("Layers", |chip| chip.action());
    view! {
        <button
            type="button"
            id=COMPOSITION_TRIGGER_ID
            aria-controls=COMPOSITION_PANEL_ID
            aria-expanded=move || studio.composition_open.get().to_string()
            class="group flex min-w-0 items-center gap-2 rounded-lg border border-edge-subtle/60 bg-surface-overlay/40 px-3 py-1.5 transition-colors hover:border-accent-muted hover:bg-surface-overlay/70"
            title=move || {
                if offers_face.get() {
                    "Give this screen a face"
                } else if on_screens.get() {
                    "Change this screen's face and its settings"
                } else {
                    "Edit this zone's layers and their settings"
                }
            }
            data-studio-chip=move || if on_screens.get() { "screen" } else { "zone" }
            on:click=move |_| {
                // "+ Face" asks for a face: the panel opens with the picker.
                if offers_face.get_untracked() {
                    screens.open(studio, true);
                } else {
                    studio.composition_open.update(|open| *open = !*open);
                }
            }
        >
            <span class="flex shrink-0 text-accent/75">
                {move || {
                    if offers_face.get() {
                        view! { <Icon icon=LuPlus width="13px" height="13px" /> }.into_any()
                    } else if on_screens.get() {
                        view! { <Icon icon=LuMonitor width="13px" height="13px" /> }.into_any()
                    } else {
                        view! { <Icon icon=LuLayers width="13px" height="13px" /> }.into_any()
                    }
                }}
            </span>
            {move || label().map(|label| view! {
                <span class="min-w-0 max-w-[200px] truncate text-[12px] font-medium text-fg-secondary group-hover:text-fg-primary">
                    {label}
                </span>
            })}
            <span
                class="shrink-0 text-[11px] group-hover:text-fg-secondary"
                class=("border-l", move || !offers_face.get())
                class=("border-edge-subtle", move || !offers_face.get())
                class=("pl-2", move || !offers_face.get())
                class=("text-fg-tertiary", move || !offers_face.get())
                class=("font-medium", move || offers_face.get())
                class=("text-fg-secondary", move || offers_face.get())
                // A phone has room for the name or for what a tap opens,
                // not both beside the toolbar; the name says more.
                class=("max-sm:hidden", move || !offers_face.get())
            >
                {action}
            </span>
            <Show when=move || !offers_face.get()>
                <span
                    class="flex shrink-0 text-fg-tertiary transition-transform duration-200"
                    class=("rotate-180", move || studio.composition_open.get())
                >
                    <Icon icon=LuChevronRight width="12px" height="12px" />
                </span>
            </Show>
        </button>
    }
}

/// The degraded indicator for the Stage, shown when the open zone has a
/// failed or asset-missing layer. The layer rail's
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
                        "A layer in this zone failed to render or is missing its asset. Open its layers to see which."
                    </div>
                </div>
            </div>
        </div>
    }
}
