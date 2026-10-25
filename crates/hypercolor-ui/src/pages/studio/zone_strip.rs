//! The Studio zone tab strip, in the page header's second row.
//!
//! One tab per LED zone in scene order, then a single Screens tab after a
//! divider whenever the scene has display surfaces, then "+" outside the
//! tablist. The active tab is Studio's selected zone. A tab renames in
//! place on double-click; right-click (or the kebab on the active tab, or
//! Shift+F10) opens its menu: Rename, Color, Pause or Resume, Move left,
//! Move right, Delete. Tabs reorder by dragging along the strip. When the
//! strip runs out of room, tabs shrink to a minimum width and then the
//! tail folds into a "N more" menu.

use std::collections::HashMap;

use hypercolor_leptos_ext::events::target_is_text_entry;
use leptos::ev;
use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::app::{CapabilitiesContext, WsContext};
use crate::components::device_pairing_modal::ModalBackdrop;
use crate::icons::*;
use crate::pointer_gesture::{
    GestureEnd, PointerEnd, PointerGesture, Press, capture_pointer, holds_capture, listener_element,
};
use crate::toasts;
use crate::ws::messages::zone_has_degraded_layer;

use super::StudioContext;
use super::surface::{Surface, SurfaceKind, surfaces_from_zones};
use super::tab_model::{
    TabStatus, ZONE_COLOR_PRESETS, deletable, delete_prompt, drag_target, drop_slot, fit_tab_count,
    next_tab_index, nudge_target, split_overflow, tab_status, tab_status_note, zone_device_count,
};
use super::undo_notice::UndoNotice;
use super::zone_controls::{
    commit_zone_color, commit_zone_enabled, commit_zone_move, commit_zone_rename, create_zone,
};

/// Element id of the panel every tab controls: the pane and Stage below.
pub const ZONE_PANEL_ID: &str = "studio-zone-panel";
/// Element id of the Screens tab.
pub const SCREENS_TAB_ID: &str = "zone-tab-screens";
/// The Screens tab's slot in keyboard order, after every zone tab.
const SCREENS_KEY: &str = "\u{0}screens";

/// The narrowest a zone tab shrinks before the tail overflows. The active
/// tab keeps more, since it also carries the kebab.
const TAB_MIN_PX: f64 = 116.0;
const ACTIVE_TAB_EXTRA_PX: f64 = 32.0;
const TAB_GAP_PX: f64 = 4.0;
/// Room held back for the overflow button, the Screens tab with its
/// divider, and "+", when each is present.
const MORE_BUTTON_PX: f64 = 84.0;
const SCREENS_TAB_PX: f64 = 124.0;
const ADD_BUTTON_PX: f64 = 36.0;
const MENU_ITEM_CLASS: &str = "dropdown-option flex w-full items-center gap-2.5 rounded-md px-2.5 \
    py-1.5 text-left text-[12px] text-fg-secondary transition-colors hover:text-fg-primary \
    disabled:cursor-not-allowed disabled:opacity-40";
const MENU_DANGER_CLASS: &str = "dropdown-option flex w-full items-center gap-2.5 rounded-md \
    px-2.5 py-1.5 text-left text-[12px] text-status-error transition-colors \
    disabled:cursor-not-allowed disabled:opacity-40";
/// Pointer travel before a press on a tab becomes a reorder drag.
const DRAG_THRESHOLD_PX: f64 = 5.0;

/// Element id of a zone's tab.
#[must_use]
pub fn zone_tab_dom_id(zone_id: &str) -> String {
    format!("zone-tab-{zone_id}")
}

/// The element id of the tab that owns the current selection.
#[must_use]
pub fn active_tab_dom_id(selected: Option<&str>, on_screens: bool) -> Option<String> {
    if on_screens {
        Some(SCREENS_TAB_ID.to_owned())
    } else {
        selected.map(zone_tab_dom_id)
    }
}

/// A zone tab's identity dot, name, and one optional status glyph. Shared
/// anatomy for every place a zone is picked by tab.
#[component]
pub fn ZoneTabLabel(
    #[prop(into)] name: Signal<String>,
    #[prop(into)] color: Signal<Option<String>>,
    #[prop(into)] status: Signal<TabStatus>,
) -> impl IntoView {
    view! {
        <span class="flex min-w-0 items-center gap-2">
            {move || match color.get() {
                Some(color) => view! {
                    <span
                        class="h-2 w-2 shrink-0 rounded-full"
                        style:background-color=color
                        aria-hidden="true"
                    />
                }
                    .into_any(),
                None => view! {
                    <span class="h-2 w-2 shrink-0 rounded-full bg-fg-tertiary/45" aria-hidden="true" />
                }
                    .into_any(),
            }}
            <span class="min-w-0 truncate">{move || name.get()}</span>
            {move || match status.get() {
                TabStatus::Clear => None,
                TabStatus::Warning => Some(
                    view! {
                        <span class="flex shrink-0 text-status-warning" aria-hidden="true">
                            <Icon icon=LuTriangleAlert width="11px" height="11px" />
                        </span>
                    }
                        .into_any(),
                ),
                TabStatus::Paused => Some(
                    view! {
                        <span class="flex shrink-0 text-fg-tertiary" aria-hidden="true">
                            <Icon icon=LuPause width="11px" height="11px" />
                        </span>
                    }
                        .into_any(),
                ),
            }}
        </span>
    }
}

/// An open tab menu: which zone, and where on screen.
#[derive(Clone, Debug, PartialEq)]
struct MenuAnchor {
    zone_id: String,
    x: f64,
    y: f64,
}

/// A press on a tab that may become a reorder drag.
#[derive(Clone, Debug)]
struct TabDrag {
    zone_id: String,
    start_x: f64,
    dragging: bool,
    /// Insertion slot among the other visible tabs, once dragging.
    slot: Option<usize>,
}

