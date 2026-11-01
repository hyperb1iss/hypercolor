//! Mobile bottom tab bar — replaces the sidebar below the `md` breakpoint.
//!
//! Renders the core tabs from the shared nav model ([`phone_nav`]), so
//! route ordering never drifts from the sidebar. Extension nav items do not
//! get tabs of their own: each one would shrink every core tab, so they
//! live in a More sheet behind a seventh tab that appears only when an
//! extension contributes items. Fixed to the bottom viewport edge with
//! safe-area padding for notched phones; the shell pads `<main>` by the
//! same height so content never hides behind the bar.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use crate::components::bottom_sheet::BottomSheet;
use crate::icons::LuEllipsis;
use crate::nav::{NavEntry, NavExtensionItems, phone_nav};

const TAB_CLASS: &str = "relative flex-1 flex flex-col items-center justify-center gap-1 min-w-0 \
                         btn-press";

/// The active-route bar on a tab's top edge: the sidebar's left bar,
/// rotated for a bottom bar.
#[component]
fn ActiveIndicator(#[prop(into)] active: Signal<bool>) -> impl IntoView {
    view! {
        <div
            class="absolute top-0 inset-x-4 h-[2px] rounded-b-full bg-accent transition-opacity duration-200"
            class:opacity-0=move || !active.get()
            class:opacity-100=move || active.get()
            style:box-shadow=move || if active.get() {
                "0 0 8px rgba(225, 53, 255, 0.5)"
            } else {
                "none"
            }
        />
    }
}

/// One core tab.
#[component]
fn TabLink(item: NavEntry) -> impl IntoView {
    let location = use_location();
    let path = item.path;
    let is_active = Memo::new(move |_| {
        let current = location.pathname.get();
        crate::route_ui::route_is_active(&current, path)
    });

    view! {
        <A
            href=crate::route_ui::route_href(item.path)
            attr:class=move || {
                if is_active.get() {
                    format!("{TAB_CLASS} text-fg-primary")
                } else {
                    format!("{TAB_CLASS} text-fg-tertiary")
                }
            }
        >
            <ActiveIndicator active=is_active />
            <span
                class="w-[18px] h-[18px] flex items-center justify-center"
                class:text-accent=move || is_active.get()
            >
                <Icon icon=item.icon width="18px" height="18px" />
            </span>
            <span class="text-[10px] leading-none truncate max-w-full px-0.5">
                {item.label}
            </span>
        </A>
    }
}

#[component]
pub fn MobileNav() -> impl IntoView {
    let location = use_location();
    let extension_nav = use_context::<NavExtensionItems>().unwrap_or_default();
    // Extension items are composed once at startup, so the split is fixed.
    let nav = phone_nav(&extension_nav.0);
    let more = StoredValue::new(nav.more);
    let has_more = more.with_value(|items| !items.is_empty());
    let (more_open, set_more_open) = signal(false);
    let more_active = Memo::new(move |_| {
        let current = location.pathname.get();
        more_open.get()
            || more.with_value(|items| {
                items
                    .iter()
                    .any(|item| crate::route_ui::route_is_active(&current, item.path))
            })
    });

    view! {
        <nav
            class="md:hidden fixed bottom-0 inset-x-0 z-40 glass border-t border-edge-subtle"
            style="padding-bottom: env(safe-area-inset-bottom)"
            aria-label="Primary"
        >
            <div class="flex items-stretch h-14">
                {nav.tabs.into_iter().map(|item| view! { <TabLink item=item /> }).collect_view()}
                {has_more.then(|| view! {
                    <button
                        type="button"
                        class=move || {
                            if more_active.get() {
                                format!("{TAB_CLASS} text-fg-primary")
                            } else {
                                format!("{TAB_CLASS} text-fg-tertiary")
                            }
                        }
                        aria-haspopup="dialog"
                        aria-expanded=move || more_open.get().to_string()
                        on:click=move |_| set_more_open.set(true)
                    >
                        <ActiveIndicator active=more_active />
                        <span
                            class="w-[18px] h-[18px] flex items-center justify-center"
                            class:text-accent=move || more_active.get()
                        >
                            <Icon icon=LuEllipsis width="18px" height="18px" />
                        </span>
                        <span class="text-[10px] leading-none truncate max-w-full px-0.5">"More"</span>
                    </button>
                })}
            </div>
        </nav>
        <Show when=move || more_open.get()>
            <BottomSheet on_close=Callback::new(move |()| set_more_open.set(false)) title="More">
                <ul class="space-y-1 pb-1">
                    {more.get_value().into_iter().map(|item| {
                        let path = item.path;
                        let is_active = Memo::new(move |_| {
                            crate::route_ui::route_is_active(&location.pathname.get(), path)
                        });
                        view! {
                            <li>
                                <A
                                    href=crate::route_ui::route_href(item.path)
                                    attr:class=move || {
                                        let base = "flex min-h-12 items-center gap-3 rounded-lg px-3 \
                                                    text-sm active:bg-surface-hover/30";
                                        if is_active.get() {
                                            format!("{base} bg-accent/10 text-fg-primary")
                                        } else {
                                            format!("{base} text-fg-secondary")
                                        }
                                    }
                                    on:click=move |_| set_more_open.set(false)
                                >
                                    <span
                                        class="flex h-5 w-5 shrink-0 items-center justify-center"
                                        class:text-accent=move || is_active.get()
                                    >
                                        <Icon icon=item.icon width="18px" height="18px" />
                                    </span>
                                    <span class="min-w-0 flex-1 truncate">{item.label}</span>
                                </A>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            </BottomSheet>
        </Show>
    }
}
