//! The device pane while the Screens tab is active: one card per Screen,
//! each with a thumbnail of its face. Selecting a card puts that screen's
//! face and mounting on the Stage, and the composition slide-over edits
//! its layers.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::{DisplaysContext, WsContext};
use crate::components::display_preview_surface::DisplayPreviewSurface;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;

use super::StudioContext;
use super::surface::{Surface, SurfaceKind, surfaces_from_zones};

/// The Screens list, filtered by the pane's normalized search term.
#[component]
pub fn ScreenList(#[prop(into)] query: Signal<String>) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let screens = Memo::new(move |_| {
        studio.active_scene.with(|scene| {
            scene
                .as_ref()
                .map(|scene| {
                    surfaces_from_zones(&scene.zones)
                        .into_iter()
                        .filter(|surface| surface.kind == SurfaceKind::Screen)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let label_of = move |surface: &Surface| {
        studio
            .screen_labels
            .with(|labels| labels.get(&surface.id).cloned())
            .unwrap_or_else(|| surface.name.clone())
    };
    let shown = Memo::new(move |_| {
        let query = query.get();
        screens
            .get()
            .into_iter()
            .map(|surface| {
                let label = label_of(&surface);
                (surface, label)
            })
            .filter(|(_, label)| query.is_empty() || label.to_lowercase().contains(&query))
            .collect::<Vec<_>>()
    });
    let count = move || {
        let total = screens.with(Vec::len);
        if query.with(String::is_empty) {
            total.to_string()
        } else {
            format!("{} of {total}", shown.with(Vec::len))
        }
    };

    view! {
        <div class="space-y-1.5">
            <div class="flex items-center gap-1.5 px-1">
                <span class=label_class(LabelSize::Small, LabelTone::Default)>"Screens"</span>
                <span class="font-mono text-[10px] tabular-nums text-fg-tertiary/70">{count}</span>
            </div>
            {move || {
                let rows = shown.get();
                if rows.is_empty() {
                    return view! {
                        <div class="rounded-lg border border-dashed border-edge-subtle/45 px-3 py-4 text-center text-[11px] text-fg-tertiary/55">
                            "No screens match"
                        </div>
                    }
                        .into_any();
                }
                rows.into_iter()
                    .map(|(surface, label)| view! { <ScreenCard surface=surface label=label /> })
                    .collect_view()
                    .into_any()
            }}
        </div>
    }
}

/// One Screen: a thumbnail of its face, its label, and its resolution.
/// The selected Screen's thumbnail rides the live preview stream the
/// Stage already subscribes to; the rest show the daemon's latest still,
/// refreshed on structural scene changes.
#[component]
fn ScreenCard(surface: Surface, label: String) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let ws = expect_context::<WsContext>();
    let displays = expect_context::<DisplaysContext>().displays_resource;
    let zone_id = StoredValue::new(surface.id.clone());
    let device_id = surface.display_device_id.clone();
    let is_selected = Memo::new(move |_| {
        studio
            .selected_surface_id
            .with(|selected| zone_id.with_value(|id| selected.as_deref() == Some(id.as_str())))
    });
    let display = Memo::new({
        let device_id = device_id.clone();
        move |_| {
            let device_id = device_id.as_ref()?;
            displays
                .get()
                .and_then(Result::ok)?
                .into_iter()
                .find(|display| display.id == *device_id)
        }
    });
    let generation = Memo::new(move |_| {
        ws.last_scene_event
            .with(|hint| hint.as_ref().map_or(0, |hint| hint.structural_generation))
    });
    let frame = Signal::derive({
        let device_id = device_id.clone();
        move || {
            let device_id = device_id.as_ref()?;
            if !is_selected.get() {
                return None;
            }
            ws.display_preview_frames
                .with(|frames| frames.get(device_id).cloned())
        }
    });
    let dimmed = !surface.enabled;
    let aria_label = format!("Face preview of {label}");

    view! {
        <button
            type="button"
            class="flex w-full items-center gap-3 rounded-xl border px-2.5 py-2 text-left transition-[border-color,background-color] duration-150"
            class=("border-accent-muted", move || is_selected.get())
            class=("bg-accent-subtle", move || is_selected.get())
            class=("border-edge-subtle/70", move || !is_selected.get())
            class=("bg-surface-overlay/40", move || !is_selected.get())
            class=("hover:bg-surface-hover/30", move || !is_selected.get())
            class=("opacity-55", move || dimmed)
            aria-pressed=move || is_selected.get().to_string()
            on:click=move |_| studio.selected_surface_id.set(Some(zone_id.get_value()))
        >
            <div class="w-16 shrink-0">
                {move || {
                    let Some(display) = display.get() else {
                        return view! {
                            <div class="flex aspect-square w-full items-center justify-center rounded-lg bg-surface-sunken/70 text-fg-tertiary/60">
                                <Icon icon=LuMonitor width="16px" height="16px" />
                            </div>
                        }
                            .into_any();
                    };
                    let shape = if display.circular { "rounded-full" } else { "rounded-lg" };
                    view! {
                        <DisplayPreviewSurface
                            frame=frame
                            fallback_route=api::display_preview_route(
                                &display.id,
                                Some(generation.get()),
                            )
                            aspect_ratio=format!("{} / {}", display.width.max(1), display.height.max(1))
                            aria_label=aria_label.clone()
                            container_class=format!("w-full overflow-hidden border border-edge-subtle/60 bg-black {shape}")
                        />
                    }
                        .into_any()
                }}
            </div>
            <span class="min-w-0 flex-1">
                <span class="block truncate text-[12px] font-medium text-fg-primary">{label.clone()}</span>
                <span class="block truncate font-mono text-[10px] tabular-nums text-fg-tertiary/70">
                    {move || {
                        display
                            .get()
                            .map(|display| format!("{} \u{d7} {}", display.width, display.height))
                            .unwrap_or_default()
                    }}
                </span>
            </span>
        </button>
    }
}