/// Shared state and handles the strip lends each tab.
#[derive(Clone, Copy)]
struct StripHandles {
    lights: Memo<Vec<Surface>>,
    light_ids: Memo<Vec<String>>,
    visible_ids: Memo<Vec<String>>,
    member_counts: Memo<HashMap<String, usize>>,
    scene_id: Memo<Option<String>>,
    menu: RwSignal<Option<MenuAnchor>>,
    tabs_ref: NodeRef<html::Div>,
    drag: StoredValue<PointerGesture<TabDrag>, LocalStorage>,
    dragging_id: RwSignal<Option<String>>,
    drop_x: RwSignal<Option<f64>>,
    swallow_click: StoredValue<bool>,
    request_delete: Callback<String>,
}

#[component]
pub fn ZoneStrip() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let caps = expect_context::<CapabilitiesContext>();
    let can_create = Memo::new(move |_| caps.zone_crud_ready());

    let surfaces = Memo::new(move |_| {
        studio.active_scene.with(|scene| {
            scene
                .as_ref()
                .map(|scene| surfaces_from_zones(&scene.zones))
                .unwrap_or_default()
        })
    });
    let lights = Memo::new(move |_| {
        surfaces
            .get()
            .into_iter()
            .filter(|surface| surface.kind == SurfaceKind::Light)
            .collect::<Vec<_>>()
    });
    let light_ids = Memo::new(move |_| {
        lights.with(|lights| {
            lights
                .iter()
                .map(|surface| surface.id.clone())
                .collect::<Vec<_>>()
        })
    });
    let screen_ids = Memo::new(move |_| {
        surfaces.with(|surfaces| {
            surfaces
                .iter()
                .filter(|surface| surface.kind == SurfaceKind::Screen)
                .map(|surface| surface.id.clone())
                .collect::<Vec<_>>()
        })
    });
    let member_counts = Memo::new(move |_| {
        studio.active_scene.with(|scene| {
            scene
                .as_ref()
                .map(|scene| {
                    scene
                        .zones
                        .iter()
                        .map(|zone| (zone.id.to_string(), zone.members.len()))
                        .collect::<HashMap<_, _>>()
                })
                .unwrap_or_default()
        })
    });
    let scene_id = Memo::new(move |_| {
        studio
            .active_scene
            .with(|scene| scene.as_ref().map(|scene| scene.id.to_string()))
    });
    let on_screens = Memo::new(move |_| {
        let selected = studio.selected_surface_id.get();
        screen_ids.with(|ids| selected.as_ref().is_some_and(|id| ids.contains(id)))
    });
    let active_light = Memo::new(move |_| {
        let selected = studio.selected_surface_id.get()?;
        light_ids.with(|ids| ids.iter().position(|id| *id == selected))
    });

    // The last Screen shown, so returning to the Screens tab reopens it.
    let last_screen = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        if on_screens.get() {
            last_screen.set_value(studio.selected_surface_id.get());
        }
    });
    let select_screens = move || {
        let ids = screen_ids.get_untracked();
        let target = last_screen
            .get_value()
            .filter(|id| ids.contains(id))
            .or_else(|| ids.first().cloned());
        if let Some(target) = target {
            studio.selected_surface_id.set(Some(target));
        }
    };

    let strip_ref = NodeRef::<html::Div>::new();
    let strip_width = RwSignal::new(f64::MAX);
    observe_width(strip_ref, strip_width);

    let layout = Memo::new(move |_| {
        let ids = light_ids.get();
        let mut reserved = ACTIVE_TAB_EXTRA_PX;
        if !screen_ids.with(Vec::is_empty) {
            reserved += SCREENS_TAB_PX;
        }
        if can_create.get() {
            reserved += ADD_BUTTON_PX;
        }
        let fit = fit_tab_count(
            strip_width.get() - reserved,
            ids.len(),
            TAB_MIN_PX,
            TAB_GAP_PX,
            MORE_BUTTON_PX,
        );
        let (shown, hidden) = split_overflow(ids.len(), active_light.get(), fit);
        let pick = |indices: Vec<usize>| {
            indices
                .into_iter()
                .filter_map(|index| ids.get(index).cloned())
                .collect::<Vec<_>>()
        };
        (pick(shown), pick(hidden))
    });
    let visible_ids = Memo::new(move |_| layout.with(|(shown, _)| shown.clone()));
    let hidden_ids = Memo::new(move |_| layout.with(|(_, hidden)| hidden.clone()));

    let menu = RwSignal::new(None::<MenuAnchor>);
    let overflow_menu = RwSignal::new(None::<(f64, f64)>);
    let confirm = RwSignal::new(None::<(String, String)>);
    let creating = RwSignal::new(false);
    let notice_seq = StoredValue::new(0_u64);

    // Created once here, in the strip's own scope. A callback minted at
    // delete time would belong to the menu or the confirmation, which are
    // gone by the time the delete completes.
    let on_deleted = Callback::new(move |record| {
        let id = notice_seq.get_value().wrapping_add(1);
        notice_seq.set_value(id);
        studio
            .undo_notice
            .set(Some(UndoNotice::zone_deleted(id, record)));
    });
    let run_delete = move |zone_id: String| {
        let ids = light_ids.get_untracked();
        let neighbor = ids.iter().position(|id| *id == zone_id).and_then(|index| {
            index
                .checked_sub(1)
                .and_then(|prev| ids.get(prev))
                .or_else(|| ids.get(index + 1))
                .cloned()
        });
        studio.history.delete_zone(zone_id, neighbor, on_deleted);
    };
    let request_delete = Callback::new(move |zone_id: String| {
        let Some(scene) = studio.active_scene.get_untracked() else {
            return;
        };
        if let Err(block) = deletable(&scene.zones, &zone_id) {
            toasts::toast_info(block.reason());
            return;
        }
        let Some(zone) = scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)
        else {
            return;
        };
        let name = lights
            .with_untracked(|lights| {
                lights
                    .iter()
                    .find(|surface| surface.id == zone_id)
                    .map(|surface| surface.name.clone())
            })
            .unwrap_or_else(|| zone.name.clone());
        match delete_prompt(&name, zone_device_count(zone)) {
            None => run_delete(zone_id),
            Some(prompt) => confirm.set(Some((zone_id, prompt))),
        }
    });

    let handles = StripHandles {
        lights,
        light_ids,
        visible_ids,
        member_counts,
        scene_id,
        menu,
        tabs_ref: NodeRef::new(),
        drag: StoredValue::new_local(PointerGesture::new()),
        dragging_id: RwSignal::new(None),
        drop_x: RwSignal::new(None),
        swallow_click: StoredValue::new(false),
        request_delete,
    };

    // Keyboard order: every zone tab (folded ones included), then Screens.
    let tab_order = move || {
        let mut order = light_ids.get_untracked();
        if !screen_ids.with_untracked(Vec::is_empty) {
            order.push(SCREENS_KEY.to_owned());
        }
        order
    };
    let step = move |key: &str, focus: bool| {
        let order = tab_order();
        let current = if on_screens.get_untracked() {
            order.iter().position(|id| id == SCREENS_KEY)
        } else {
            let selected = studio.selected_surface_id.get_untracked();
            order
                .iter()
                .position(|id| selected.as_deref() == Some(id.as_str()))
        };
        let Some(next) = next_tab_index(current, order.len(), key) else {
            return false;
        };
        let Some(target) = order.get(next).cloned() else {
            return false;
        };
        let dom_id = if target == SCREENS_KEY {
            select_screens();
            SCREENS_TAB_ID.to_owned()
        } else {
            let dom_id = zone_tab_dom_id(&target);
            studio.selected_surface_id.set(Some(target));
            dom_id
        };
        if focus {
            focus_soon(dom_id);
        }
        true
    };

    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        let Some(target) = ev
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        if target.get_attribute("role").as_deref() != Some("tab") {
            return;
        }
        let key = ev.key();
        let zone_id = target
            .id()
            .strip_prefix("zone-tab-")
            .filter(|_| target.id() != SCREENS_TAB_ID)
            .map(str::to_owned);
        if key == "ContextMenu" || (key == "F10" && ev.shift_key()) {
            if let Some(zone_id) = zone_id {
                ev.prevent_default();
                let rect = target.get_bounding_client_rect();
                menu.set(Some(MenuAnchor {
                    zone_id,
                    x: rect.left(),
                    y: rect.bottom() + 4.0,
                }));
            }
            return;
        }
        if key == "F2" {
            if let Some(zone_id) = zone_id {
                ev.prevent_default();
                start_rename(studio, lights, &zone_id);
            }
            return;
        }
        if ev.ctrl_key() || ev.meta_key() || ev.alt_key() {
            return;
        }
        if matches!(key.as_str(), "ArrowLeft" | "ArrowRight" | "Home" | "End") && step(&key, true) {
            ev.prevent_default();
        }
    };

    // `[` and `]` cycle tabs from anywhere on the page outside a text field.
    let brackets = window_event_listener(ev::keydown, move |ev| {
        if ev.default_prevented() || ev.ctrl_key() || ev.meta_key() || ev.alt_key() {
            return;
        }
        let key = ev.key();
        if key != "[" && key != "]" {
            return;
        }
        if target_is_text_entry(ev.target())
            || menu.get_untracked().is_some()
            || confirm.get_untracked().is_some()
        {
            return;
        }
        if step(&key, false) {
            ev.prevent_default();
        }
    });
    on_cleanup(move || brackets.remove());

    let has_screens = Memo::new(move |_| !screen_ids.with(Vec::is_empty));
    let screen_count = Memo::new(move |_| screen_ids.with(Vec::len));

    view! {
        <div
            node_ref=strip_ref
            class="flex h-full min-w-0 flex-1 items-end gap-1 self-stretch"
            on:keydown=on_keydown
        >
            <div
                node_ref=handles.tabs_ref
                role="tablist"
                aria-label="Zones"
                aria-owns=move || has_screens.get().then_some(SCREENS_TAB_ID)
                class="relative flex min-w-0 items-end gap-1"
            >
                <For each=move || visible_ids.get() key=|id: &String| id.clone() let:zone_id>
                    <ZoneTab zone_id=zone_id handles=handles />
                </For>
                {move || {
                    handles
                        .drop_x
                        .get()
                        .map(|x| {
                            view! {
                                <span
                                    class="pointer-events-none absolute bottom-1 top-1 w-0.5 rounded-full bg-accent"
                                    style:left=format!("{x:.1}px")
                                />
                            }
                        })
                }}
            </div>
            <Show when=move || !hidden_ids.with(Vec::is_empty)>
                <button
                    type="button"
                    class="mb-1 flex h-7 shrink-0 items-center gap-1 rounded-md px-2 text-[11px] font-medium text-fg-tertiary transition-colors hover:bg-surface-hover/40 hover:text-fg-primary"
                    aria-haspopup="menu"
                    title="More zones"
                    on:click=move |ev: web_sys::MouseEvent| {
                        if let Some(element) = listener_element(&ev) {
                            let rect = element.get_bounding_client_rect();
                            overflow_menu.set(Some((rect.left(), rect.bottom() + 4.0)));
                        }
                    }
                >
                    <Icon icon=LuChevronDown width="12px" height="12px" />
                    {move || format!("{} more", hidden_ids.with(Vec::len))}
                </button>
            </Show>
            <Show when=move || has_screens.get()>
                <span class="mx-1 mb-2.5 h-4 w-px shrink-0 bg-edge-subtle" aria-hidden="true" />
                <div
                    class="relative flex h-8 shrink-0 items-center rounded-t-lg transition-colors md:h-9"
                    class=("bg-surface-base", move || on_screens.get())
                    class=("text-fg-primary", move || on_screens.get())
                    class=("text-fg-tertiary", move || !on_screens.get())
                    class=("hover:bg-surface-hover/30", move || !on_screens.get())
                    class=("hover:text-fg-secondary", move || !on_screens.get())
                >
                    <Show when=move || on_screens.get()>
                        <span class="pointer-events-none absolute inset-x-2 top-0 h-0.5 rounded-full bg-accent" />
                    </Show>
                    <button
                        type="button"
                        role="tab"
                        id=SCREENS_TAB_ID
                        aria-selected=move || on_screens.get().to_string()
                        aria-controls=ZONE_PANEL_ID
                        tabindex=move || if on_screens.get() { "0" } else { "-1" }
                        class="flex h-full items-center gap-2 px-3 text-[12px] font-medium"
                        on:click=move |_| select_screens()
                    >
                        <Icon icon=LuMonitor width="12px" height="12px" />
                        "Screens"
                        <span class="font-mono text-[10px] tabular-nums text-fg-tertiary">
                            {move || screen_count.get()}
                        </span>
                    </button>
                </div>
            </Show>
            <Show when=move || can_create.get()>
                <button
                    type="button"
                    class="mb-1 ml-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-fg-tertiary transition-colors hover:bg-surface-hover/40 hover:text-accent disabled:opacity-40"
                    title="New zone"
                    aria-label="New zone"
                    disabled=move || creating.get()
                    on:click=move |_| {
                        if !creating.get_untracked() {
                            create_zone(studio, creating);
                        }
                    }
                >
                    <Icon icon=LuPlus width="14px" height="14px" />
                </button>
            </Show>
        </div>
        <ZoneTabMenu handles=handles />
        <OverflowMenu anchor=overflow_menu hidden_ids=hidden_ids handles=handles />
        <Show when=move || confirm.get().is_some()>
            <ModalBackdrop on_close=Callback::new(move |()| confirm.set(None)) label="Delete zone">
                <h2 class="mb-4 text-sm font-medium text-fg-primary">
                    {move || confirm.get().map(|(_, prompt)| prompt)}
                </h2>
                <div class="flex justify-end gap-2">
                    <button
                        type="button"
                        class="btn-press rounded-lg border border-edge-subtle px-3 py-2 text-xs text-fg-secondary"
                        on:click=move |_| confirm.set(None)
                    >
                        "Keep zone"
                    </button>
                    <button
                        type="button"
                        class="btn-press rounded-lg border border-status-error/30 bg-status-error/10 px-3 py-2 text-xs text-status-error"
                        on:click=move |_| {
                            if let Some((zone_id, _)) = confirm.get_untracked() {
                                confirm.set(None);
                                run_delete(zone_id);
                            }
                        }
                    >
                        "Delete zone"
                    </button>
                </div>
            </ModalBackdrop>
        </Show>
    }
}

