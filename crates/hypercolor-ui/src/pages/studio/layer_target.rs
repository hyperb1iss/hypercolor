//! What a layer applies to, in Studio.
//!
//! A layer applies to its whole zone or to some of the zone's outputs.
//! This module is the Studio side of choosing that: the state every entry
//! point shares ([`LayerTargets`]), the editor that hangs off whatever
//! asked for it ([`TargetEditorHost`]: a popover, or a sheet on a phone),
//! and the banner that runs a pick on the canvas ([`PickBanner`]).
//!
//! Three places add a layer to chosen outputs and all go through
//! [`LayerTargets::add_layer_to`]: "+ Layer" in the output properties bar,
//! "Add layer to this device" on a device card, and the target line under
//! "Add layer" in the stack. A change to a layer already in the stack is
//! one step in Studio's history. Screens take faces through their own tab
//! and are never offered here.

use std::collections::HashSet;

use hypercolor_types::api::scene::ZoneResource;
use hypercolor_types::scene::ZoneRole;
use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::api::{self, DeviceSummary};
use crate::components::bottom_sheet::BottomSheet;
use crate::components::layer_panel::scope::{
    Target, TargetOutput, draft_words, is_screen_member, outputs_label, zone_target_outputs,
};
use crate::components::layer_panel::target_checklist::TargetChecklist;
use crate::components::layer_panel::{LayerTargetHost, TargetSubject};
use crate::icons::*;
use crate::layout_utils;

use super::StudioContext;
use super::zone_strip::menu_position;

/// The capability a daemon advertises once a layer can be limited to
/// outputs. Without it Studio shows no target anywhere.
pub(crate) const LAYER_SCOPE_CAPABILITY: &str = "layer-scope";

/// The viewport width from which the composition panel docks beside the
/// Stage instead of covering it: Tailwind's `xl`.
const PANEL_DOCKS_QUERY: &str = "(min-width: 80rem)";

/// An open target editor.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TargetEditor {
    pub zone_id: String,
    pub subject: TargetSubject,
    pub draft: Target,
    /// The box of the element that asked: left, top, bottom.
    pub anchor: (f64, f64, f64),
}

/// A target being picked on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TargetPick {
    pub zone_id: String,
    pub subject: TargetSubject,
    pub draft: Target,
    /// The composition panel covered the canvas and stepped aside for the
    /// pick; it comes back when the pick ends.
    pub reopen_panel: bool,
}

/// Everything Studio shares about layer targets.
#[derive(Clone, Copy)]
pub(crate) struct LayerTargets {
    /// Whether the daemon can limit a layer to outputs.
    pub enabled: Memo<bool>,
    /// The open zone's outputs, as targets name them. Empty on a Screen.
    pub outputs: Memo<Vec<TargetOutput>>,
    /// A target chosen for the next added layer, with the zone it was
    /// chosen in. It lasts until that layer is added or the selection
    /// changes. Unset, or another zone's, the next layer follows the
    /// canvas selection.
    pub chosen: RwSignal<Option<(String, Target)>>,
    /// What the next added layer applies to.
    pub add_target: Memo<Target>,
    /// Moves to ask the composition panel for its Add layer picker.
    pub add_requests: RwSignal<u64>,
    pub editor: RwSignal<Option<TargetEditor>>,
    pub pick: RwSignal<Option<TargetPick>>,
    /// Outputs a row or a checklist entry under the pointer names; the
    /// canvas outlines them.
    pub lit: RwSignal<HashSet<String>>,
    /// The selection's emphasis was set aside with "All". It comes back
    /// with the next selection.
    pub emphasis_off: RwSignal<bool>,
    /// The outputs whose layers stand in front: the selection, while the
    /// stack has a limited layer for it to tell apart.
    pub emphasis: Memo<HashSet<String>>,
    /// The element that opened the editor, where focus returns.
    asker: StoredValue<Option<web_sys::Element>, LocalStorage>,
}

