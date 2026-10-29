//! The "Move to" menu, the single-pointer twin of every Studio drag.
//!
//! A device card's kebab, a channel row's context menu, a compact row in
//! Other zones, and the canvas properties panel all open it. It lists what
//! can move (the whole device, or one of its channels) and where: every
//! other LED zone, Unassigned, and a new zone named after the device.

use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::app::CapabilitiesContext;
use crate::components::zone_tabs::{ZoneDot, focus_soon};
use crate::icons::*;
use crate::pointer_gesture::listener_element;

use super::drag::{DragController, FocusReturn};
use super::drag_model::{DropAction, MovePayload, Origin};
use super::moves::{canvas_payload, led_zones, perform};
use super::zone_strip::{focus_first_item, menu_position, move_item_focus};

const ITEM_CLASS: &str = "dropdown-option flex w-full items-center gap-2.5 rounded-md px-2.5 \
    py-1.5 text-left text-[12px] text-fg-secondary transition-colors hover:text-fg-primary \
    disabled:cursor-not-allowed disabled:opacity-40";
const GROUP_LABEL_CLASS: &str = "px-2.5 pb-1 pt-1.5 text-[10px] font-semibold uppercase \
    tracking-[0.14em] text-fg-tertiary";

/// An open "Move to" menu.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MoveMenuAnchor {
    pub x: f64,
    pub y: f64,
    /// What can move, as `(label, payload)`: the whole device first, then
    /// its channels. One entry when only one thing can.
    pub scopes: Vec<(String, MovePayload)>,
    pub scope: usize,
    /// Where focus returns when the menu closes, and after a move.
    pub focus: FocusReturn,
}

impl MoveMenuAnchor {
    /// A menu opened under `element`.
    #[must_use]
    pub fn below(
        element: &web_sys::Element,
        scopes: Vec<(String, MovePayload)>,
        scope: usize,
        focus: FocusReturn,
    ) -> Self {
        let rect = element.get_bounding_client_rect();
        Self {
            x: rect.left(),
            y: rect.bottom() + 4.0,
            scopes,
            scope,
            focus,
        }
    }

    /// A menu opened at a pointer position.
    #[must_use]
    pub fn at(
        x: f64,
        y: f64,
        scopes: Vec<(String, MovePayload)>,
        scope: usize,
        focus: FocusReturn,
    ) -> Self {
        Self {
            x,
            y,
            scopes,
            scope,
            focus,
        }
    }

    fn payload(&self) -> Option<&MovePayload> {
        self.scopes
            .get(self.scope)
            .or_else(|| self.scopes.first())
            .map(|(_, payload)| payload)
    }
}

/// Whether a key on a focused row asks for its context menu.
#[must_use]
pub(crate) fn is_menu_key(ev: &web_sys::KeyboardEvent) -> bool {
    ev.key() == "ContextMenu" || (ev.key() == "F10" && ev.shift_key())
}