/// One zone tab: the selectable tab itself, its in-place rename field, the
/// kebab on the active tab, and the drag that reorders it.
#[component]
fn ZoneTab(zone_id: String, handles: StripHandles) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let ws = use_context::<WsContext>();
    let id = StoredValue::new(zone_id.clone());
    let dom_id = zone_tab_dom_id(&zone_id);

    let surface = Memo::new(move |_| {
        handles.lights.with(|lights| {
            lights
                .iter()
                .find(|surface| id.with_value(|id| surface.id == *id))
                .cloned()
        })
    });
    let degraded = Memo::new(move |_| {
        let (Some(ws), Some(scene_id), Some(surface)) = (ws, handles.scene_id.get(), surface.get())
        else {
            return false;
        };
        ws.layer_health
            .with(|map| zone_has_degraded_layer(map, &scene_id, &surface.id, &surface.layer_ids))
    });
    let status = Memo::new(move |_| {
        let Some(surface) = surface.get() else {
            return TabStatus::Clear;
        };
        let members = handles
            .member_counts
            .with(|counts| id.with_value(|id| counts.get(id).copied().unwrap_or(0)));
        tab_status(surface.enabled, degraded.get(), members)
    });
    let note = Memo::new(move |_| tab_status_note(status.get(), degraded.get()));
    let name = Signal::derive(move || {
        surface
            .get()
            .map(|surface| surface.name)
            .unwrap_or_default()
    });
    let color = Signal::derive(move || surface.get().and_then(|surface| surface.color));

    let is_active = Memo::new(move |_| {
        studio
            .selected_surface_id
            .with(|selected| id.with_value(|id| selected.as_deref() == Some(id.as_str())))
    });
    let renaming = Memo::new(move |_| {
        studio.zone_rename_draft.with(|draft| {
            draft
                .as_ref()
                .is_some_and(|(draft_id, _)| id.with_value(|id| draft_id == id))
        })
    });
    let is_dragged = Memo::new(move |_| {
        handles
            .dragging_id
            .with(|dragging| id.with_value(|id| dragging.as_deref() == Some(id.as_str())))
    });

    let end_drag = move |pointer_id: i32, how: PointerEnd| {
        let ended = handles
            .drag
            .try_update_value(|gesture| gesture.end(pointer_id, how))
            .flatten();
        let Some((outcome, state)) = ended else {
            return;
        };
        handles.dragging_id.set(None);
        handles.drop_x.set(None);
        if !state.dragging || outcome != GestureEnd::Commit {
            return;
        }
        // The release's click lands on this tab next; it must not select.
        handles.swallow_click.set_value(true);
        let Some(slot) = state.slot else {
            return;
        };
        let visible = handles.visible_ids.get_untracked();
        let led_ids = handles.light_ids.get_untracked();
        if let Some(target) = drag_target(&visible, &state.zone_id, slot, &led_ids) {
            commit_zone_move(studio, &state.zone_id, &target);
        }
    };
    on_cleanup(move || {
        let _ = handles.drag.try_update_value(|gesture| {
            if gesture.current().is_some_and(|state| {
                id.try_with_value(|id| state.zone_id == *id)
                    .unwrap_or(false)
            }) {
                gesture.abandon();
                handles.dragging_id.try_set(None);
                handles.drop_x.try_set(None);
            }
        });
    });

    let tab_dom_id = dom_id.clone();
    let tab_view = move || {
        if renaming.get() {
            return view! { <TabRenameField zone_id=id.get_value() dom_id=tab_dom_id.clone() /> }
                .into_any();
        }
        view! {
            <button
                type="button"
                role="tab"
                id=tab_dom_id.clone()
                aria-selected=move || is_active.get().to_string()
                aria-controls=ZONE_PANEL_ID
                tabindex=move || if is_active.get() { "0" } else { "-1" }
                title=move || note.get()
                class="flex h-full min-w-0 flex-1 touch-none select-none items-center pl-3 pr-2 text-[12px] font-medium"
                on:click=move |_| {
                    if handles.swallow_click.get_value() {
                        handles.swallow_click.set_value(false);
                        return;
                    }
                    studio.selected_surface_id.set(Some(id.get_value()));
                }
                on:dblclick=move |_| start_rename(studio, handles.lights, &id.get_value())
                on:contextmenu=move |ev: web_sys::MouseEvent| {
                    ev.prevent_default();
                    handles.menu.set(Some(MenuAnchor {
                        zone_id: id.get_value(),
                        x: f64::from(ev.client_x()),
                        y: f64::from(ev.client_y()),
                    }));
                }
                on:pointerdown=move |ev: web_sys::PointerEvent| {
                    handles.swallow_click.set_value(false);
                    if ev.button() != 0 {
                        return;
                    }
                    let Some(element) = listener_element(&ev) else {
                        return;
                    };
                    let pointer_id = ev.pointer_id();
                    let press = handles
                        .drag
                        .try_update_value(|gesture| {
                            gesture.press(pointer_id, |owner, _| holds_capture(Some(&element), owner))
                        })
                        .unwrap_or(Press::Busy);
                    match press {
                        Press::Busy => return,
                        Press::Stale(_) => {
                            handles.dragging_id.set(None);
                            handles.drop_x.set(None);
                        }
                        Press::Ready => {}
                    }
                    capture_pointer(&element, &ev);
                    let state = TabDrag {
                        zone_id: id.get_value(),
                        start_x: f64::from(ev.client_x()),
                        dragging: false,
                        slot: None,
                    };
                    handles.drag.update_value(|gesture| gesture.start(pointer_id, ev.button(), state));
                }
                on:pointermove=move |ev: web_sys::PointerEvent| {
                    let pointer_id = ev.pointer_id();
                    if handles.drag.with_value(|gesture| gesture.press_released(pointer_id, ev.buttons())) {
                        end_drag(pointer_id, PointerEnd::Up);
                        return;
                    }
                    let Some(state) = handles.drag.with_value(|gesture| gesture.state(pointer_id).cloned()) else {
                        return;
                    };
                    let x = f64::from(ev.client_x());
                    if !state.dragging && (x - state.start_x).abs() < DRAG_THRESHOLD_PX {
                        return;
                    }
                    ev.prevent_default();
                    if !state.dragging {
                        handles.dragging_id.set(Some(state.zone_id.clone()));
                    }
                    let (slot, line) = measure_slot(handles.tabs_ref, &state.zone_id, x);
                    handles.drag.update_value(|gesture| {
                        if let Some(state) = gesture.state_mut(pointer_id) {
                            state.dragging = true;
                            state.slot = slot;
                        }
                    });
                    handles.drop_x.set(line);
                }
                on:pointerup=move |ev: web_sys::PointerEvent| end_drag(ev.pointer_id(), PointerEnd::Up)
                on:pointercancel=move |ev: web_sys::PointerEvent| end_drag(ev.pointer_id(), PointerEnd::Cancel)
                on:lostpointercapture=move |ev: web_sys::PointerEvent| end_drag(ev.pointer_id(), PointerEnd::LostCapture)
            >
                <ZoneTabLabel name=name color=color status=status />
            </button>
        }
            .into_any()
    };

    view! {
        <div
            role="presentation"
            data-zone-tab=zone_id
            class="relative flex h-8 max-w-[220px] shrink items-center rounded-t-lg transition-[background-color,color,opacity] duration-150 md:h-9"
            class=("min-w-[148px]", move || is_active.get())
            class=("min-w-[116px]", move || !is_active.get())
            class=("bg-surface-base", move || is_active.get())
            class=("text-fg-primary", move || is_active.get())
            class=("text-fg-tertiary", move || !is_active.get())
            class=("hover:bg-surface-hover/30", move || !is_active.get())
            class=("hover:text-fg-secondary", move || !is_active.get())
            class=("opacity-50", move || is_dragged.get())
        >
            <Show when=move || is_active.get()>
                <span class="pointer-events-none absolute inset-x-2 top-0 h-0.5 rounded-full bg-accent" />
            </Show>
            {tab_view}
            <Show when=move || is_active.get() && !renaming.get()>
                <button
                    type="button"
                    tabindex="-1"
                    class="mr-1 flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-fg-tertiary transition-colors hover:bg-surface-hover/50 hover:text-fg-primary"
                    title="Zone options"
                    aria-label="Zone options"
                    aria-haspopup="menu"
                    on:click=move |ev: web_sys::MouseEvent| {
                        if let Some(element) = listener_element(&ev) {
                            let rect = element.get_bounding_client_rect();
                            handles.menu.set(Some(MenuAnchor {
                                zone_id: id.get_value(),
                                x: rect.left(),
                                y: rect.bottom() + 4.0,
                            }));
                        }
                    }
                >
                    <Icon icon=LuEllipsis width="13px" height="13px" />
                </button>
            </Show>
        </div>
    }
}

