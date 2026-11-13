//! The composition slide-over — Studio's on-demand layer editor.
//!
//! The two-column workspace keeps the Stage uncluttered: the layer stack
//! is not a permanent rail. This panel slides in over the Stage when the
//! now-playing chip is clicked, hosts the shared [`LayerPanel`], and
//! dismisses on a scrim click, the close button in its header, or
//! `Escape`.

use hypercolor_leptos_ext::events::target_is_text_entry;
use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::api;
use crate::components::layer_panel::LayerPanel;
use crate::components::zone_tabs::ZoneDot;
use crate::icons::*;

use super::StudioContext;
use super::layer_target::ShowingStrip;

pub(super) const COMPOSITION_PANEL_ID: &str = "studio-composition-panel";
pub(super) const COMPOSITION_TRIGGER_ID: &str = "studio-composition-trigger";

/// The right-edge composition slide-over. Stays mounted and animates via
/// a transform; `inert` while closed keeps its controls out of the tab
/// order. Positioned `absolute` inside the Stage pane so it overlays the
/// Stage only, never the device pane.
#[component]
pub fn CompositionPanel(
    #[prop(into)] active_scene: Signal<Option<api::SceneDocument>>,
    selected_zone_id: ReadSignal<Option<String>>,
    set_selected_zone_id: WriteSignal<Option<String>>,
    #[prop(into)] surface_label: Signal<Option<String>>,
    layers_resource: LocalResource<api::ApiResult<api::LayerStackResponse>>,
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let open = studio.composition_open;
    let panel_ref = NodeRef::<leptos::html::Aside>::new();
    let close_ref = NodeRef::<leptos::html::Button>::new();

    // Focus follows the disclosure without stealing it back from a control
    // elsewhere in Studio when the panel closes.
    Effect::new(move |previous: Option<bool>| {
        let is_open = open.get();
        if is_open && previous == Some(false) {
            request_animation_frame(move || {
                if open.try_get_untracked() == Some(true)
                    && let Some(button) = close_ref.try_get_untracked().flatten()
                {
                    let options = web_sys::FocusOptions::new();
                    options.set_prevent_scroll(true);
                    let _ = button.focus_with_options(&options);
                }
            });
        }
        is_open
    });
    let close = move || {
        let focused_in_panel = panel_ref.get_untracked().is_some_and(|panel| {
            document()
                .active_element()
                .is_some_and(|active| panel.contains(Some(&active)))
        });
        if focused_in_panel
            && let Some(trigger) = document()
                .get_element_by_id(COMPOSITION_TRIGGER_ID)
                .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = trigger.focus_with_options(&options);
        }
        open.set(false);
    };

    // The chip that opens the panel calls a selection of screens their
    // face settings and a zone's stack its layers; the header says the
    // same, and names the screens in place of the zone.
    let screen_title = Memo::new(move |_| {
        studio
            .screens
            .panel
            .with(|panel| panel.as_ref().map(|panel| panel.title.clone()))
    });
    let is_screen = Memo::new(move |_| screen_title.with(Option::is_some));

    // The zone's identity color, for the dot beside its name.
    let zone_color = Memo::new(move |_| {
        let selected = selected_zone_id.get()?;
        active_scene.with(|scene| {
            scene
                .as_ref()?
                .zones
                .iter()
                .find(|zone| zone.id.to_string() == selected)
                .and_then(|zone| zone.color.clone())
        })
    });
    let layer_targets = studio.targets.host(studio);
    let screen_host = studio.screens.host(studio);

    // Escape closes the panel while it is open, unless a text field owns
    // the key (a rename in the rail cancels with Escape too) or a target
    // is being chosen, in its editor or on the canvas, which ends first.
    // The handle is removed with the page so the listener never outlives
    // its signals.
    let keydown = window_event_listener(ev::keydown, move |event| {
        if open.get_untracked()
            && event.key() == "Escape"
            && !event.default_prevented()
            && !target_is_text_entry(event.target())
            && studio.targets.editor.with_untracked(Option::is_none)
            && studio.targets.pick.with_untracked(Option::is_none)
        {
            close();
        }
    });
    on_cleanup(move || keydown.remove());

    view! {
        <div
            // Wide layouts dock the panel beside the Stage, which stays
            // live and clickable; narrower ones overlay it, and the scrim
            // is the way out without dimming what is being tuned much.
            class="studio-composition-scrim absolute inset-0 z-30 bg-black/25 xl:hidden"
            class=("opacity-0", move || !open.get())
            class=("pointer-events-none", move || !open.get())
            on:click=move |_| close()
            aria-hidden="true"
        />
        <aside
            node_ref=panel_ref
            id=COMPOSITION_PANEL_ID
            aria-label="Composition"
            aria-hidden=move || (!open.get()).to_string()
            class="studio-composition-panel absolute inset-y-0 right-0 z-40 flex w-[420px] max-w-[88%] flex-col
                   border-l border-edge-subtle bg-surface-base"
            class=("translate-x-full", move || !open.get())
            inert=move || (!open.get()).then_some("")
        >
            // The header names the panel in the words of the chip that
            // opened it, then the zone or screen whose stack it holds, and
            // takes the Stage header's padding and line box so the two
            // rules meet when the panel docks beside it.
            <div class="flex shrink-0 items-center justify-between gap-3 border-b border-edge-subtle/60 py-3 pl-4 pr-3">
                <h2 class="flex min-w-0 items-center gap-2">
                    <span class="flex shrink-0 text-accent/75">
                        <Icon icon=LuLayers width="13px" height="13px" />
                    </span>
                    // The chip's own box, borders and all. The line height
                    // sits on the span because the global heading rule is
                    // unlayered and outranks a utility on the `h2` itself.
                    <span class="shrink-0 border border-transparent py-1.5 text-[12px] font-semibold leading-normal text-fg-primary">
                        {move || if is_screen.get() { "Face settings" } else { "Layers" }}
                    </span>
                    {move || screen_title.get().or_else(|| surface_label.get()).map(|name| view! {
                        <span class="flex min-w-0 items-center gap-1.5 border-l border-edge-subtle pl-2 text-[12px] font-normal leading-normal text-fg-tertiary">
                            <Show
                                when=move || !is_screen.get()
                                fallback=|| view! {
                                    <span class="flex shrink-0">
                                        <Icon icon=LuMonitor width="12px" height="12px" />
                                    </span>
                                }
                            >
                                <ZoneDot color=zone_color />
                            </Show>
                            <span class="truncate" data-composition-subject="">{name}</span>
                        </span>
                    })}
                </h2>
                <button
                    node_ref=close_ref
                    type="button"
                    class="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-fg-tertiary btn-press hover:bg-surface-hover/40 hover:text-fg-primary"
                    title="Close the composition panel"
                    on:click=move |_| close()
                >
                    <Icon icon=LuX width="14px" height="14px" />
                </button>
            </div>
            <ShowingStrip />
            <div class="scrollbar-none min-h-0 flex-1 overflow-y-auto px-4 pb-6">
                <LayerPanel
                    active_scene=active_scene
                    selected_zone_id=selected_zone_id
                    set_selected_zone_id=set_selected_zone_id
                    surface_label=surface_label
                    layers_resource=layers_resource
                    on_layers_mutated=on_layers_mutated
                    targets=layer_targets
                    screens=screen_host
                />
            </div>
        </aside>
    }
}