/// A zone's outputs as targets name them: each by the name its device
/// card shows, under its device's name.
#[must_use]
pub(crate) fn target_outputs_for(
    zone: &ZoneResource,
    devices: &[DeviceSummary],
) -> Vec<TargetOutput> {
    let placed = api::zone_outputs(zone);
    zone_target_outputs(zone, |member| {
        let device_name = devices
            .iter()
            .find(|device| device.layout_device_id == member.device_id)
            .map_or_else(|| member.name.clone(), |device| device.name.clone());
        if is_screen_member(member) {
            return (member.name.clone(), device_name);
        }
        let name = placed
            .iter()
            .find(|output| output.id == member.id.0)
            .and_then(|output| layout_utils::rail_output_name(output, devices))
            .map_or_else(|| member.name.clone(), |rail| rail.name);
        (name, device_name)
    })
}

/// Whether the composition panel sits beside the Stage at this width.
fn panel_docks() -> bool {
    window()
        .match_media(PANEL_DOCKS_QUERY)
        .ok()
        .flatten()
        .is_some_and(|query| query.matches())
}

impl LayerTargets {
    pub fn new(
        active_scene: Signal<Option<api::SceneDocument>>,
        selected_surface_id: RwSignal<Option<String>>,
        selected_output_ids: RwSignal<HashSet<String>>,
    ) -> Self {
        let caps = expect_context::<crate::app::CapabilitiesContext>();
        let devices = expect_context::<crate::app::DevicesContext>().devices_resource;
        let enabled = Memo::new(move |_| caps.has(LAYER_SCOPE_CAPABILITY));
        let open_zone = move || {
            let zone_id = selected_surface_id.get()?;
            active_scene.with(|scene| {
                scene
                    .as_ref()?
                    .zones
                    .iter()
                    .find(|zone| zone.id.to_string() == zone_id && zone.role != ZoneRole::Display)
                    .cloned()
            })
        };
        let outputs = Memo::new(move |_| {
            let Some(zone) = open_zone() else {
                return Vec::new();
            };
            let devices = devices.get().and_then(Result::ok).unwrap_or_default();
            target_outputs_for(&zone, &devices)
        });
        let chosen = RwSignal::new(None::<(String, Target)>);
        let add_target = Memo::new(move |_| {
            let zone_id = selected_surface_id.get();
            let chosen = chosen.with(|chosen| {
                chosen
                    .as_ref()
                    .filter(|(zone, _)| Some(zone) == zone_id.as_ref())
                    .map(|(_, target)| target.clone())
            });
            chosen.unwrap_or_else(|| {
                outputs.with(|outputs| {
                    selected_output_ids.with(|selected| Target::from_selection(selected, outputs))
                })
            })
        });
        let emphasis_off = RwSignal::new(false);
        // A new selection is the newest word on what is being worked on:
        // it brings the emphasis back, and it replaces a target chosen for
        // the next layer before it, unless that target is the selection.
        Effect::new(move |previous: Option<HashSet<String>>| {
            let selected = selected_output_ids.get();
            if emphasis_off.get_untracked() {
                emphasis_off.set(false);
            }
            let moved_on = previous.is_some_and(|previous| previous != selected)
                && chosen.with_untracked(|chosen| {
                    chosen
                        .as_ref()
                        .is_some_and(|(_, target)| target.ids() != selected)
                });
            if moved_on {
                chosen.set(None);
            }
            selected
        });
        let emphasis = Memo::new(move |_| {
            if !enabled.get() || emphasis_off.get() {
                return HashSet::new();
            }
            let limited = open_zone()
                .is_some_and(|zone| zone.layers.iter().any(|layer| layer.scope.is_some()));
            if !limited {
                return HashSet::new();
            }
            outputs.with(|outputs| {
                selected_output_ids.with(|selected| {
                    outputs
                        .iter()
                        .filter(|output| selected.contains(&output.id))
                        .map(|output| output.id.clone())
                        .collect()
                })
            })
        });
        let targets = Self {
            enabled,
            outputs,
            chosen,
            add_target,
            add_requests: RwSignal::new(0),
            editor: RwSignal::new(None),
            pick: RwSignal::new(None),
            lit: RwSignal::new(HashSet::new()),
            emphasis_off,
            emphasis,
            asker: StoredValue::new_local(None),
        };
        // Another tab is another zone: an editor or a pick left open
        // belongs to the one just left.
        Effect::new(move |previous: Option<Option<String>>| {
            let current = selected_surface_id.get();
            if previous.is_some_and(|previous| previous != current) {
                targets.editor.set(None);
                targets.pick.set(None);
                targets.lit.set(HashSet::new());
            }
            current
        });
        targets
    }