/// The in-place rename field that stands in for a tab while renaming.
/// Enter or blur commits, Escape discards; focus returns to the tab.
#[component]
fn TabRenameField(zone_id: String, dom_id: String) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let input_ref = NodeRef::<html::Input>::new();
    let seed = studio
        .zone_rename_draft
        .with_untracked(|draft| draft.as_ref().map(|(_, text)| text.clone()))
        .unwrap_or_default();
    // Select the whole name a frame after mount: selecting during the
    // mount's own tick loses the selection to the value write.
    Effect::new(move |_| {
        if input_ref.get().is_some() {
            request_animation_frame(move || {
                if let Some(input) = input_ref.get_untracked() {
                    let _ = input.focus();
                    input.select();
                }
            });
        }
    });
    let zone_id = StoredValue::new(zone_id);
    let dom_id = StoredValue::new(dom_id);
    let open = move || {
        studio.zone_rename_draft.with_untracked(|draft| {
            draft
                .as_ref()
                .is_some_and(|(id, _)| zone_id.with_value(|zone_id| id == zone_id))
        })
    };
    let finish = move |commit: Option<String>| {
        studio.zone_rename_draft.set(None);
        if let Some(name) = commit {
            commit_zone_rename(studio, &zone_id.get_value(), &name);
        }
        focus_soon(dom_id.get_value());
    };
    view! {
        <input
            node_ref=input_ref
            class="h-full min-w-0 flex-1 border-b border-accent-muted bg-transparent px-3 text-[12px] font-medium text-fg-primary outline-none"
            aria-label="Zone name"
            prop:value=seed
            on:input=move |ev| {
                let value = event_target_value(&ev);
                studio.zone_rename_draft.set(Some((zone_id.get_value(), value)));
            }
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                match ev.key().as_str() {
                    "Enter" => {
                        ev.prevent_default();
                        finish(Some(event_target_value(&ev)));
                    }
                    "Escape" => {
                        ev.prevent_default();
                        finish(None);
                    }
                    _ => {}
                }
            }
            // Closing the field unmounts the input, and Chromium fires blur
            // on a removed element. Only commit while the draft is open, so
            // Enter cannot double-submit and Escape really discards.
            on:blur=move |ev| {
                if open() {
                    finish(Some(event_target_value(&ev)));
                }
            }
        />
    }
}

