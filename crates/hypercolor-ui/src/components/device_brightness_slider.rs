//! Brightness slider used by the layout zone properties panel.
//!
//! Pure UI: caller supplies the current value (plus a `mixed` flag for
//! when selected zones disagree) and a change callback. The caller owns
//! aggregation and the write path. Brightness in Hypercolor is now a
//! per-zone property on `DeviceZone`, so the change callback typically
//! updates the spatial layout and lets the existing preview flow push
//! the delta to the daemon.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::icons::*;
use hypercolor_leptos_ext::events::Input;

#[component]
pub fn DeviceBrightnessSlider(
    /// Current brightness as `(value_0_to_100, mixed)`. `mixed` is true
    /// when the selection spans zones with differing brightness; the
    /// slider then displays "Mixed" and sits at the average.
    #[prop(into)]
    value: Signal<(u8, bool)>,
    /// Fires with the new brightness (0-100) on each drag input event.
    on_change: Callback<u8>,
    /// Accent color rgb triple (e.g. `"255, 106, 193"`).
    #[prop(into)]
    rgb: String,
) -> impl IntoView {
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
                prop:value=move || value.get().0.to_string()
                on:input=move |ev| {
                    let event = Input::from_event(ev);
                    if let Some(brightness) = event.value::<u8>() {
                        on_change.run(brightness);
                    }
                }
            />
            <span class="text-[10px] font-mono tabular-nums text-fg-tertiary w-10 text-right shrink-0">
                {move || {
                    let (v, mixed) = value.get();
                    if mixed {
                        "Mixed".to_string()
                    } else {
                        format!("{v}%")
                    }
                }}
            </span>
        </div>
    }
}