    /// What Studio lends the layer panel, while the daemon can limit
    /// layers at all.
    pub fn host(self, studio: StudioContext) -> Signal<Option<LayerTargetHost>> {
        let host = LayerTargetHost {
            outputs: self.outputs.into(),
            emphasis: self.emphasis.into(),
            pointer: studio.pointer_output_id.into(),
            hover: Callback::new(move |outputs| self.light(outputs)),
            add_target: self.add_target.into(),
            edit: Callback::new(move |(subject, element)| {
                self.open_editor(studio, subject, &element);
            }),
            add_requests: self.add_requests.into(),
            added: Callback::new(move |target: Target| {
                self.chosen.set(None);
                // The outputs the new layer plays on become the selection,
                // so it arrives in front whichever way its target was
                // chosen. A whole-zone layer is in front of every selection.
                let outputs = target.ids();
                if !outputs.is_empty()
                    && studio
                        .selected_output_ids
                        .with_untracked(|selected| *selected != outputs)
                {
                    studio.selected_output_ids.set(outputs);
                }
            }),
            replaced: Callback::new(move |(zone_id, old, new): (String, _, _)| {
                studio.history.layer_replaced(&zone_id, old, new);
            }),
        };
        Signal::derive(move || self.enabled.get().then_some(host))
    }

    /// Outline `outputs` on the canvas; none to stop.
    pub fn light(self, outputs: HashSet<String>) {
        if self.lit.with_untracked(|lit| *lit != outputs) {
            self.lit.set(outputs);
        }
    }

    /// Open the composition panel's Add layer picker on a layer limited to
    /// `outputs` of the zone `zone_id`, opening that zone's tab first. The
    /// outputs become the canvas selection, so the stack shows what is on
    /// them and the new layer arrives in front.
    pub fn add_layer_to(self, studio: StudioContext, zone_id: &str, outputs: &[String]) {
        if outputs.is_empty() {
            return;
        }
        self.chosen.set(Some((
            zone_id.to_owned(),
            Target::Outputs(outputs.iter().cloned().collect()),
        )));
        let selection: HashSet<String> = outputs.iter().cloned().collect();
        if studio.selected_surface_id.get_untracked().as_deref() != Some(zone_id) {
            studio.selected_surface_id.set(Some(zone_id.to_owned()));
            // The tab switch clears the last zone's selection first.
            request_animation_frame(move || {
                studio.selected_output_ids.try_set(selection);
            });
        } else if studio
            .selected_output_ids
            .with_untracked(|selected| *selected != selection)
        {
            studio.selected_output_ids.set(selection);
        }
        studio.composition_open.set(true);
        self.add_requests
            .update(|asked| *asked = asked.wrapping_add(1));
    }

    /// Open the editor on `subject`, hung from `element`.
    pub fn open_editor(
        self,
        studio: StudioContext,
        subject: TargetSubject,
        element: &web_sys::Element,
    ) {
        let Some(zone_id) = studio.selected_surface_id.get_untracked() else {
            return;
        };
        let draft = match &subject {
            TargetSubject::NewLayer => self.add_target.get_untracked(),
            TargetSubject::Layer { id, .. } => {
                let scope = studio.history.current_scene().and_then(|scene| {
                    scene
                        .zones
                        .iter()
                        .find(|zone| zone.id.to_string() == zone_id)?
                        .layers
                        .iter()
                        .find(|layer| layer.id == *id)
                        .map(|layer| layer.scope.clone())
                });
                let Some(scope) = scope else {
                    return;
                };
                Target::of(scope.as_ref())
            }
        };
        let rect = element.get_bounding_client_rect();
        self.asker.set_value(Some(element.clone()));
        self.pick.set(None);
        self.editor.set(Some(TargetEditor {
            zone_id,
            subject,
            draft,
            anchor: (rect.left(), rect.top(), rect.bottom()),
        }));
    }