/// The per-tab menu, portaled so the Stage's slide-over cannot cover it.
#[component]
fn ZoneTabMenu(handles: StripHandles) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let menu = handles.menu;
    let colors_open = RwSignal::new(false);
    let menu_ref = NodeRef::<html::Div>::new();

    Effect::new(move |_| {
        if menu.get().is_some() {
            colors_open.set(false);
            request_animation_frame(move || focus_first_item(menu_ref));
        }
    });

    let zone = Memo::new(move |_| {
        let anchor = menu.get()?;
        handles.lights.with(|lights| {
            lights
                .iter()
                .find(|surface| surface.id == anchor.zone_id)
                .cloned()
        })
    });
    let neighbors = Memo::new(move |_| {
        let Some(anchor) = menu.get() else {
            return (None, None);
        };
        let ids = handles.light_ids.get();
        (
            nudge_target(&ids, &anchor.zone_id, -1),
            nudge_target(&ids, &anchor.zone_id, 1),
        )
    });
    let delete_block = Memo::new(move |_| {
        let anchor = menu.get()?;
        studio.active_scene.with(|scene| {
            scene
                .as_ref()
                .and_then(|scene| deletable(&scene.zones, &anchor.zone_id).err())
        })
    });

    let close = move || {
        if let Some(anchor) = menu.get_untracked() {
            menu.set(None);
            focus_soon(zone_tab_dom_id(&anchor.zone_id));
        }
    };
    let act = move |action: Box<dyn FnOnce(String)>| {
        if let Some(anchor) = menu.get_untracked() {
            menu.set(None);
            action(anchor.zone_id);
        }
    };

    let item = MENU_ITEM_CLASS;

    view! {
        <Show when=move || menu.get().is_some()>
            <Portal>
                <div
                    class="fixed inset-0 z-[70]"
                    on:pointerdown=move |_| close()
                    on:contextmenu=move |ev: web_sys::MouseEvent| {
                        ev.prevent_default();
                        close();
                    }
                />
                <div
                    node_ref=menu_ref
                    role="menu"
                    aria-label="Zone options"
                    class="fixed z-[71] w-52 overflow-hidden rounded-lg border border-edge-subtle bg-surface-overlay/98 p-1 backdrop-blur-xl dropdown-glow animate-enter-down"
                    style=move || {
                        menu.get()
                            .map(|anchor| menu_position(anchor.x, anchor.y, 208.0, 280.0))
                            .unwrap_or_default()
                    }
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        match ev.key().as_str() {
                            "Escape" | "Tab" => {
                                ev.prevent_default();
                                close();
                            }
                            "ArrowDown" | "ArrowUp" | "Home" | "End" => {
                                ev.prevent_default();
                                move_item_focus(menu_ref, &ev.key());
                            }
                            _ => {}
                        }
                    }
                >
                    <button
                        type="button"
                        role="menuitem"
                        class=item
                        on:click=move |_| {
                            act(Box::new(move |zone_id| start_rename(studio, handles.lights, &zone_id)));
                        }
                    >
                        <Icon icon=LuPencil width="13px" height="13px" />
                        "Rename"
                    </button>
                    <button
                        type="button"
                        role="menuitem"
                        class=item
                        aria-expanded=move || colors_open.get().to_string()
                        on:click=move |_| colors_open.update(|open| *open = !*open)
                    >
                        <Icon icon=LuPalette width="13px" height="13px" />
                        <span class="flex-1">"Color"</span>
                        {move || {
                            zone.get()
                                .and_then(|zone| zone.color)
                                .map(|color| {
                                    view! {
                                        <span
                                            class="h-2.5 w-2.5 rounded-full"
                                            style:background-color=color
                                            aria-hidden="true"
                                        />
                                    }
                                })
                        }}
                    </button>
                    <Show when=move || colors_open.get()>
                        <ColorSwatches
                            current=Signal::derive(move || zone.get().and_then(|zone| zone.color))
                            on_pick=Callback::new(move |color: Option<String>| {
                                act(Box::new(move |zone_id| commit_zone_color(studio, &zone_id, color)));
                            })
                        />
                    </Show>
                    <button
                        type="button"
                        role="menuitem"
                        class=item
                        on:click=move |_| {
                            let enabled = zone.get_untracked().is_some_and(|zone| zone.enabled);
                            act(Box::new(move |zone_id| commit_zone_enabled(studio, &zone_id, !enabled)));
                        }
                    >
                        {move || {
                            if zone.get().is_some_and(|zone| zone.enabled) {
                                view! {
                                    <Icon icon=LuPause width="13px" height="13px" />
                                    "Pause zone"
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <Icon icon=LuPlay width="13px" height="13px" />
                                    "Resume zone"
                                }
                                    .into_any()
                            }
                        }}
                    </button>
                    <button
                        type="button"
                        role="menuitem"
                        class=item
                        disabled=move || neighbors.get().0.is_none()
                        on:click=move |_| {
                            if let Some(target) = neighbors.get_untracked().0 {
                                act(Box::new(move |zone_id| commit_zone_move(studio, &zone_id, &target)));
                            }
                        }
                    >
                        <Icon icon=LuChevronLeft width="13px" height="13px" />
                        "Move left"
                    </button>
                    <button
                        type="button"
                        role="menuitem"
                        class=item
                        disabled=move || neighbors.get().1.is_none()
                        on:click=move |_| {
                            if let Some(target) = neighbors.get_untracked().1 {
                                act(Box::new(move |zone_id| commit_zone_move(studio, &zone_id, &target)));
                            }
                        }
                    >
                        <Icon icon=LuChevronRight width="13px" height="13px" />
                        "Move right"
                    </button>
                    <div class="my-1 h-px bg-edge-subtle/60" role="separator" />
                    <button
                        type="button"
                        role="menuitem"
                        class=MENU_DANGER_CLASS
                        disabled=move || delete_block.get().is_some()
                        title=move || delete_block.get().map(|block| block.reason())
                        on:click=move |_| {
                            act(Box::new(move |zone_id| handles.request_delete.run(zone_id)));
                        }
                    >
                        <Icon icon=LuTrash2 width="13px" height="13px" />
                        "Delete zone"
                    </button>
                </div>
            </Portal>
        </Show>
    }
}

