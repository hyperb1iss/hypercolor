use std::collections::HashSet;

use hypercolor_leptos_ext::events::{Change, Input};
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::device_brightness_slider::DeviceBrightnessSlider;
use crate::components::layout_builder::LayoutWriteHandle;
use crate::compound_selection::CompoundDepth;
use crate::icons::*;
use crate::layout_geometry;
use hypercolor_types::spatial::SpatialLayout;

use super::{group_op_button, zone_pixel_input};

#[component]
pub(super) fn GroupZoneProperties(
    ids: HashSet<String>,
    #[prop(into)] layout: Signal<Option<SpatialLayout>>,
    #[prop(into)] canvas_dims: Signal<(f32, f32)>,
    #[prop(into)] brightness_value: Signal<(u8, bool)>,
    brightness_rgb: String,
    on_brightness_change: Callback<u8>,
    set_layout: LayoutWriteHandle,
    set_is_dirty: WriteSignal<bool>,
    #[prop(into)] compound_depth: Signal<CompoundDepth>,
    group_rot_offset: ReadSignal<f32>,
    set_group_rot_offset: WriteSignal<f32>,
    group_scale_factor: ReadSignal<f32>,
    set_group_scale_factor: WriteSignal<f32>,
) -> impl IntoView {
    let count = ids.len();
    let (cw, ch) = canvas_dims.get_untracked();

    let centroid = layout
        .with_untracked(|l| {
            l.as_ref()
                .and_then(|l| layout_geometry::group_centroid(l, &ids))
        })
        .unwrap_or(hypercolor_types::spatial::NormalizedPosition::new(0.5, 0.5));
    let cx_px = centroid.x * cw;
    let cy_px = centroid.y * ch;

    let rot_offset = group_rot_offset.get_untracked();
    let scale_factor = group_scale_factor.get_untracked();

    let depth_label = {
        let depth = compound_depth.get_untracked();
        match depth {
            CompoundDepth::Root => "Device".to_string(),
            CompoundDepth::Device { .. } => "Slot".to_string(),
            CompoundDepth::Slot { ref slot_id, .. } => slot_id.clone(),
        }
    };

    let ids_pos_x = ids.clone();
    let ids_pos_y = ids.clone();
    let ids_center_h = ids.clone();
    let ids_center_v = ids.clone();
    let ids_rot_slider = ids.clone();
    let ids_rot_input = ids.clone();
    let ids_scale_slider = ids.clone();
    let ids_scale_input = ids.clone();
    let ids_align_left = ids.clone();
    let ids_align_hc = ids.clone();
    let ids_align_right = ids.clone();
    let ids_align_top = ids.clone();
    let ids_align_vc = ids.clone();
    let ids_align_bottom = ids.clone();
    let ids_dist_h = ids.clone();
    let ids_dist_v = ids.clone();
    let ids_pack_h = ids.clone();
    let ids_pack_v = ids.clone();
    let ids_mirror_h = ids.clone();
    let ids_mirror_v = ids;

    let dist_enabled = count >= 3;

    view! {
        <div class="space-y-2">
            <div class="flex flex-wrap items-center gap-x-3 gap-y-2 min-w-0">
                <div class="flex items-center gap-1.5 min-w-0">
                    <span
                        class="shrink-0 px-2 py-0.5 rounded-md text-[10px] font-mono tabular-nums"
                        style="color: rgba(225, 53, 255, 0.8); background: rgba(225, 53, 255, 0.08)"
                    >
                        {format!("{count} outputs")}
                    </span>
                    <span class="text-[10px] text-fg-tertiary/40 font-mono">{depth_label}</span>
                </div>
                <div class="flex-1" />
                <DeviceBrightnessSlider
                    value=brightness_value
                    on_change=on_brightness_change
                    rgb=brightness_rgb
                />
            </div>

            <div class="flex flex-wrap items-center gap-1.5">
                <div
                    class="flex items-center gap-1.5 shrink-0 rounded-lg px-2 py-1"
                    style="background: rgba(255, 255, 255, 0.02)"
                >
                    <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0 w-5">"Pos"</span>
                    {zone_pixel_input("X", cx_px, "1", 0, cw, {
                        let ids = ids_pos_x;
                        move |px: f32| {
                            let (cw, _) = canvas_dims.get_untracked();
                            let norm_x = (px / cw).clamp(0.0, 1.0);
                            let centroid = layout.with_untracked(|l| {
                                l.as_ref()
                                    .and_then(|l| layout_geometry::group_centroid(l, &ids))
                            });
                            if let Some(c) = centroid {
                                let target =
                                    hypercolor_types::spatial::NormalizedPosition::new(norm_x, c.y);
                                set_layout.update(|l| {
                                    if let Some(layout) = l {
                                        layout_geometry::translate_group(layout, &ids, target);
                                    }
                                });
                                set_is_dirty.set(true);
                            }
                        }
                    })}
                    {zone_pixel_input("Y", cy_px, "1", 0, ch, {
                        let ids = ids_pos_y;
                        move |px: f32| {
                            let (_, ch) = canvas_dims.get_untracked();
                            let norm_y = (px / ch).clamp(0.0, 1.0);
                            let centroid = layout.with_untracked(|l| {
                                l.as_ref()
                                    .and_then(|l| layout_geometry::group_centroid(l, &ids))
                            });
                            if let Some(c) = centroid {
                                let target =
                                    hypercolor_types::spatial::NormalizedPosition::new(c.x, norm_y);
                                set_layout.update(|l| {
                                    if let Some(layout) = l {
                                        layout_geometry::translate_group(layout, &ids, target);
                                    }
                                });
                                set_is_dirty.set(true);
                            }
                        }
                    })}
                    <button
                        class="shrink-0 p-0.5 rounded text-fg-tertiary/30 hover:text-accent transition-colors btn-press"
                        title="Center group horizontally"
                        on:click=move |_| {
                            let centroid = layout.with_untracked(|l| {
                                l.as_ref()
                                    .and_then(|l| layout_geometry::group_centroid(l, &ids_center_h))
                            });
                            if let Some(c) = centroid {
                                let target =
                                    hypercolor_types::spatial::NormalizedPosition::new(0.5, c.y);
                                set_layout.update(|l| {
                                    if let Some(layout) = l {
                                        layout_geometry::translate_group(
                                            layout,
                                            &ids_center_h,
                                            target,
                                        );
                                    }
                                });
                                set_is_dirty.set(true);
                            }
                        }
                    >
                        <Icon icon=LuAlignCenterHorizontal width="11px" height="11px" />
                    </button>
                    <button
                        class="shrink-0 p-0.5 rounded text-fg-tertiary/30 hover:text-accent transition-colors btn-press"
                        title="Center group vertically"
                        on:click=move |_| {
                            let centroid = layout.with_untracked(|l| {
                                l.as_ref()
                                    .and_then(|l| layout_geometry::group_centroid(l, &ids_center_v))
                            });
                            if let Some(c) = centroid {
                                let target =
                                    hypercolor_types::spatial::NormalizedPosition::new(c.x, 0.5);
                                set_layout.update(|l| {
                                    if let Some(layout) = l {
                                        layout_geometry::translate_group(
                                            layout,
                                            &ids_center_v,
                                            target,
                                        );
                                    }
                                });
                                set_is_dirty.set(true);
                            }
                        }
                    >
                        <Icon icon=LuAlignCenterVertical width="11px" height="11px" />
                    </button>
                </div>

                <div
                    class="flex items-center gap-1.5 flex-1 min-w-28 rounded-lg px-2 py-1"
                    style="background: rgba(255, 255, 255, 0.02)"
                >
                    <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0">"Rot"</span>
                    <input
                        type="range"
                        min="-180"
                        max="180"
                        step="1"
                        class="min-w-0 flex-1"
                        prop:value=format!("{rot_offset:.0}")
                        on:input=move |ev| {
                            let event = Input::from_event(ev);
                            if let Some(new_deg) = event.value::<f32>() {
                                let old_deg = group_rot_offset.get_untracked();
                                let delta_rad = (new_deg - old_deg).to_radians();
                                set_group_rot_offset.set(new_deg);
                                set_layout.update(|l| {
                                    if let Some(layout) = l {
                                        layout_geometry::rotate_group(
                                            layout,
                                            &ids_rot_slider,
                                            delta_rad,
                                        );
                                    }
                                });
                                set_is_dirty.set(true);
                            }
                        }
                    />
                    <div class="flex items-center gap-0.5 shrink-0">
                        <input
                            type="number"
                            min="-360"
                            max="360"
                            step="1"
                            class="w-12 bg-surface-sunken border border-edge-subtle rounded px-1.5 py-0.5
                                   text-[11px] text-fg-primary font-mono tabular-nums text-right
                                   focus:outline-none focus:border-accent-muted glow-ring transition-colors"
                            prop:value=format!("{rot_offset:.0}")
                            on:change=move |ev| {
                                let event = Change::from_event(ev);
                                if let Some(new_deg) = event.value::<f32>() {
                                    let old_deg = group_rot_offset.get_untracked();
                                    let delta_rad = (new_deg - old_deg).to_radians();
                                    set_group_rot_offset.set(new_deg);
                                    set_layout.update(|l| {
                                        if let Some(layout) = l {
                                            layout_geometry::rotate_group(
                                                layout,
                                                &ids_rot_input,
                                                delta_rad,
                                            );
                                        }
                                    });
                                    set_is_dirty.set(true);
                                }
                            }
                        />
                        <span class="text-[11px] font-mono text-fg-tertiary/30">{"\u{00b0}"}</span>
                    </div>
                </div>

                <div
                    class="flex items-center gap-1.5 flex-1 min-w-28 rounded-lg px-2 py-1"
                    style="background: rgba(255, 255, 255, 0.02)"
                >
                    <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0">"Scale"</span>
                    <input
                        type="range"
                        min="0.2"
                        max="3.0"
                        step="0.05"
                        class="min-w-0 flex-1"
                        prop:value=format!("{scale_factor:.2}")
                        on:input=move |ev| {
                            let event = Input::from_event(ev);
                            if let Some(new_factor) = event.value::<f32>() {
                                let old_factor = group_scale_factor.get_untracked();
                                if old_factor.abs() > 0.001 {
                                    let ratio = new_factor / old_factor;
                                    set_group_scale_factor.set(new_factor);
                                    set_layout.update(|l| {
                                        if let Some(layout) = l {
                                            layout_geometry::scale_group(
                                                layout,
                                                &ids_scale_slider,
                                                ratio,
                                            );
                                        }
                                    });
                                    set_is_dirty.set(true);
                                }
                            }
                        }
                    />
                    <div class="flex items-center gap-0.5 shrink-0">
                        <input
                            type="number"
                            min="0.2"
                            max="3.0"
                            step="0.05"
                            class="w-12 bg-surface-sunken border border-edge-subtle rounded px-1.5 py-0.5
                                   text-[11px] text-fg-primary font-mono tabular-nums text-right
                                   focus:outline-none focus:border-accent-muted glow-ring transition-colors"
                            prop:value=format!("{scale_factor:.2}")
                            on:change=move |ev| {
                                let event = Change::from_event(ev);
                                if let Some(new_factor) = event.value::<f32>() {
                                    let old_factor = group_scale_factor.get_untracked();
                                    if old_factor.abs() > 0.001 {
                                        let ratio = new_factor / old_factor;
                                        set_group_scale_factor.set(new_factor);
                                        set_layout.update(|l| {
                                            if let Some(layout) = l {
                                                layout_geometry::scale_group(
                                                    layout,
                                                    &ids_scale_input,
                                                    ratio,
                                                );
                                            }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                }
                            }
                        />
                        <span class="text-[11px] font-mono text-fg-tertiary/30">"x"</span>
                    </div>
                </div>
            </div>

            <div class="flex items-center gap-1.5 flex-wrap">
                <div
                    class="flex items-center gap-0.5 shrink-0 rounded-lg px-2 py-1"
                    style="background: rgba(255, 255, 255, 0.02)"
                >
                    <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0 pr-1">"Align"</span>
                    {group_op_button(LuAlignStartVertical, "Align left edges", move || {
                        let ids = ids_align_left.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                    layout_geometry::AlignAnchor::Min,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {group_op_button(LuAlignCenterVertical, "Align horizontal centers", move || {
                        let ids = ids_align_hc.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                    layout_geometry::AlignAnchor::Center,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {group_op_button(LuAlignEndVertical, "Align right edges", move || {
                        let ids = ids_align_right.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                    layout_geometry::AlignAnchor::Max,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    <div class="w-px h-3 bg-edge-subtle mx-0.5" />
                    {group_op_button(LuAlignStartHorizontal, "Align top edges", move || {
                        let ids = ids_align_top.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::Y,
                                    layout_geometry::AlignAnchor::Min,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {group_op_button(LuAlignCenterHorizontal, "Align vertical centers", move || {
                        let ids = ids_align_vc.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::Y,
                                    layout_geometry::AlignAnchor::Center,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {group_op_button(LuAlignEndHorizontal, "Align bottom edges", move || {
                        let ids = ids_align_bottom.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::Y,
                                    layout_geometry::AlignAnchor::Max,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                </div>

                <div
                    class="flex items-center gap-0.5 shrink-0 rounded-lg px-2 py-1"
                    style=move || if dist_enabled {
                        "background: rgba(255, 255, 255, 0.02)"
                    } else {
                        "background: rgba(255, 255, 255, 0.02); opacity: 0.35; pointer-events: none"
                    }
                    title=move || if dist_enabled {
                        ""
                    } else {
                        "Distribute needs 3+ outputs"
                    }
                >
                    <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0 pr-1">"Dist"</span>
                    {group_op_button(LuAlignHorizontalDistributeCenter, "Distribute horizontally (even gaps)", move || {
                        let ids = ids_dist_h.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::distribute_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {group_op_button(LuAlignVerticalDistributeCenter, "Distribute vertically (even gaps)", move || {
                        let ids = ids_dist_v.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::distribute_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::Y,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                </div>

                <div
                    class="flex items-center gap-0.5 shrink-0 rounded-lg px-2 py-1"
                    style="background: rgba(255, 255, 255, 0.02)"
                >
                    <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0 pr-1">"Pack"</span>
                    {group_op_button(LuFoldHorizontal, "Pack horizontally (no gaps)", move || {
                        let ids = ids_pack_h.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::pack_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {group_op_button(LuFoldVertical, "Pack vertically (no gaps)", move || {
                        let ids = ids_pack_v.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::pack_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::Y,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                </div>

                <div
                    class="flex items-center gap-0.5 shrink-0 rounded-lg px-2 py-1"
                    style="background: rgba(255, 255, 255, 0.02)"
                >
                    <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0 pr-1">"Mirror"</span>
                    {group_op_button(LuFlipHorizontal, "Mirror across vertical axis", move || {
                        let ids = ids_mirror_h.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::mirror_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {group_op_button(LuFlipVertical, "Mirror across horizontal axis", move || {
                        let ids = ids_mirror_v.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::mirror_group(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::Y,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                </div>

                <div class="flex-1" />
                <span class="text-[9px] text-fg-tertiary/30 shrink-0">
                    "Transforms apply around group center"
                </span>
            </div>
        </div>
    }
}
