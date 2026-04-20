//! Per-device brightness slider — slots under each palette device card header.
//!
//! For multi-zone devices this is effectively a group-level brightness: the
//! daemon applies the value to every zone of the device.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_throttle_fn_with_arg;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::DevicesContext;
use crate::icons::*;

#[component]
pub(super) fn DeviceBrightnessSlider(
    /// Device id as used by the REST layer (`dev.id`, NOT
    /// `layout_device_id` — the daemon indexes brightness by the physical
    /// device id).
    device_id: String,
    /// Initial brightness (0–100) seeded from the device summary.
    initial: u8,
    /// Accent color triple used for the track + icon glow so the slider
    /// matches the card it lives on.
    #[prop(into)]
    rgb: String,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();
    let (value, set_value) = signal(initial);

    let device_id_for_push = device_id.clone();
    let push = use_throttle_fn_with_arg(
        move |brightness: u8| {
            let did = device_id_for_push.clone();
            let devices_resource = ctx.devices_resource;
            leptos::task::spawn_local(async move {
                let req = api::UpdateDeviceRequest {
                    name: None,
                    enabled: None,
                    brightness: Some(brightness),
                };
                if api::update_device(&did, &req).await.is_err() {
                    devices_resource.refetch();
                }
            });
        },
        50.0,
    );

    let icon_style = format!("color: rgba({rgb}, 0.55); flex-shrink: 0");
    let border_style = format!("border-color: rgba({rgb}, 0.08)");
    let track_style = format!("accent-color: rgb({rgb})");

    view! {
        <div
            class="flex items-center gap-2 px-2.5 py-1.5 border-t"
            style=border_style
            on:click=|ev: web_sys::MouseEvent| ev.stop_propagation()
        >
            <Icon icon=LuSun width="11px" height="11px" style=icon_style />
            <input
                type="range"
                min="0"
                max="100"
                step="1"
                class="flex-1 h-1 rounded-full appearance-none cursor-pointer"
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
            <span class="text-[9px] font-mono tabular-nums text-fg-tertiary/70 w-8 text-right shrink-0">
                {move || format!("{}%", value.get())}
            </span>
        </div>
    }
}
