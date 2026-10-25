//! Saved controller placements whose hardware is absent from the registry.
//!
//! The card is dimmed and quiet: a vendor word, its status, and its LED
//! count. Remove (from this zone) and Forget (the saved controller,
//! everywhere) live inside its kebab, so the card keeps one trailing icon.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use crate::api;
use crate::app::DevicesContext;
use crate::components::device_pairing_modal::ModalBackdrop;
use crate::icons::{LuCpu, LuEllipsis, LuTrash2, LuX};
use crate::toasts;

use super::StudioContext;
use super::device_assignment::ZoneDeviceRow;
use super::device_card::friendly_offline_label;

#[derive(Clone, Copy)]
pub(super) struct SavedControllersRefresh(pub Callback<()>);

#[component]
pub(super) fn OfflineDeviceCard(row: ZoneDeviceRow, select: String, placed: bool) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let devices = expect_context::<DevicesContext>();
    let registry_available =
        Memo::new(move |_| matches!(devices.devices_resource.get(), Some(Ok(_))));
    let registry_status = move || match devices.devices_resource.get() {
        Some(Ok(_)) => "Offline",
        Some(Err(_)) => "Unavailable",
        None => "Loading",
    };
    let refresh = expect_context::<SavedControllersRefresh>();
    let menu_open = super::keyed_disclosure(
        studio.rail_disclosure,
        format!("offline-menu::{select}::{}", row.device_id),
    );
    let confirming = RwSignal::new(false);
    let submitting = RwSignal::new(false);
    let label = friendly_offline_label(&row.device_id);
    let device_id = StoredValue::new(row.device_id);
    let zone_id = StoredValue::new(select);
    let close = Callback::new(move |()| {
        if !submitting.get_untracked() {
            confirming.set(false);
        }
    });
    let forget = Callback::new(move |()| {
        if submitting.get_untracked() || !registry_available.get_untracked() {
            return;
        }
        submitting.set(true);
        let id = device_id.get_value();
        spawn_local(async move {
            match api::forget_saved_device(&id).await {
                Ok(()) => {
                    devices.devices_resource.refetch();
                    devices.layouts_resource.refetch();
                    studio.refresh_scene.run(());
                    refresh.0.run(());
                    confirming.set(false);
                    toasts::toast_success("Saved controller forgotten");
                }
                Err(error) => toasts::toast_error(&error.to_string()),
            }
            submitting.set(false);
        });
    });
    let has_actions = Memo::new(move |_| placed || registry_available.get());
    let item = "btn-press flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left \
                text-[11px] transition-colors hover:bg-surface-hover/40 disabled:opacity-40";

    view! {
        <div class="overflow-hidden rounded-lg border border-dashed border-edge-subtle/45 opacity-70">
            <div class="flex w-full items-center">
                <div
                    class="flex min-w-0 flex-1 items-center gap-2 px-2.5 py-2 text-fg-tertiary"
                    title=move || {
                        if registry_available.get() {
                            "Saved placement. Controller is not connected."
                        } else {
                            "Device connection status is unavailable."
                        }
                    }
                >
                    <Icon icon=LuCpu width="12px" height="12px" />
                    <span class="min-w-0 flex-1 truncate text-[11px]">{label}</span>
                    <span class="shrink-0 rounded bg-surface-sunken/70 px-1 text-[9px] font-medium">
                        {registry_status}
                    </span>
                    <span class="shrink-0 font-mono text-[9px]">{format!("{} LEDs", row.led_count)}</span>
                </div>
                <Show when=move || has_actions.get()>
                    <button
                        type="button"
                        class="btn-press mr-1 flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-fg-tertiary/70 transition-colors hover:text-fg-secondary"
                        title="Offline device options"
                        aria-label="Offline device options"
                        aria-expanded=move || menu_open.get().to_string()
                        on:click=move |_| menu_open.update(|open| *open = !*open)
                    >
                        <Icon icon=LuEllipsis width="14px" height="14px" />
                    </button>
                </Show>
            </div>
            <div
                class="space-y-0.5 border-t border-edge-subtle/45 bg-surface-sunken/60 px-1.5 py-1.5"
                class=("hidden", move || !menu_open.get())
            >
                {placed
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                class=format!("{item} text-fg-secondary")
                                disabled=move || studio.history.busy.get()
                                on:click=move |_| {
                                    menu_open.set(false);
                                    super::device_card::remove_device_from_zone(
                                        studio,
                                        zone_id.get_value(),
                                        device_id.get_value(),
                                    );
                                }
                            >
                                <Icon icon=LuX width="12px" height="12px" />
                                <span>"Remove from this zone"</span>
                            </button>
                        }
                    })}
                <Show when=move || registry_available.get()>
                    <button
                        type="button"
                        class=format!("{item} text-status-error")
                        on:click=move |_| {
                            menu_open.set(false);
                            confirming.set(true);
                        }
                    >
                        <Icon icon=LuTrash2 width="12px" height="12px" />
                        <span>"Forget controller"</span>
                    </button>
                </Show>
            </div>
        </div>
        <Show when=move || confirming.get() && registry_available.get()>
            <ModalBackdrop on_close=close label="Forget saved controller">
                <h2 class="mb-2 text-sm font-medium text-fg-primary">"Forget saved controller?"</h2>
                <p class="mb-2 text-xs text-fg-secondary">{label}</p>
                <p class="mb-3 break-all font-mono text-[10px] text-fg-tertiary">{move || device_id.get_value()}</p>
                <p class="mb-4 text-xs text-fg-tertiary">
                    "Remove this controller's attachments and outputs from all layouts and scenes. Temporary disconnection alone keeps these placements."
                </p>
                <div class="flex justify-end gap-2">
                    <button type="button" class="btn-press rounded-lg border border-edge-subtle px-3 py-2 text-xs text-fg-secondary"
                        disabled=move || submitting.get() on:click=move |_| close.run(())>"Keep controller"</button>
                    <button type="button" class="btn-press rounded-lg border border-status-error/30 bg-status-error/10 px-3 py-2 text-xs text-status-error"
                        disabled=move || submitting.get() on:click=move |_| forget.run(())>
                        {move || if submitting.get() { "Forgetting..." } else { "Forget controller" }}
                    </button>
                </div>
            </ModalBackdrop>
        </Show>
    }
}
