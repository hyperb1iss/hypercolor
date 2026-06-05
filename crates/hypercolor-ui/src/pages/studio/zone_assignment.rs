//! Device-output assignment for a multi-zone Studio scene (Spec 65 §9.3).
//!
//! The unit of assignment is a `Output` — one device output or
//! addressable segment, never a whole physical device. The panel lists
//! every output grouped by its owning zone (and, within a zone, by
//! physical device), lets the user multi-select outputs, and moves them
//! into a zone or out of one through the spec 64 device sub-routes. Every
//! mutation carries the active scene's `groups_revision` as the `If-Match`
//! precondition; a stale outcome reloads the scene.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_types::scene::{Zone, ZoneRole};
use hypercolor_types::spatial::Output;

use crate::api;
use crate::api::zones::ZoneOutcome;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::toasts;

use super::StudioContext;

/// One LED zone and the device outputs currently assigned to it.
#[derive(Clone, PartialEq)]
struct ZoneOutputs {
    id: String,
    name: String,
    color: String,
    outputs: Vec<Output>,
}

/// Display name for an LED zone in the assignment panel — the user's typed
/// name, or "Default zone" for an unnamed `Primary` group (§9.2).
fn zone_display_name(group: &Zone) -> String {
    let trimmed = group.name.trim();
    if group.role == ZoneRole::Primary
        && (trimmed.is_empty() || trimmed.eq_ignore_ascii_case("primary"))
    {
        "Default zone".to_owned()
    } else {
        group.name.clone()
    }
}

/// The §9.3 device-output assignment panel: a collapsible strip docked
/// below the Studio Stage canvas while the scene is multi-zone.
#[component]
pub fn ZoneAssignment() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let selected = RwSignal::new(HashSet::<String>::new());
    let collapsed = RwSignal::new(false);

    let zones = Memo::new(move |_| {
        let Some(scene) = studio.active_scene.get() else {
            return Vec::new();
        };
        scene
            .groups
            .iter()
            .filter(|group| group.role != ZoneRole::Display)
            .map(|group| ZoneOutputs {
                id: group.id.to_string(),
                name: zone_display_name(group),
                color: group
                    .color
                    .clone()
                    .unwrap_or_else(|| "rgba(128, 255, 234, 0.8)".to_owned()),
                outputs: group.layout.zones.clone(),
            })
            .collect::<Vec<_>>()
    });

    // Assign-target options for the toolbar `SilkSelect`.
    let zone_options = Memo::new(move |_| {
        zones
            .get()
            .into_iter()
            .map(|zone| (zone.id, zone.name))
            .collect::<Vec<_>>()
    });
    let selected_count = Memo::new(move |_| selected.get().len());

    let assign_to = Callback::new(move |zone_id: String| {
        let assignments = selected
            .get_untracked()
            .into_iter()
            .map(|id| api::zones::OutputAssignment::Existing { id })
            .collect::<Vec<_>>();
        let count = assignments.len();
        if count == 0 {
            return;
        }
        let Some(scene) = studio.active_scene.get_untracked() else {
            toasts::toast_error("No active scene is available");
            return;
        };
        spawn_local(async move {
            match api::zones::assign_devices(
                &scene.id,
                &zone_id,
                assignments,
                Some(scene.groups_revision),
            )
            .await
            {
                Ok(ZoneOutcome::Applied(_)) => {
                    toasts::toast_success(&format!("Assigned {count} output(s)"));
                    selected.set(HashSet::new());
                    studio.refresh_scene.run(());
                }
                Ok(ZoneOutcome::Stale { .. }) => {
                    toasts::toast_error("Scene changed elsewhere — reloaded, try again");
                    studio.refresh_scene.run(());
                }
                Err(error) => toasts::toast_error(&format!("Assignment failed: {error}")),
            }
        });
    });
    let on_clear = Callback::new(move |()| selected.set(HashSet::new()));

    view! {
        <div class="border-t border-edge-subtle/70 bg-surface-raised/40">
            <div class="flex items-center justify-between gap-3 px-4 py-2.5">
                <button
                    type="button"
                    class="-mx-1 flex items-center gap-1.5 rounded-md px-1 py-0.5 transition-colors hover:bg-surface-hover/30"
                    on:click=move |_| collapsed.update(|value| *value = !*value)
                >
                    {move || {
                        let icon = if collapsed.get() {
                            LuChevronRight
                        } else {
                            LuChevronDown
                        };
                        view! {
                            <Icon
                                icon=icon
                                width="13px"
                                height="13px"
                                style="color: rgba(139, 133, 160, 0.6)"
                            />
                        }
                    }}
                    <span class=label_class(LabelSize::Small, LabelTone::Strong)>
                        "Zone assignment"
                    </span>
                    <Show when=move || collapsed.get()>
                        <span class="text-[10px] text-fg-tertiary/55">
                            {move || {
                                let count = zones.get().len();
                                format!("{count} zone{}", if count == 1 { "" } else { "s" })
                            }}
                        </span>
                    </Show>
                </button>
                <Show when=move || !collapsed.get()>
                    <ZoneAssignmentToolbar
                        selected_count=selected_count
                        zone_options=zone_options
                        assign_to=assign_to
                        on_clear=on_clear
                    />
                </Show>
            </div>
            <Show when=move || !collapsed.get()>
                <div class="scrollbar-none max-h-52 overflow-y-auto px-4 pb-3">
                    {move || {
                        let zones = zones.get();
                        if zones.is_empty() {
                            view! {
                                <div class="rounded-lg border border-dashed border-edge-subtle/45 px-3 py-4 text-center text-[11px] text-fg-tertiary/55">
                                    "No zones in this scene"
                                </div>
                            }
                                .into_any()
                        } else {
                            zones
                                .into_iter()
                                .map(|zone| view! { <ZoneOutputSection zone=zone selected=selected /> })
                                .collect_view()
                                .into_any()
                        }
                    }}
                </div>
            </Show>
        </div>
    }
}