    /// Close the editor. `apply` keeps what it chose, when that names
    /// something; otherwise the draft is dropped.
    pub fn close_editor(self, studio: StudioContext, apply: bool) {
        let Some(editor) = self.editor.get_untracked() else {
            return;
        };
        self.editor.set(None);
        self.light(HashSet::new());
        if apply {
            self.apply(studio, editor.zone_id, editor.subject, editor.draft);
        }
        self.return_focus();
    }

    /// Hand focus back to whatever opened the editor, if it is still
    /// there: a changed layer's row is rebuilt, and its chip with it.
    fn return_focus(self) {
        let Some(asker) = self.asker.try_update_value(Option::take).flatten() else {
            return;
        };
        if asker.is_connected()
            && let Ok(element) = asker.dyn_into::<web_sys::HtmlElement>()
        {
            let _ = element.focus();
        }
    }

    /// Keep a chosen target: for the next added layer it waits for the
    /// add; for a layer in the stack it is one step of history.
    fn apply(self, studio: StudioContext, zone_id: String, subject: TargetSubject, draft: Target) {
        if !draft.is_complete() {
            return;
        }
        match subject {
            TargetSubject::NewLayer => self.chosen.set(Some((zone_id, draft))),
            TargetSubject::Layer { id, name } => {
                studio
                    .history
                    .set_layer_scope(zone_id, id, name, draft.scope());
            }
        }
    }

    /// Carry the editor's draft onto the canvas, where boxes toggle it. A
    /// panel that covers the canvas steps aside until the pick ends.
    pub fn begin_pick(self, studio: StudioContext) {
        let Some(editor) = self.editor.get_untracked() else {
            return;
        };
        self.editor.set(None);
        self.asker.set_value(None);
        self.light(HashSet::new());
        let reopen_panel = studio.composition_open.get_untracked() && !panel_docks();
        if reopen_panel {
            studio.composition_open.set(false);
        }
        // Boxes toggle outputs, so the pick starts from the outputs the
        // draft names; the whole zone starts it empty.
        let mut draft = editor.draft;
        self.outputs
            .with_untracked(|outputs| draft.drop_missing(outputs));
        if draft.is_whole_zone() {
            draft = Target::Outputs(std::collections::BTreeSet::new());
        }
        self.pick.set(Some(TargetPick {
            zone_id: editor.zone_id,
            subject: editor.subject,
            draft,
            reopen_panel,
        }));
    }

    /// End the pick, keeping it when `apply` and it names an output.
    pub fn end_pick(self, studio: StudioContext, apply: bool) {
        let Some(pick) = self.pick.get_untracked() else {
            return;
        };
        self.pick.set(None);
        if pick.reopen_panel {
            studio.composition_open.set(true);
        }
        if apply {
            self.apply(studio, pick.zone_id, pick.subject, pick.draft);
        }
    }
}

/// How the editor names its subject: "Ripples applies to".
fn subject_heading(subject: &TargetSubject) -> String {
    match subject {
        TargetSubject::NewLayer => "New layer applies to".to_owned(),
        TargetSubject::Layer { name, .. } => format!("{name} applies to"),
    }
}

