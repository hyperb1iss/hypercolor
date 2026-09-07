//! The mounting picker for a display-capable device: how the panel is
//! physically installed, so everything drawn on it turns to read upright.
//! A device setting, not a face or scene property, so it lives beside
//! brightness on the device and in the Studio screen header alike.

use hypercolor_types::scene::DisplayRotation;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use crate::api;
use crate::app::{DevicesContext, DisplaysContext};
use crate::components::silk_select::SilkSelect;
use crate::display_rotation::{
    display_rotation_select_options, display_rotation_value, parse_display_rotation,
};
use crate::icons::*;
use crate::toasts;

/// Picker for a display's mounting rotation. Renders nothing while
/// `rotation` is `None` (the device has no panel). Writes through the
/// device route and refreshes the device and display lists so every
/// consumer, including the Studio preview, follows the new mount.
#[component]
pub fn MountingSelect(
    /// The display-capable device to update.
    #[prop(into)]
    device_id: Signal<Option<String>>,
    /// The device's current mount; `None` hides the picker.
    #[prop(into)]
    rotation: Signal<Option<DisplayRotation>>,
    /// Trigger classes, so hosts can size the picker to their row.
    #[prop(into, optional)]
    class: Option<String>,
) -> impl IntoView {
    let devices = use_context::<DevicesContext>().map(|ctx| ctx.devices_resource);
    let displays = use_context::<DisplaysContext>().map(|ctx| ctx.displays_resource);
    // Optimistic local value so the trigger tracks the pick before the
    // refetch echoes it back; a failed write falls back to the server value.
    let (pending, set_pending) = signal(None::<DisplayRotation>);
    let value = Signal::derive(move || {
        pending
            .get()
            .or_else(|| rotation.get())
            .map_or_else(String::new, |rotation| {
                display_rotation_value(rotation).to_owned()
            })
    });
    Effect::new(move |_| {
        // A fresh server value retires the optimistic one.
        let _ = rotation.get();
        set_pending.set(None);
    });

    let on_change = Callback::new(move |raw: String| {
        let Some(id) = device_id.get_untracked() else {
            return;
        };
        let next = parse_display_rotation(&raw);
        if rotation.get_untracked() == Some(next) {
            return;
        }
        set_pending.set(Some(next));
        spawn_local(async move {
            match api::set_display_rotation(&id, next).await {
                Ok(_) => {
                    if let Some(devices) = devices {
                        devices.refetch();
                    }
                    if let Some(displays) = displays {
                        displays.refetch();
                    }
                }
                Err(error) => {
                    set_pending.try_set(None);
                    toasts::toast_error(&format!("Could not update the mounting: {error}"));
                }
            }
        });
    });

    let trigger_class = class.unwrap_or_else(|| {
        "border border-edge-subtle bg-surface-sunken/55 px-3 py-2 text-xs text-fg-primary"
            .to_owned()
    });

    view! {
        <Show when=move || rotation.get().is_some()>
            <div class="flex items-center gap-3" title="How the panel is mounted; everything drawn on it turns to match">
                <Icon icon=LuRotateCcw width="12px" height="12px" style="color: rgba(139, 133, 160, 0.5)" />
                <SilkSelect
                    value=value
                    options=Signal::derive(display_rotation_select_options)
                    on_change=on_change
                    placeholder="Mounting"
                    class=trigger_class.clone()
                    label_class="font-medium"
                />
            </div>
        </Show>
    }
}
