//! Mobile bottom tab bar — replaces the sidebar below the `md` breakpoint.
//!
//! Renders the same [`nav_model`] the sidebar consumes, so extension nav
//! items and route ordering never drift between the two shells. Fixed to the
//! bottom viewport edge with safe-area padding for notched phones; the shell
//! pads `<main>` by the same height so content never hides behind the bar.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use crate::nav::{NavExtensionItems, nav_model};

#[component]
pub fn MobileNav() -> impl IntoView {
    let location = use_location();
    let extension_nav = use_context::<NavExtensionItems>().unwrap_or_default();

    view! {
        <nav
            class="md:hidden fixed bottom-0 inset-x-0 z-40 glass border-t border-edge-subtle"
            style="padding-bottom: env(safe-area-inset-bottom)"
            aria-label="Primary"
        >
            <div class="flex items-stretch h-14">
                {move || nav_model(&extension_nav.0).into_iter().map(|item| {
                    let is_active = {
                        let path = item.path;
                        Memo::new(move |_| {
                            let current = location.pathname.get();
                            crate::route_ui::route_is_active(&current, path)
                        })
                    };

                    view! {
                        <A
                            href=crate::route_ui::route_href(item.path)
                            attr:class=move || {
                                let base = "relative flex-1 flex flex-col items-center \
                                            justify-center gap-1 min-w-0 btn-press";
                                if is_active.get() {
                                    format!("{base} text-fg-primary")
                                } else {
                                    format!("{base} text-fg-tertiary")
                                }
                            }
                        >
                            // Active indicator — top edge, the sidebar's left
                            // bar rotated for a bottom bar.
                            <div
                                class="absolute top-0 inset-x-4 h-[2px] rounded-b-full bg-accent transition-opacity duration-200"
                                class:opacity-0=move || !is_active.get()
                                class:opacity-100=move || is_active.get()
                                style:box-shadow=move || if is_active.get() {
                                    "0 0 8px rgba(225, 53, 255, 0.5)"
                                } else {
                                    "none"
                                }
                            />
                            <span
                                class="w-[18px] h-[18px] flex items-center justify-center"
                                class:text-accent=move || is_active.get()
                            >
                                <Icon icon=item.icon width="18px" height="18px" />
                            </span>
                            <span class="text-[10px] leading-none truncate max-w-full px-1">
                                {item.label}
                            </span>
                        </A>
                    }
                }).collect_view()}
            </div>
        </nav>
    }
}
