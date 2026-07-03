//! The composition slide-over — Studio's on-demand layer editor.
//!
//! The two-column workspace keeps the Stage uncluttered: the layer stack
//! is not a permanent rail. This panel slides in over the Stage when the
//! now-playing chip is clicked, hosts the shared [`LayerPanel`], and
//! dismisses on a scrim click, the close button, or `Escape`.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::components::layer_panel::LayerPanel;
use crate::icons::*;

use super::StudioContext;
use super::surface::UNASSIGNED_SURFACE_ID;

/// The right-edge composition slide-over. Stays mounted and animates via
/// a transform; `inert` while closed keeps its controls out of the tab
/// order. Positioned `absolute` inside the Stage pane so it overlays the
/// Stage only, never the zone tree.
#[component]
pub fn CompositionPanel(
    #[prop(into)] active_scene: Signal<Option<api::ActiveSceneResponse>>,
    selected_group_id: ReadSignal<Option<String>>,
    set_selected_group_id: WriteSignal<Option<String>>,
    #[prop(into)] surface_label: Signal<Option<String>>,
    layers_resource: LocalResource<Result<api::LayerStackResponse, String>>,
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let open = studio.composition_open;
    // The synthetic Unassigned entry has no layer stack (§9.4). A `Memo`,
    // not a plain derived signal: the body below swaps the mounted panel
    // on this value, and a non-memoized derive re-runs that swap on every
    // selection change — disposing a `LayerPanel` whose resources are
    // still in flight, which panics the reactive runtime when the stale
    // Suspense closure re-polls.
    let is_unassigned =
        Memo::new(move |_| selected_group_id.get().as_deref() == Some(UNASSIGNED_SURFACE_ID));

    // Escape closes the panel while it is open.
    let _keydown = window_event_listener(ev::keydown, move |event| {
        if open.get_untracked() && event.key() == "Escape" {
            open.set(false);
        }
    });

    view! {
        <div
            class="absolute inset-0 z-30 bg-black/45 backdrop-blur-sm transition-opacity duration-200 ease-out"
            class=("opacity-0", move || !open.get())
            class=("pointer-events-none", move || !open.get())
            on:click=move |_| open.set(false)
        />
        <aside
            class="absolute inset-y-0 right-0 z-40 flex w-[420px] max-w-[88%] flex-col
                   border-l border-edge-subtle bg-surface-base/95 backdrop-blur-md
                   transition-transform duration-200 ease-out"
            class=("translate-x-full", move || !open.get())
            style="box-shadow: -20px 0 52px -28px rgba(0, 0, 0, 0.75)"
            inert=move || !open.get()
        >
            <div class="flex shrink-0 justify-end border-b border-edge-subtle/55 px-3 py-2">
                <button
                    type="button"
                    class="flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary transition-all btn-press hover:bg-surface-hover/40 hover:text-fg-primary"
                    title="Close the composition panel"
                    on:click=move |_| open.set(false)
                >
                    <Icon icon=LuX width="14px" height="14px" />
                </button>
            </div>
            <div class="scrollbar-none min-h-0 flex-1 overflow-y-auto px-4 pb-6">
                {move || {
                    if is_unassigned.get() {
                        view! { <UnassignedNote /> }.into_any()
                    } else {
                        view! {
                            <LayerPanel
                                active_scene=active_scene
                                selected_group_id=selected_group_id
                                set_selected_group_id=set_selected_group_id
                                surface_label=surface_label
                                layers_resource=layers_resource
                                on_layers_mutated=on_layers_mutated
                            />
                        }
                            .into_any()
                    }
                }}
            </div>
        </aside>
    }
}

/// Shown when the Unassigned entry is selected: unassigned lights belong
/// to no zone, so there is no layer stack to compose.
#[component]
fn UnassignedNote() -> impl IntoView {
    view! {
        <div class="pt-4">
            <div class="rounded-xl border border-dashed border-edge-subtle/55 bg-surface-overlay/30 px-4 py-6 text-center">
                <div class="mx-auto mb-3 flex h-9 w-9 items-center justify-center rounded-lg bg-surface-sunken/70">
                    <Icon
                        icon=LuBan
                        width="16px"
                        height="16px"
                        style="color: rgba(241, 250, 140, 0.75)"
                    />
                </div>
                <div class="text-sm font-medium text-fg-secondary">"No layer stack"</div>
                <div class="mt-1.5 text-[12px] leading-5 text-fg-tertiary/70">
                    "Unassigned lights belong to no zone, so there is nothing to
                     compose here. Assign their outputs to a zone in the Layout
                     view to give them a layer stack."
                </div>
            </div>
        </div>
    }
}