#[component]
fn ZoneAssignmentToolbar(
    #[prop(into)] selected_count: Signal<usize>,
    #[prop(into)] zone_options: Signal<Vec<(String, String)>>,
    assign_to: Callback<String>,
    on_clear: Callback<()>,
) -> impl IntoView {
    view! {
        {move || {
            let count = selected_count.get();
            if count == 0 {
                view! {
                    <span class="text-[11px] text-fg-tertiary/60">
                        "Select outputs to reassign"
                    </span>
                }
                    .into_any()
            } else {
                view! {
                    <div class="flex items-center gap-2">
                        <span class="text-[11px] font-medium text-fg-secondary">
                            {format!("{count} selected")}
                        </span>
                        <span class="text-[11px] text-fg-tertiary/60">"Assign to"</span>
                        <SilkSelect
                            value=Signal::derive(String::new)
                            options=zone_options
                            on_change=assign_to
                            placeholder="Pick a zone…".to_string()
                            class="border border-edge-subtle/70 bg-surface-overlay/40 px-2.5 py-1 text-[12px]"
                        />
                        <button
                            type="button"
                            class="chip-interactive inline-flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary hover:text-fg-secondary"
                            title="Clear selection"
                            on:click=move |_| on_clear.run(())
                        >
                            <Icon icon=LuX width="12px" height="12px" />
                        </button>
                    </div>
                }
                    .into_any()
            }
        }}
    }
}

#[component]
fn ZoneOutputSection(zone: ZoneOutputs, selected: RwSignal<HashSet<String>>) -> impl IntoView {
    let swatch = zone.color.clone();
    let outputs = zone.outputs.clone();
    view! {
        <div class="mt-2 first:mt-0">
            <div class="mb-1 flex items-center gap-1.5">
                <span
                    class="h-2.5 w-2.5 rounded-full border border-edge-subtle/70"
                    style:background-color=swatch
                />
                <span class="text-[11px] font-medium text-fg-secondary">{zone.name}</span>
                <span class="text-[10px] text-fg-tertiary/55">
                    {format!("{} output(s)", outputs.len())}
                </span>
            </div>
            {if outputs.is_empty() {
                view! {
                    <div class="rounded-md border border-dashed border-edge-subtle/40 px-2 py-1.5 text-[10px] text-fg-tertiary/50">
                        "No outputs assigned"
                    </div>
                }
                    .into_any()
            } else {
                view! {
                    <div class="flex flex-wrap gap-1.5">
                        {outputs
                            .into_iter()
                            .map(|output| {
                                view! { <OutputChip output=output selected=selected /> }
                            })
                            .collect_view()}
                    </div>
                }
                    .into_any()
            }}
        </div>
    }
}

#[component]
fn OutputChip(output: Output, selected: RwSignal<HashSet<String>>) -> impl IntoView {
    let output_id = output.id.clone();
    let toggle_id = output.id.clone();
    let is_selected = Signal::derive(move || selected.with(|set| set.contains(output_id.as_str())));
    // A multi-channel device names its segment; a single-zone device just
    // its own name. Both stay user-facing — never a raw id (§4).
    let label = match &output.zone_name {
        Some(channel) if !channel.trim().is_empty() => {
            format!("{} · {channel}", output.name)
        }
        _ => output.name.clone(),
    };
    view! {
        <button
            type="button"
            class="chip-interactive rounded-md border px-2 py-1 text-[11px] transition-colors"
            class=("border-accent-muted", move || is_selected.get())
            class=("bg-accent/12", move || is_selected.get())
            class=("text-fg-primary", move || is_selected.get())
            class=("border-edge-subtle/70", move || !is_selected.get())
            class=("text-fg-secondary", move || !is_selected.get())
            on:click=move |_| {
                selected
                    .update(|set| {
                        if !set.remove(&toggle_id) {
                            set.insert(toggle_id.clone());
                        }
                    });
            }
        >
            {label}
        </button>
    }
}
