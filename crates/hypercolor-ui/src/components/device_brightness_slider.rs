//! Multi-device brightness slider used by the layout zone properties panel.
//!
//! Controls the master brightness of one or more devices at once. Writes
//! flow through `PUT /api/v1/devices/:id` and the daemon's
//! `sync_device_output_brightness` path. When the selection spans devices
//! with differing brightness the label shows "Mixed" and the slider sits
//! at the average; dragging collapses all devices to the new value.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_throttle_fn_with_arg;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::DevicesContext;
use crate::icons::*;

#[component]
pub fn DeviceBrightnessSlider(
    /// Physical device ids (`DeviceSummary::id`) to control. Typically
    /// derived from the currently-selected layout zones' owning devices.
    #[prop(into)]
    device_ids: Signal<Vec<String>>,
    /// Accent color rgb triple (e.g. `"255, 106, 193"`) for the slider
    /// accent and icon tint.
    #[prop(into)]
    rgb: String,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    // Aggregate brightness across the selected devices: returns
    // `(display_value, mixed)`. `mixed` is true when devices disagree.
    let brightness_agg = Memo::new(move |_| {
        let ids = device_ids.get();
        if ids.is_empty() {
            return (100u8, false);
        }
        let devices = ctx
            .devices_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default();
        let brightnesses: Vec<u8> = ids
            .iter()
            .filter_map(|id| devices.iter().find(|d| &d.id == id).map(|d| d.brightness))
            .collect();
        if brightnesses.is_empty() {
            return (100u8, false);
        }
        let first = brightnesses[0];
        let mixed = brightnesses.iter().any(|b| *b != first);
        if mixed {
            let sum: u32 = brightnesses.iter().map(|b| u32::from(*b)).sum();
            #[allow(clippy::cast_possible_truncation)]
            let avg = (sum / brightnesses.len() as u32) as u8;
            (avg, true)
        } else {
            (first, false)
        }
    });

    // Local signal drives the slider; we sync from `brightness_agg`
    // whenever selection or server state changes so the slider tracks
    // ground truth without fighting the user's drag.
    let (value, set_value) = signal(brightness_agg.get_untracked().0);
    let mixed = Memo::new(move |_| brightness_agg.get().1);

    Effect::new(move |_| {
        let new_value = brightness_agg.get().0;
        if value.get_untracked() != new_value {
            set_value.set(new_value);
        }
    });

    let push = use_throttle_fn_with_arg(
        move |brightness: u8| {
            let ids = device_ids.get_untracked();
            let devices_resource = ctx.devices_resource;
            leptos::task::spawn_local(async move {
                let mut had_err = false;
                for id in &ids {
                    let req = api::UpdateDeviceRequest {
                        name: None,
                        enabled: None,
                        brightness: Some(brightness),
                    };
                    if api::update_device(id, &req).await.is_err() {
                        had_err = true;
                    }
                }
                if had_err {
                    devices_resource.refetch();
                }
            });
        },
        50.0,
    );

    let icon_style = format!("color: rgba({rgb}, 0.6); flex-shrink: 0");
    let track_style = format!("accent-color: rgb({rgb})");

    view! {
        <div class="flex items-center gap-2 min-w-0">
            <Icon icon=LuSun width="12px" height="12px" style=icon_style />
            <input
                type="range"
                min="0"
                max="100"
                step="1"
                class="flex-1 min-w-[100px] max-w-[180px] h-1 rounded-full appearance-none cursor-pointer"
                style=track_style
                prop:value=move || value.get().to_string()
                on:input=move |ev| {
                    let target = ev
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                    if let Some(el) = target
                        && let Ok(brightness) = el.value().parse::<u8>()
                    {
                        set_value.set(brightness);
                        push(brightness);
                    }
                }
            />
            <span class="text-[10px] font-mono tabular-nums text-fg-tertiary w-10 text-right shrink-0">
                {move || {
                    if mixed.get() {
                        "Mixed".to_string()
                    } else {
                        format!("{}%", value.get())
                    }
                }}
            </span>
        </div>
    }
}