/// The Color item's swatches: the identity presets, a custom pick, and
/// "None" to clear.
#[component]
fn ColorSwatches(
    current: Signal<Option<String>>,
    on_pick: Callback<Option<String>>,
) -> impl IntoView {
    view! {
        <div class="grid grid-cols-5 gap-1.5 px-2.5 pb-2 pt-1">
            {ZONE_COLOR_PRESETS
                .iter()
                .map(|(label, hex)| {
                    let hex = (*hex).to_owned();
                    let probe = hex.clone();
                    let pick = hex.clone();
                    view! {
                        <button
                            type="button"
                            role="menuitemradio"
                            class="h-6 w-6 rounded-full border border-edge-subtle/70 ring-accent ring-offset-1 ring-offset-surface-overlay transition-transform hover:scale-110"
                            class=("ring-2", move || current.get().is_some_and(|c| c.eq_ignore_ascii_case(&probe)))
                            style:background-color=hex
                            title=*label
                            aria-label=*label
                            on:click=move |_| on_pick.run(Some(pick.clone()))
                        />
                    }
                })
                .collect_view()}
            <label
                class="relative flex h-6 w-6 cursor-pointer items-center justify-center rounded-full border border-dashed border-edge-default text-fg-tertiary hover:text-fg-primary"
                title="Custom color"
            >
                <Icon icon=LuPlus width="11px" height="11px" />
                <input
                    type="color"
                    class="absolute inset-0 cursor-pointer opacity-0"
                    aria-label="Custom color"
                    prop:value=move || current.get().unwrap_or_else(|| "#e135ff".to_owned())
                    on:change=move |ev| on_pick.run(Some(event_target_value(&ev)))
                />
            </label>
            <button
                type="button"
                role="menuitem"
                class="col-span-4 flex h-6 items-center rounded-md px-1.5 text-[11px] text-fg-tertiary transition-colors hover:text-fg-primary"
                on:click=move |_| on_pick.run(None)
            >
                "No color"
            </button>
        </div>
    }
}

