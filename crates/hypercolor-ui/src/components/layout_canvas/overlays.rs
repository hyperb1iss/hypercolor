use std::collections::HashSet;

use leptos::prelude::*;

use crate::api;
use crate::compound_selection::CompoundDepth;
use crate::layout_geometry;
use crate::layout_utils;
use hypercolor_types::spatial::SpatialLayout;

#[component]
pub(super) fn CompoundBoundingBoxOutline(
    #[prop(into)] layout: Signal<Option<SpatialLayout>>,
    #[prop(into)] selected_zone_ids: Signal<HashSet<String>>,
    #[prop(into)] compound_depth: Signal<CompoundDepth>,
) -> impl IntoView {
    view! {
        {move || {
            let ids = selected_zone_ids.get();
            if ids.len() <= 1 {
                return None;
            }
            layout.with(|l| {
                let layout = l.as_ref()?;
                let bounds = layout_geometry::compound_bounding_box(layout, &ids)?;
                let depth = compound_depth.get();
                let entered = !matches!(depth, CompoundDepth::Root);

                let x_pct = (bounds.center.x - bounds.size.x * 0.5) * 100.0;
                let y_pct = (bounds.center.y - bounds.size.y * 0.5) * 100.0;
                let w_pct = bounds.size.x * 100.0;
                let h_pct = bounds.size.y * 100.0;

                let opacity = if entered { "0.2" } else { "1" };
                let style = format!(
                    "left: {x_pct:.2}%; top: {y_pct:.2}%; width: {w_pct:.2}%; height: {h_pct:.2}%; \
                     opacity: {opacity}; \
                     border: 1.5px dashed rgba(128, 255, 234, 0.4); \
                     border-radius: 8px; \
                     box-shadow: 0 0 16px rgba(128, 255, 234, 0.08); \
                     pointer-events: none; \
                     transition: opacity 0.15s ease"
                );

                let hint = matches!(depth, CompoundDepth::Root).then(|| {
                    view! {
                        <div class="absolute -bottom-4 left-1/2 -translate-x-1/2 whitespace-nowrap
                                    text-[9px] text-fg-tertiary/40 pointer-events-none select-none
                                    animate-[fadeIn_0.3s_ease]">
                            "Double-click to select individually"
                        </div>
                    }
                });

                Some(view! {
                    <div class="absolute" style=style>
                        {hint}
                    </div>
                })
            })
        }}
    }
}

#[component]
pub(super) fn CanvasDepthBreadcrumb(
    #[prop(into)] compound_depth: Signal<CompoundDepth>,
    devices_resource: LocalResource<Result<Vec<api::DeviceSummary>, String>>,
) -> impl IntoView {
    view! {
        {move || {
            let depth = compound_depth.get();
            match depth {
                CompoundDepth::Root => None,
                CompoundDepth::Device { ref device_id } => {
                    let name = devices_resource
                        .get()
                        .and_then(Result::ok)
                        .and_then(|devices| {
                            layout_utils::effective_device_name(device_id, &devices)
                        })
                        .unwrap_or_else(|| "Device".to_string());
                    Some(view! {
                        <div class="absolute bottom-2 left-1/2 -translate-x-1/2 z-50
                                    flex items-center gap-2 px-3 py-1.5 rounded-lg
                                    bg-black/60 backdrop-blur-sm border border-edge-subtle/30
                                    pointer-events-none select-none">
                            <span class="text-[10px] font-medium" style="color: rgba(128, 255, 234, 0.7)">
                                {name}
                            </span>
                            <span class="text-[9px] text-fg-tertiary/40">
                                "\u{203a} Slots"
                            </span>
                            <span class="text-[9px] text-fg-tertiary/25 ml-1">
                                "Esc to exit"
                            </span>
                        </div>
                    }.into_any())
                }
                CompoundDepth::Slot { ref device_id, ref slot_id } => {
                    let (dev_name, slot_name) = devices_resource
                        .get()
                        .and_then(Result::ok)
                        .map(|devices| {
                            let dev_name = layout_utils::effective_device_name(
                                device_id,
                                &devices,
                            )
                            .unwrap_or_else(|| "Device".to_string());
                            let slot_name = layout_utils::effective_slot_name(
                                device_id,
                                slot_id,
                                &devices,
                            )
                            .unwrap_or_else(|| slot_id.replace('-', " "));
                            (dev_name, slot_name)
                        })
                        .unwrap_or_else(|| {
                            ("Device".to_string(), slot_id.replace('-', " "))
                        });
                    Some(view! {
                        <div class="absolute bottom-2 left-1/2 -translate-x-1/2 z-50
                                    flex items-center gap-2 px-3 py-1.5 rounded-lg
                                    bg-black/60 backdrop-blur-sm border border-edge-subtle/30
                                    pointer-events-none select-none">
                            <span class="text-[10px] font-medium" style="color: rgba(128, 255, 234, 0.7)">
                                {dev_name}
                            </span>
                            <span class="text-[9px] text-fg-tertiary/40">
                                "\u{203a} "
                            </span>
                            <span class="text-[10px] font-medium capitalize" style="color: rgba(128, 255, 234, 0.5)">
                                {slot_name}
                            </span>
                            <span class="text-[9px] text-fg-tertiary/25 ml-1">
                                "Esc to go back"
                            </span>
                        </div>
                    }.into_any())
                }
            }
        }}
    }
}