/// The target editor: a popover hung from whatever asked for it, or a
/// bottom sheet on a phone. Mounted once per Studio page.
#[component]
pub(super) fn TargetEditorHost() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let targets = studio.targets;
    let phone = crate::viewport::is_phone_layout();
    let panel_ref = NodeRef::<html::Div>::new();
    // The rendered size, so the popover stays on screen whatever the root
    // font size makes of its width.
    let size = RwSignal::new((288.0_f64, 320.0_f64));
    let open = Memo::new(move |_| targets.editor.with(Option::is_some));
    let draft = Signal::derive(move || {
        targets
            .editor
            .with(|editor| editor.as_ref().map(|editor| editor.draft.clone()))
            .unwrap_or_default()
    });
    let complete = Memo::new(move |_| draft.with(Target::is_complete));
    let heading = Memo::new(move |_| {
        targets.editor.with(|editor| {
            editor
                .as_ref()
                .map(|editor| subject_heading(&editor.subject))
                .unwrap_or_default()
        })
    });
    let on_change = Callback::new(move |next: Target| {
        targets.editor.update(|editor| {
            if let Some(editor) = editor {
                editor.draft = next;
            }
        });
    });
    // The canvas outlines what the draft names, so a tick shows where it
    // lands; an entry under the pointer shows its own outputs instead, to
    // tell one fan from the next.
    let pointed = RwSignal::new(HashSet::<String>::new());
    let on_hover = Callback::new(move |outputs| pointed.set(outputs));
    Effect::new(move |_| {
        let Some(named) = targets
            .editor
            .with(|editor| editor.as_ref().map(|editor| editor.draft.ids()))
        else {
            if !pointed.with_untracked(HashSet::is_empty) {
                pointed.set(HashSet::new());
            }
            return;
        };
        let pointed = pointed.get();
        targets.light(if pointed.is_empty() { named } else { pointed });
    });

    Effect::new(move |_| {
        if open.get() && !phone.get_untracked() {
            request_animation_frame(move || {
                let Some(panel) = panel_ref.try_get_untracked().flatten() else {
                    return;
                };
                let rect = panel.get_bounding_client_rect();
                let _ = size.try_set((rect.width(), rect.height()));
                // Start on what is chosen now, so arrows move from there.
                let start = panel
                    .query_selector("[data-target-item][aria-checked=true]")
                    .ok()
                    .flatten()
                    .or_else(|| panel.query_selector("[data-target-item]").ok().flatten());
                if let Some(start) = start.and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = start.focus();
                }
            });
        }
    });

    // Escape drops the draft wherever focus is, in the popover or behind
    // it. A sheet closes itself on the key and reports back.
    let keydown = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape"
            && targets.editor.with_untracked(Option::is_some)
            && !phone.get_untracked()
        {
            ev.prevent_default();
            targets.close_editor(studio, false);
        }
    });
    on_cleanup(move || keydown.remove());

    let footer = move || {
        view! {
            <div class="flex items-center justify-between gap-2 border-t border-edge-subtle/60 px-2 py-2">
                <button
                    type="button"
                    class="inline-flex min-w-0 items-center gap-1.5 rounded-md px-2 py-1.5 text-[11px] font-medium text-fg-secondary transition-colors hover:bg-surface-hover/40 hover:text-fg-primary pointer-coarse:min-h-11"
                    title="Click boxes on the canvas to choose outputs"
                    on:click=move |_| targets.begin_pick(studio)
                >
                    <Icon icon=LuMousePointerClick width="12px" height="12px" />
                    <span class="truncate">"Pick on canvas"</span>
                </button>
                <button
                    type="button"
                    class="btn-press shrink-0 rounded-md border border-accent-muted bg-accent/10 px-3 py-1.5 text-[11px] font-semibold text-accent transition-colors hover:bg-accent/15 disabled:cursor-not-allowed disabled:opacity-45 pointer-coarse:min-h-11"
                    disabled=move || !complete.get()
                    title=move || (!complete.get()).then_some("Choose an output, or the whole zone")
                    on:click=move |_| targets.close_editor(studio, true)
                >
                    "Done"
                </button>
            </div>
        }
    };

    view! {
        <Show when=move || open.get()>
            {move || {
                if phone.get() {
                    view! {
                        <BottomSheet
                            on_close=Callback::new(move |()| targets.close_editor(studio, false))
                            title=heading.get_untracked()
                        >
                            <TargetChecklist
                                draft=draft
                                outputs=targets.outputs
                                on_change=on_change
                            />
                            {footer()}
                        </BottomSheet>
                    }
                    .into_any()
                } else {
                    view! {
                        <Portal>
                            // A click outside keeps the choice, as "Done"
                            // does; Escape is the way to drop it.
                            <div
                                class="fixed inset-0 z-[70]"
                                on:pointerdown=move |_| targets.close_editor(studio, true)
                                on:contextmenu=move |ev: web_sys::MouseEvent| {
                                    ev.prevent_default();
                                    targets.close_editor(studio, true);
                                }
                            />
                            <div
                                node_ref=panel_ref
                                role="dialog"
                                aria-label=move || heading.get()
                                class="fixed z-[71] flex w-72 max-w-[calc(100vw-1rem)] flex-col rounded-lg border border-edge-subtle bg-surface-overlay/98 backdrop-blur-xl dropdown-glow animate-enter-down"
                                style=move || {
                                    let (width, height) = size.get();
                                    targets
                                        .editor
                                        .with(|editor| {
                                            editor.as_ref().map(|editor| {
                                                let (left, top, bottom) = editor.anchor;
                                                // Below what asked, or above it when
                                                // only that side has the room.
                                                let below = bottom + 4.0;
                                                let viewport = window()
                                                    .inner_height()
                                                    .ok()
                                                    .and_then(|value| value.as_f64())
                                                    .unwrap_or(800.0);
                                                let y = if below + height > viewport - 8.0
                                                    && top - height - 4.0 >= 8.0
                                                {
                                                    top - height - 4.0
                                                } else {
                                                    below
                                                };
                                                menu_position(left, y, width, height)
                                            })
                                        })
                                        .unwrap_or_default()
                                }
                                on:keydown=move |ev: web_sys::KeyboardEvent| {
                                    match ev.key().as_str() {
                                        "ArrowDown" | "ArrowUp" | "Home" | "End" => {
                                            ev.prevent_default();
                                            move_focus(panel_ref, "[data-target-item]", &ev.key());
                                        }
                                        "Tab" => {
                                            // Focus stays in the popover: the page
                                            // behind it is out of reach until it closes.
                                            ev.prevent_default();
                                            let step = if ev.shift_key() { "ArrowUp" } else { "ArrowDown" };
                                            move_focus(panel_ref, "button:not([disabled])", step);
                                        }
                                        _ => {}
                                    }
                                }
                            >
                                <div class="px-3 pb-1 pt-2.5 text-[10px] font-semibold uppercase tracking-[0.14em] text-fg-tertiary">
                                    <span class="block truncate">{move || heading.get()}</span>
                                </div>
                                <div class="ambient-scroll max-h-[min(20rem,55vh)] overflow-y-auto p-1" style:--ambient-hue=crate::ambient::hue_style()>
                                    <TargetChecklist
                                        draft=draft
                                        outputs=targets.outputs
                                        on_change=on_change
                                        on_hover=on_hover
                                    />
                                </div>
                                {footer()}
                            </div>
                        </Portal>
                    }
                    .into_any()
                }
            }}
        </Show>
    }
}

