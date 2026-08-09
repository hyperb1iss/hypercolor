//! Confirm dialog for deleting a simulated display device.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, DeviceSummary};
use crate::app::DevicesContext;
use crate::components::device_pairing_modal::ModalBackdrop;
use crate::icons::*;
use crate::toasts;

/// Destructive confirm for removing a simulated display. Deletion tears
/// down the simulator config, its face assignments, and its layout
/// placements on the daemon side.
#[component]
pub fn DeleteSimulatorModal(
    device: DeviceSummary,
    #[prop(into)] on_close: Callback<()>,
    /// Fires with the device ID after the simulator is deleted. The parent
    /// should only dismiss the modal if the ID matches the currently-shown
    /// device, to guard against stale async responses.
    #[prop(into)]
    on_deleted: Callback<String>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();
    let device_id = device.id.clone();
    let device_name = device.name.clone();
    let (submitting, set_submitting) = signal(false);

    let do_delete = Callback::new({
        let device_id = device_id.clone();
        move |()| {
            let device_id = device_id.clone();
            set_submitting.set(true);
            let devices_resource = ctx.devices_resource;
            leptos::task::spawn_local(async move {
                match api::delete_simulated_display(&device_id).await {
                    Ok(()) => {
                        toasts::toast_success("Simulator deleted");
                        devices_resource.refetch();
                        on_deleted.run(device_id);
                    }
                    Err(error) => {
                        toasts::toast_error(&error);
                        set_submitting.set(false);
                    }
                }
            });
        }
    });

    view! {
        <ModalBackdrop on_close=on_close label="Delete simulator">
            <div class="text-center">
                <div class="w-12 h-12 rounded-xl flex items-center justify-center mx-auto mb-3"
                     style="background: rgba(255, 99, 99, 0.08); border: 1px solid rgba(255, 99, 99, 0.12)">
                    <Icon icon=LuTrash2 width="22px" height="22px" style="color: rgba(255, 99, 99, 0.6)" />
                </div>
                <h2 class="text-sm font-medium text-fg-primary mb-1">"Delete simulator?"</h2>
                <p class="text-xs text-fg-tertiary mb-4">
                    "This will remove "
                    <span class="text-fg-secondary font-medium">{device_name.clone()}</span>
                    " along with its face assignment and layout placements. This cannot be undone."
                </p>
                <div class="flex items-center gap-2 justify-center">
                    <button
                        class="flex-1 px-4 py-2 rounded-lg text-xs font-medium transition-all btn-press
                               border"
                        style="background: rgba(255, 99, 99, 0.1); color: rgb(255, 99, 99); border-color: rgba(255, 99, 99, 0.2)"
                        disabled=move || submitting.get()
                        on:click=move |_| do_delete.run(())
                    >
                        {move || if submitting.get() { "Deleting..." } else { "Delete simulator" }}
                    </button>
                    <button
                        class="px-4 py-2 rounded-lg text-xs font-medium text-fg-tertiary bg-surface-overlay/40
                               border border-edge-subtle hover:bg-surface-hover/60 transition-colors"
                        on:click=move |_| on_close.run(())
                    >
                        "Cancel"
                    </button>
                </div>
            </div>
        </ModalBackdrop>
    }
}