/// The portaled menu, mounted once per Studio page.
#[component]
pub(crate) fn MoveMenuHost() -> impl IntoView {
    let drag = expect_context::<DragController>();
    let studio = drag.studio;
    let caps = expect_context::<CapabilitiesContext>();
    let can_create = Memo::new(move |_| caps.zone_crud_ready());
    let menu = drag.menu;
    let menu_ref = NodeRef::<html::Div>::new();
    // The rendered size, so the menu stays on screen whatever the root
    // font size makes of its width.
    let size = RwSignal::new((280.0_f64, 320.0_f64));

    let open = Memo::new(move |_| menu.with(Option::is_some));
    Effect::new(move |_| {
        if open.get() {
            request_animation_frame(move || {
                if let Some(panel) = menu_ref.try_get_untracked().flatten() {
                    let rect = panel.get_bounding_client_rect();
                    let _ = size.try_set((rect.width(), rect.height()));
                }
                focus_first_item(menu_ref);
            });
        }
    });

    let destinations = Memo::new(move |_| {
        let payload = menu.with(|anchor| anchor.as_ref().and_then(|a| a.payload().cloned()))?;
        let zones = studio
            .active_scene
            .with(|scene| scene.as_ref().map(led_zones).unwrap_or_default())
            .into_iter()
            .filter(|(zone_id, _, _)| !payload.is_in(zone_id))
            .collect::<Vec<_>>();
        Some((payload, zones))
    });

    let close = move || {
        if let Some(anchor) = menu.get_untracked() {
            menu.set(None);
            focus_soon(anchor.focus.target);
        }
    };
    let choose = move |action: DropAction| {
        let Some(anchor) = menu.get_untracked() else {
            return;
        };
        menu.set(None);
        let Some(payload) = anchor.payload().cloned() else {
            return;
        };
        perform(drag, payload, action, Some(anchor.focus));
    };
    let busy = move || studio.history.busy.get();
    let ambient_hue = crate::ambient::hue_style();

    view! {
        <Show when=move || menu.with(Option::is_some)>
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
                    aria-label="Move to"
                    class="ambient-scroll fixed z-[71] max-h-96 w-60 overflow-y-auto rounded-lg border border-edge-subtle bg-surface-overlay/98 p-1 backdrop-blur-xl dropdown-glow animate-enter-down"
                    style=move || {
                        let (width, height) = size.get();
                        let position = menu.with(|anchor| {
                            anchor
                                .as_ref()
                                .map(|anchor| menu_position(anchor.x, anchor.y, width, height))
                                .unwrap_or_default()
                        });
                        format!("{position}; --ambient-hue: {}", ambient_hue())
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
                    {move || {
                        let scopes = menu
                            .with(|anchor| {
                                anchor.as_ref().map(|anchor| (anchor.scopes.clone(), anchor.scope))
                            })
                            .filter(|(scopes, _)| scopes.len() > 1)?;
                        let (scopes, current) = scopes;
                        Some(view! {
                            <div role="group" aria-label="What moves">
                                <div class=GROUP_LABEL_CLASS aria-hidden="true">"Move"</div>
                                {scopes
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, (label, _))| {
                                        let checked = index == current;
                                        view! {
                                            <button
                                                type="button"
                                                role="menuitemradio"
                                                aria-checked=checked.to_string()
                                                class=ITEM_CLASS
                                                on:click=move |_| {
                                                    menu.update(|anchor| {
                                                        if let Some(anchor) = anchor {
                                                            anchor.scope = index;
                                                        }
                                                    });
                                                }
                                            >
                                                <span
                                                    class="flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-full border"
                                                    class=("border-accent", checked)
                                                    class=("border-edge-default", !checked)
                                                    aria-hidden="true"
                                                >
                                                    {checked.then(|| view! {
                                                        <span class="h-1.5 w-1.5 rounded-full bg-accent" />
                                                    })}
                                                </span>
                                                <span class="min-w-0 truncate">{label}</span>
                                            </button>
                                        }
                                    })
                                    .collect_view()}
                            </div>
                            <div class="my-1 h-px bg-edge-subtle/60" role="separator" />
                        })
                    }}
                    <div role="group" aria-label="Destinations">
                        <div class=GROUP_LABEL_CLASS aria-hidden="true">"Move to"</div>
                        {move || {
                            let Some((payload, zones)) = destinations.get() else {
                                return ().into_any();
                            };
                            let rows = zones
                                .into_iter()
                                .map(|(zone_id, name, color)| {
                                    view! {
                                        <button
                                            type="button"
                                            role="menuitem"
                                            class=ITEM_CLASS
                                            disabled=busy
                                            on:click=move |_| choose(DropAction::MoveTo(zone_id.clone()))
                                        >
                                            <ZoneDot color=Signal::stored(color) />
                                            <span class="min-w-0 truncate">{name}</span>
                                        </button>
                                    }
                                })
                                .collect_view();
                            let unassign = (payload.origin != Origin::Unassigned).then(|| {
                                view! {
                                    <button
                                        type="button"
                                        role="menuitem"
                                        class=ITEM_CLASS
                                        disabled=busy
                                        on:click=move |_| choose(DropAction::Unassign)
                                    >
                                        <span class="h-2 w-2 shrink-0 rounded-full border border-dashed border-fg-tertiary/60" aria-hidden="true" />
                                        "Unassigned"
                                    </button>
                                }
                            });
                            view! {
                                {rows}
                                {unassign}
                                <Show when=move || can_create.get()>
                                    // Distinct from any zone the strip's "+" named "New zone",
                                    // which this menu also lists as a destination.
                                    <button
                                        type="button"
                                        role="menuitem"
                                        class=ITEM_CLASS
                                        aria-label="Move to a new zone"
                                        disabled=busy
                                        on:click=move |_| choose(DropAction::NewZone)
                                    >
                                        <Icon icon=LuPlus width="12px" height="12px" />
                                        "A new zone"
                                    </button>
                                </Show>
                            }
                                .into_any()
                        }}
                    </div>
                </div>
            </Portal>
        </Show>
    }
}

/// The canvas properties panel's "Move to" button for the selected
/// outputs. Renders nothing outside Studio.
#[component]
pub(crate) fn OutputMoveButton(output_ids: Vec<String>) -> impl IntoView {
    let Some(drag) = use_context::<DragController>() else {
        return ().into_any();
    };
    let studio = drag.studio;
    let ids = StoredValue::new(output_ids);
    let dom_id = "output-move-to";
    view! {
        <button
            type="button"
            id=dom_id
            class="shrink-0 rounded-md p-1 text-fg-tertiary/50 transition-colors btn-press hover:bg-accent/8 hover:text-accent disabled:opacity-40"
            title="Move to"
            aria-label="Move to"
            aria-haspopup="menu"
            disabled=move || studio.history.busy.get()
            on:click=move |ev: web_sys::MouseEvent| {
                let Some(element) = listener_element(&ev) else {
                    return;
                };
                let Some(zone_id) = studio.selected_surface_id.get_untracked() else {
                    return;
                };
                let Some(payload) = canvas_payload(drag, &zone_id, &ids.get_value()) else {
                    return;
                };
                drag.menu.set(Some(MoveMenuAnchor::below(
                    &element,
                    vec![(payload.label.clone(), payload)],
                    0,
                    FocusReturn {
                        target: dom_id.to_owned(),
                        fallback: None,
                    },
                )));
            }
        >
            <Icon icon=LuArrowRightLeft width="12px" height="12px" />
        </button>
    }
    .into_any()
}