/// Move focus among the elements of `panel` that match `selector`, as a
/// menu's arrow keys do.
fn move_focus(panel: NodeRef<html::Div>, selector: &str, key: &str) {
    let Some(panel) = panel.try_get_untracked().flatten() else {
        return;
    };
    let Ok(nodes) = panel.query_selector_all(selector) else {
        return;
    };
    let items: Vec<web_sys::HtmlElement> = (0..nodes.length())
        .filter_map(|index| nodes.get(index))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .collect();
    if items.is_empty() {
        return;
    }
    let active = document().active_element();
    let current = items.iter().position(|item| {
        let item: &web_sys::Element = item;
        active.as_ref() == Some(item)
    });
    let last = items.len() - 1;
    let next = match key {
        "ArrowDown" => current.map_or(0, |index| if index >= last { 0 } else { index + 1 }),
        "ArrowUp" => current.map_or(last, |index| if index == 0 { last } else { index - 1 }),
        "Home" => 0,
        _ => last,
    };
    let _ = items[next].focus();
}

/// The strip over the canvas while a target is picked on it: what is
/// being picked for, what is picked so far, and the way out.
#[component]
pub(super) fn PickBanner() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let targets = studio.targets;
    let picking = Memo::new(move |_| targets.pick.with(Option::is_some));
    let heading = Memo::new(move |_| {
        targets.pick.with(|pick| {
            pick.as_ref()
                .map_or_else(String::new, |pick| match &pick.subject {
                    TargetSubject::NewLayer => "Pick outputs for the new layer".to_owned(),
                    TargetSubject::Layer { name, .. } => format!("Pick outputs for {name}"),
                })
        })
    });
    let picked = Memo::new(move |_| {
        targets.pick.with(|pick| {
            pick.as_ref()
                .filter(|pick| pick.draft.is_complete())
                .map(|pick| {
                    targets
                        .outputs
                        .with(|outputs| draft_words(&pick.draft, outputs))
                })
        })
    });

    // Escape leaves the pick as it would any mode, unless a field has it.
    let keydown = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape"
            && targets.pick.with_untracked(Option::is_some)
            && !hypercolor_leptos_ext::events::target_is_text_entry(ev.target())
        {
            ev.prevent_default();
            targets.end_pick(studio, false);
        }
    });
    on_cleanup(move || keydown.remove());

    view! {
        <Show when=move || picking.get()>
            <div
                class="flex shrink-0 items-center gap-3 border-b border-accent-muted bg-accent-subtle px-5 py-2 animate-enter-fade max-md:px-3"
                role="status"
                data-studio-pick=""
            >
                <span class="flex shrink-0 text-accent">
                    <Icon icon=LuMousePointerClick width="15px" height="15px" />
                </span>
                <div class="min-w-0 flex-1">
                    <div class="truncate text-[12px] font-medium text-fg-primary">
                        {move || heading.get()}
                    </div>
                    <div class="truncate text-[11px] text-fg-tertiary">
                        {move || match picked.get() {
                            Some(words) => words,
                            None => "Click a box to add it, and again to take it out".to_owned(),
                        }}
                    </div>
                </div>
                <button
                    type="button"
                    class="btn-press shrink-0 rounded-md border border-edge-subtle px-2.5 py-1 text-[11px] font-medium text-fg-secondary transition-colors hover:bg-surface-hover/40 hover:text-fg-primary pointer-coarse:min-h-11"
                    on:click=move |_| targets.end_pick(studio, false)
                >
                    "Cancel"
                </button>
                <button
                    type="button"
                    class="btn-press shrink-0 rounded-md border border-accent-muted bg-accent/10 px-3 py-1 text-[11px] font-semibold text-accent transition-colors hover:bg-accent/15 disabled:cursor-not-allowed disabled:opacity-45 pointer-coarse:min-h-11"
                    disabled=move || picked.with(Option::is_none)
                    on:click=move |_| targets.end_pick(studio, true)
                >
                    "Done"
                </button>
            </div>
        </Show>
    }
}

