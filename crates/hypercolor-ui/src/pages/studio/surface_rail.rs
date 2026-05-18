//! The Lights & Screens rail — the left rail of the Studio workspace.
//!
//! Every surface is one selectable row. Today the Lights section holds a
//! single row ("All Lights"); with multi-zone (Wave 9) it grows to one row
//! per zone plus an Unassigned entry — the rail fills in, it is not rebuilt.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;

use super::StudioContext;
use super::surface::{Surface, SurfaceKind, surfaces_from_groups};

/// The left rail. Reads the active scene from [`StudioContext`] and drives
/// the selected-surface state.
#[component]
pub fn SurfaceRail() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let surfaces = Memo::new(move |_| {
        studio
            .active_scene
            .get()
            .map(|scene| surfaces_from_groups(&scene.groups))
            .unwrap_or_default()
    });
    let lights = Memo::new(move |_| {
        surfaces
            .get()
            .into_iter()
            .filter(|surface| surface.kind == SurfaceKind::Light)
            .collect::<Vec<_>>()
    });
    let screens = Memo::new(move |_| {
        surfaces
            .get()
            .into_iter()
            .filter(|surface| surface.kind == SurfaceKind::Screen)
            .collect::<Vec<_>>()
    });

    view! {
        <div class="flex h-full flex-col border-r border-edge-subtle/70 bg-surface-raised/40">
            <div class="border-b border-edge-subtle/60 px-4 py-3">
                <span class=label_class(LabelSize::Section, LabelTone::Strong)>
                    "Lights & Screens"
                </span>
            </div>
            <div class="scrollbar-none flex-1 space-y-4 overflow-y-auto px-3 py-3">
                <SurfaceSection title="Lights" surfaces=lights kind=SurfaceKind::Light />
                <SurfaceSection title="Screens" surfaces=screens kind=SurfaceKind::Screen />
            </div>
        </div>
    }
}

#[component]
fn SurfaceSection(
    title: &'static str,
    #[prop(into)] surfaces: Signal<Vec<Surface>>,
    kind: SurfaceKind,
) -> impl IntoView {
    let empty_label = match kind {
        SurfaceKind::Light => "No lights in this scene",
        SurfaceKind::Screen => "No screens connected",
    };
    view! {
        <div class="space-y-1.5">
            <div class="px-1">
                <span class=label_class(LabelSize::Small, LabelTone::Default)>{title}</span>
            </div>
            {move || {
                let items = surfaces.get();
                if items.is_empty() {
                    view! {
                        <div class="rounded-lg border border-dashed border-edge-subtle/45 px-3 py-4 text-center text-[11px] text-fg-tertiary/55">
                            {empty_label}
                        </div>
                    }.into_any()
                } else {
                    items
                        .into_iter()
                        .map(|surface| view! { <SurfaceRow surface=surface /> })
                        .collect_view()
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn SurfaceRow(surface: Surface) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let row_id = surface.id.clone();
    let select_id = surface.id.clone();
    let is_selected =
        Signal::derive(move || studio.selected_surface_id.get().as_deref() == Some(row_id.as_str()));
    let icon = match surface.kind {
        SurfaceKind::Light => LuLightbulb,
        SurfaceKind::Screen => LuMonitor,
    };
    let dimmed = !surface.enabled;

    view! {
        <button
            type="button"
            class="card-hover flex w-full items-center gap-2.5 rounded-xl border px-3 py-2.5 text-left transition-all"
            class=("border-accent-muted", move || is_selected.get())
            class=("bg-accent/8", move || is_selected.get())
            class=("border-edge-subtle/70", move || !is_selected.get())
            class=("bg-surface-overlay/40", move || !is_selected.get())
            class=("opacity-55", move || dimmed)
            on:click=move |_| studio.selected_surface_id.set(Some(select_id.clone()))
        >
            <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-surface-sunken/70">
                <Icon
                    icon=icon
                    width="14px"
                    height="14px"
                    style="color: rgba(128, 255, 234, 0.8)"
                />
            </span>
            <span class="min-w-0 flex-1 truncate text-sm font-medium text-fg-primary">
                {surface.name}
            </span>
        </button>
    }
}
