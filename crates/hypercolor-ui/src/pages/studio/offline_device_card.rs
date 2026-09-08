//! Saved controller placements whose hardware is absent from the registry.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use crate::api;
use crate::app::DevicesContext;
use crate::components::device_pairing_modal::ModalBackdrop;
use crate::icons::{LuCpu, LuTrash2, LuX};
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
    let refresh = expect_context::<SavedControllersRefresh>();
    let select_body = select.clone();
    let confirming = RwSignal::new(false);
    let submitting = RwSignal::new(false);
    let label = friendly_offline_label(&row.device_id);
    let device_id = StoredValue::new(row.device_id);
    let close = Callback::new(move |()| {
        if !submitting.get_untracked() {
            confirming.set(false);
        }
    });
    let remove = Callback::new(move |()| {
        if submitting.get_untracked() {
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
                    toasts::toast_success("Saved controller removed");
                }
                Err(error) => toasts::toast_error(&error.to_string()),
            }
            submitting.set(false);
        });
    });

    view! {
        <div class="flex w-full items-center rounded-lg border border-dashed border-edge-subtle/45">
            <button
                type="button"
                class="flex min-w-0 flex-1 items-center gap-2 px-2.5 py-2 text-left text-fg-tertiary"
                title="Saved placement. Controller is not connected."
                on:click=move |_| studio.selected_surface_id.set(Some(select_body.clone()))
            >
                <Icon icon=LuCpu width="12px" height="12px" />
                <span class="min-w-0 flex-1 truncate text-[11px]">{label}</span>
                <span class="shrink-0 rounded bg-surface-sunken/70 px-1 text-[9px] font-medium">"Offline"</span>
                <span class="shrink-0 font-mono text-[9px]">{format!("{} LEDs", row.led_count)}</span>
            </button>
            {placed.then(|| view! {
                <button type="button" class="btn-press flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-fg-tertiary hover:bg-surface-hover"
                    title="Remove from this zone" aria-label="Remove from this zone"
                    on:click=move |_| super::device_card::remove_device_from_zone(studio, select.clone(), device_id.get_value())>
                    <Icon icon=LuX width="13px" height="13px" />
                </button>
            })}
            <button
                type="button"
                class="btn-press mr-1.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-status-error hover:bg-status-error/10"
                title="Delete saved controller"
                aria-label="Delete saved controller"
                on:click=move |_| confirming.set(true)
            >
                <Icon icon=LuTrash2 width="13px" height="13px" />
            </button>
        </div>
        <Show when=move || confirming.get()>
            <ModalBackdrop on_close=close label="Delete saved controller">
                <h2 class="mb-2 text-sm font-medium text-fg-primary">"Delete saved controller?"</h2>
                <p class="mb-2 text-xs text-fg-secondary">{label}</p>
                <p class="mb-3 break-all font-mono text-[10px] text-fg-tertiary">{move || device_id.get_value()}</p>
                <p class="mb-4 text-xs text-fg-tertiary">
                    "Remove this controller's attachments and outputs from all layouts and scenes. Temporary disconnection alone keeps these placements."
                </p>
                <div class="flex justify-end gap-2">
                    <button type="button" class="btn-press rounded-lg border border-edge-subtle px-3 py-2 text-xs text-fg-secondary"
                        disabled=move || submitting.get() on:click=move |_| close.run(())>"Cancel"</button>
                    <button type="button" class="btn-press rounded-lg border border-status-error/30 bg-status-error/10 px-3 py-2 text-xs text-status-error"
                        disabled=move || submitting.get() on:click=move |_| remove.run(())>
                        {move || if submitting.get() { "Deleting..." } else { "Delete controller" }}
                    </button>
                </div>
            </ModalBackdrop>
        </Show>
    }
}