/// The line the composition panel shows while a selection puts its layers
/// in front: "Showing: K70", with the way back to every layer.
#[component]
pub(super) fn ShowingStrip() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let targets = studio.targets;
    let showing = Memo::new(move |_| {
        targets.emphasis.with(|selected| {
            if selected.is_empty() {
                return None;
            }
            let ids: HashSet<&str> = selected.iter().map(String::as_str).collect();
            targets.outputs.with(|outputs| outputs_label(&ids, outputs))
        })
    });
    view! {
        <Show when=move || showing.with(Option::is_some)>
            <div
                class="flex shrink-0 items-center justify-between gap-3 border-b border-edge-subtle/60 bg-surface-sunken/40 py-1.5 pl-4 pr-3"
                data-studio-showing=""
            >
                <div class="flex min-w-0 items-baseline gap-1.5 text-[11px]">
                    <span class="shrink-0 text-fg-tertiary">"Showing"</span>
                    <span class="min-w-0 truncate font-medium text-fg-primary">
                        {move || showing.get().unwrap_or_default()}
                    </span>
                </div>
                <button
                    type="button"
                    class="btn-press shrink-0 rounded-md border border-edge-subtle px-2 py-0.5 text-[11px] font-medium text-fg-secondary transition-colors hover:border-accent-muted hover:text-fg-primary"
                    title="Bring every layer of this zone back to the front"
                    aria-label="Show every layer"
                    on:click=move |_| targets.emphasis_off.set(true)
                >
                    "All"
                </button>
            </div>
        </Show>
    }
}