/// The "N more" menu of tabs folded off the end of the strip.
#[component]
fn OverflowMenu(
    anchor: RwSignal<Option<(f64, f64)>>,
    hidden_ids: Memo<Vec<String>>,
    handles: StripHandles,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let menu_ref = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if anchor.get().is_some() {
            request_animation_frame(move || focus_first_item(menu_ref));
        }
    });
    let rows = Memo::new(move |_| {
        let hidden = hidden_ids.get();
        handles.lights.with(|lights| {
            hidden
                .iter()
                .filter_map(|id| lights.iter().find(|surface| surface.id == *id).cloned())
                .collect::<Vec<_>>()
        })
    });
    view! {
        <Show when=move || anchor.get().is_some()>
            <Portal>
                <div class="fixed inset-0 z-[70]" on:pointerdown=move |_| anchor.set(None) />
                <div
                    node_ref=menu_ref
                    role="menu"
                    aria-label="More zones"
                    class="fixed z-[71] max-h-80 w-56 overflow-y-auto rounded-lg border border-edge-subtle bg-surface-overlay/98 p-1 backdrop-blur-xl dropdown-glow animate-enter-down"
                    style=move || {
                        anchor
                            .get()
                            .map(|(x, y)| menu_position(x, y, 224.0, 320.0))
                            .unwrap_or_default()
                    }
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        match ev.key().as_str() {
                            "Escape" | "Tab" => {
                                ev.prevent_default();
                                anchor.set(None);
                            }
                            "ArrowDown" | "ArrowUp" | "Home" | "End" => {
                                ev.prevent_default();
                                move_item_focus(menu_ref, &ev.key());
                            }
                            _ => {}
                        }
                    }
                >
                    {move || {
                        rows.get()
                            .into_iter()
                            .map(|surface| {
                                let id = surface.id.clone();
                                view! {
                                    <button
                                        type="button"
                                        role="menuitem"
                                        class="dropdown-option flex w-full items-center rounded-md px-2.5 py-1.5 text-left text-[12px] text-fg-secondary transition-colors hover:text-fg-primary"
                                        on:click=move |_| {
                                            anchor.set(None);
                                            let dom_id = zone_tab_dom_id(&id);
                                            studio.selected_surface_id.set(Some(id.clone()));
                                            focus_soon(dom_id);
                                        }
                                    >
                                        <ZoneTabLabel
                                            name=Signal::stored(surface.name.clone())
                                            color=Signal::stored(surface.color.clone())
                                            status=Signal::stored(if surface.enabled { TabStatus::Clear } else { TabStatus::Paused })
                                        />
                                    </button>
                                }
                            })
                            .collect_view()
                    }}
                </div>
            </Portal>
        </Show>
    }
}