/// "+ Layer" in the output properties bar: add a layer that applies to
/// the selected outputs alone. Renders nothing outside Studio, on a daemon
/// that cannot limit a layer, in a zone with nothing to choose between, or
/// for a selection of screens.
#[component]
pub(crate) fn OutputLayerButton(output_ids: Vec<String>) -> impl IntoView {
    let Some(studio) = use_context::<StudioContext>() else {
        return ().into_any();
    };
    let ids = StoredValue::new(output_ids);
    let offered = Memo::new(move |_| {
        studio
            .selected_surface_id
            .get()
            .is_some_and(|zone_id| ids.with_value(|ids| offers_layer(studio, &zone_id, ids)))
    });
    view! {
        <Show when=move || offered.get()>
            <button
                type="button"
                id="output-add-layer"
                class="btn-press mr-1 flex h-7 shrink-0 items-center gap-1 rounded-md border border-edge-subtle px-2 text-[11px] font-medium text-fg-secondary transition-colors hover:border-accent-muted hover:bg-surface-overlay hover:text-fg-primary"
                title="Add a layer that plays on this selection alone"
                aria-label="Add layer to the selection"
                on:click=move |_| {
                    let Some(zone_id) = studio.selected_surface_id.get_untracked() else {
                        return;
                    };
                    ids.with_value(|ids| add_layer_to_outputs(studio, &zone_id, ids));
                }
            >
                <span class="flex text-accent">
                    <Icon icon=LuPlus width="12px" height="12px" />
                </span>
                "Layer"
            </button>
        </Show>
    }
    .into_any()
}

/// Whether a layer can be limited to some of `outputs` in the zone
/// `zone_id`: the daemon limits layers, the zone has LED outputs to choose
/// between, and at least one of `outputs` is among them.
#[must_use]
pub(crate) fn offers_layer(studio: StudioContext, zone_id: &str, outputs: &[String]) -> bool {
    studio.targets.enabled.get()
        && studio
            .active_scene
            .with(|scene| !pickable_in(scene.as_ref(), zone_id, outputs).is_empty())
}

/// The ids of `outputs` a layer can be limited to in the zone `zone_id`
/// of `scene`: its LED outputs among them, and none at all when the zone
/// has nothing to choose between.
#[must_use]
pub(crate) fn pickable_in(
    scene: Option<&api::SceneDocument>,
    zone_id: &str,
    outputs: &[String],
) -> Vec<String> {
    let Some(zone) = scene.and_then(|scene| {
        scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id && zone.role != ZoneRole::Display)
    }) else {
        return Vec::new();
    };
    let lights: Vec<&str> = zone
        .members
        .iter()
        .filter(|member| !is_screen_member(member))
        .map(|member| member.id.0.as_str())
        .collect();
    if lights.len() < 2 {
        return Vec::new();
    }
    lights
        .into_iter()
        .filter(|id| outputs.iter().any(|output| output == id))
        .map(str::to_owned)
        .collect()
}

/// Add a layer limited to the pickable ones of `outputs` in `zone_id`.
pub(crate) fn add_layer_to_outputs(studio: StudioContext, zone_id: &str, outputs: &[String]) {
    let pickable = studio
        .active_scene
        .with_untracked(|scene| pickable_in(scene.as_ref(), zone_id, outputs));
    studio.targets.add_layer_to(studio, zone_id, &pickable);
}