/// Put a zone's tab into rename mode, seeded with its shown name.
fn start_rename(studio: StudioContext, lights: Memo<Vec<Surface>>, zone_id: &str) {
    let name = lights.with_untracked(|lights| {
        lights
            .iter()
            .find(|surface| surface.id == zone_id)
            .map(|surface| surface.name.clone())
    });
    if let Some(name) = name {
        studio
            .zone_rename_draft
            .set(Some((zone_id.to_owned(), name)));
    }
}

/// Where a drag at `x` would drop among the visible tabs other than
/// `dragged`: the insertion slot, and the drop line's offset inside the
/// tablist.
fn measure_slot(
    tabs_ref: NodeRef<html::Div>,
    dragged: &str,
    x: f64,
) -> (Option<usize>, Option<f64>) {
    let Some(container) = tabs_ref.get_untracked() else {
        return (None, None);
    };
    let origin = container.get_bounding_client_rect().left();
    let Ok(nodes) = container.query_selector_all("[data-zone-tab]") else {
        return (None, None);
    };
    let mut others = Vec::new();
    for index in 0..nodes.length() {
        let Some(element) = nodes
            .get(index)
            .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        if element.get_attribute("data-zone-tab").as_deref() == Some(dragged) {
            continue;
        }
        let rect = element.get_bounding_client_rect();
        others.push((rect.left(), rect.right()));
    }
    if others.is_empty() {
        return (None, None);
    }
    let centers: Vec<f64> = others
        .iter()
        .map(|(left, right)| f64::midpoint(*left, *right))
        .collect();
    let slot = drop_slot(x, &centers);
    let line = others
        .get(slot)
        .map(|(left, _)| left - TAB_GAP_PX / 2.0 - 1.0)
        .or_else(|| {
            others
                .last()
                .map(|(_, right)| right + TAB_GAP_PX / 2.0 - 1.0)
        })
        .map(|line| line - origin);
    (Some(slot), line)
}

/// Inline position for a portaled menu opened at `(x, y)`, kept on screen.
fn menu_position(x: f64, y: f64, width: f64, height: f64) -> String {
    let window = window();
    let viewport = |value: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>, fallback: f64| {
        value
            .ok()
            .and_then(|value| value.as_f64())
            .unwrap_or(fallback)
    };
    let view_width = viewport(window.inner_width(), 1280.0);
    let view_height = viewport(window.inner_height(), 800.0);
    let left = x.min(view_width - width - 8.0).max(8.0);
    let top = y.min(view_height - height - 8.0).max(8.0);
    format!("left: {left:.0}px; top: {top:.0}px")
}

fn menu_items(menu_ref: NodeRef<html::Div>) -> Vec<web_sys::HtmlElement> {
    let Some(menu) = menu_ref.get_untracked() else {
        return Vec::new();
    };
    let Ok(nodes) = menu.query_selector_all(
        "[role=menuitem]:not([disabled]), [role=menuitemradio]:not([disabled])",
    ) else {
        return Vec::new();
    };
    (0..nodes.length())
        .filter_map(|index| nodes.get(index))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .collect()
}

fn focus_first_item(menu_ref: NodeRef<html::Div>) {
    if let Some(first) = menu_items(menu_ref).first() {
        let _ = first.focus();
    }
}

fn move_item_focus(menu_ref: NodeRef<html::Div>, key: &str) {
    let items = menu_items(menu_ref);
    let active = document().active_element();
    let current = items.iter().position(|item| {
        let item: &web_sys::Element = item;
        active.as_ref() == Some(item)
    });
    let last = items.len().saturating_sub(1);
    let next = match key {
        "ArrowDown" => current.map_or(0, |i| if i >= last { 0 } else { i + 1 }),
        "ArrowUp" => current.map_or(last, |i| if i == 0 { last } else { i - 1 }),
        "Home" => 0,
        _ => last,
    };
    if let Some(item) = items.get(next) {
        let _ = item.focus();
    }
}

/// Focus an element by id after the next render, once it exists.
fn focus_soon(dom_id: String) {
    request_animation_frame(move || {
        if let Some(element) = document()
            .get_element_by_id(&dom_id)
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = element.focus();
        }
    });
}

/// Track an element's content width in `width` with a `ResizeObserver`.
fn observe_width(target: NodeRef<html::Div>, width: RwSignal<f64>) {
    type Observer = (web_sys::ResizeObserver, Closure<dyn FnMut(js_sys::Array)>);
    let observer: StoredValue<Option<Observer>, LocalStorage> = StoredValue::new_local(None);
    Effect::new(move |_| {
        let Some(element) = target.get() else {
            return;
        };
        if observer.with_value(Option::is_some) {
            return;
        }
        let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() else {
                return;
            };
            width.try_set(entry.content_rect().width());
        });
        let Ok(resize) = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()) else {
            return;
        };
        let element: &web_sys::Element = &element;
        resize.observe(element);
        observer.set_value(Some((resize, callback)));
    });
    on_cleanup(move || {
        if let Some((resize, _callback)) = observer.try_update_value(Option::take).flatten() {
            resize.disconnect();
        }
    });
}
