use std::collections::HashSet;

use crate::components::layer_panel::scope::{is_screen_output, selection_count_words};
use hypercolor_leptos_ext::events::{Change, Input};
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::layout_builder::LayoutWriteHandle;
use crate::components::layout_builder::gesture::merge_gesture;
use crate::compound_selection::CompoundDepth;
use crate::icons::*;
use crate::layout_geometry;
use crate::pages::studio::layer_target::OutputLayerButton;
use crate::pages::studio::move_menu::OutputMoveButton;
use hypercolor_types::spatial::{Output, SpatialLayout};

use super::{
    FIELD_AFFIX, FIELD_BOX, FIELD_INPUT, GROUP, GROUP_LABEL, ICON_BUTTON, compound_op_button,
    zone_pixel_input,
};

/// A cluster of one-shot operations, led by the word that names it.
const OPS: &str = "flex shrink-0 items-center gap-0.5";

#[component]
pub(super) fn CompoundZoneProperties(
    ids: HashSet<String>,
    #[prop(into)] layout: Signal<Option<SpatialLayout>>,
    #[prop(into)] canvas_dims: Signal<(f32, f32)>,
    set_layout: LayoutWriteHandle,
    set_is_dirty: WriteSignal<bool>,
    #[prop(into)] compound_depth: Signal<CompoundDepth>,
    compound_rot_offset: ReadSignal<f32>,
    set_compound_rot_offset: WriteSignal<f32>,
    compound_scale_factor: ReadSignal<f32>,
    set_compound_scale_factor: WriteSignal<f32>,
) -> impl IntoView {
    let count = ids.len();
    let move_ids: Vec<String> = ids.iter().cloned().collect();
    let layer_ids = move_ids.clone();
    let (cw, ch) = canvas_dims.get_untracked();

    let centroid = layout
        .with_untracked(|l| {
            l.as_ref()
                .and_then(|l| layout_geometry::compound_centroid(l, &ids))
        })
        .unwrap_or(hypercolor_types::spatial::NormalizedPosition::new(0.5, 0.5));
    let cx_px = centroid.x * cw;
    let cy_px = centroid.y * ch;

    let rot_offset = compound_rot_offset.get_untracked();
    let scale_factor = compound_scale_factor.get_untracked();
    let rotation_gesture = StoredValue::new(None::<(SpatialLayout, f32, Vec<Output>)>);

    let screens = layout.with_untracked(|layout| {
        layout.as_ref().map_or(0, |layout| {
            layout
                .zones
                .iter()
                .filter(|output| ids.contains(&output.id) && is_screen_output(output))
                .count()
        })
    });
    let title = selection_count_words(count, screens);
    // How deep the selection reaches into a device's parts, which a
    // selection of screens alone has none of.
    let depth_label = (screens < count).then(|| {
        let depth = compound_depth.get_untracked();
        match depth {
            CompoundDepth::Root => "Device".to_string(),
            CompoundDepth::Device { .. } => "Slot".to_string(),
            CompoundDepth::Slot { ref slot_id, .. } => slot_id.clone(),
        }
    });

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
        <div class="flex w-full min-w-0 flex-wrap items-center gap-x-3 gap-y-2">
            <div class="flex min-w-0 grow basis-28 items-baseline gap-2">
                <span class="shrink-0 text-sm font-medium tabular-nums text-fg-primary" data-selection-title="">
                    {title}
                </span>
                {depth_label.map(|label| view! {
                    <span class="min-w-0 truncate text-xs text-fg-tertiary">{label}</span>
                })}
            </div>

                <div
                    class=GROUP
                >
                    {zone_pixel_input("X", cx_px, "1", 0, cw, {
                        let ids = ids_pos_x;
                        move |px: f32| {
                            let (cw, _) = canvas_dims.get_untracked();
                            let norm_x = (px / cw).clamp(0.0, 1.0);
                            let centroid = layout.with_untracked(|l| {
                                l.as_ref()
                                    .and_then(|l| layout_geometry::compound_centroid(l, &ids))
                            });
                            if let Some(c) = centroid {
                                let target =
                                    hypercolor_types::spatial::NormalizedPosition::new(norm_x, c.y);
                                set_layout.update(|l| {
                                    if let Some(layout) = l {
                                        layout_geometry::translate_compound(layout, &ids, target);
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
                                    .and_then(|l| layout_geometry::compound_centroid(l, &ids))
                            });
                            if let Some(c) = centroid {
                                let target =
                                    hypercolor_types::spatial::NormalizedPosition::new(c.x, norm_y);
                                set_layout.update(|l| {
                                    if let Some(layout) = l {
                                        layout_geometry::translate_compound(layout, &ids, target);
                                    }
                                });
                                set_is_dirty.set(true);
                            }
                        }
                    })}
                    <button
                        class=ICON_BUTTON
                        title="Center group horizontally"
                        on:click=move |_| {
                            let centroid = layout.with_untracked(|l| {
                                l.as_ref()
                                    .and_then(|l| layout_geometry::compound_centroid(l, &ids_center_h))
                            });
                            if let Some(c) = centroid {
                                let target =
                                    hypercolor_types::spatial::NormalizedPosition::new(0.5, c.y);
                                set_layout.update(|l| {
                                    if let Some(layout) = l {
                                        layout_geometry::translate_compound(
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
                        <Icon icon=LuAlignCenterHorizontal width="14px" height="14px" />
                    </button>
                    <button
                        class=ICON_BUTTON
                        title="Center group vertically"
                        on:click=move |_| {
                            let centroid = layout.with_untracked(|l| {
                                l.as_ref()
                                    .and_then(|l| layout_geometry::compound_centroid(l, &ids_center_v))
                            });
                            if let Some(c) = centroid {
                                let target =
                                    hypercolor_types::spatial::NormalizedPosition::new(c.x, 0.5);
                                set_layout.update(|l| {
                                    if let Some(layout) = l {
                                        layout_geometry::translate_compound(
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
                        <Icon icon=LuAlignCenterVertical width="14px" height="14px" />
                    </button>
                </div>

                <div
                    class=GROUP
                >
                    <input
                        type="range"
                        min="-180"
                        max="180"
                        step="1"
                        class="w-20 min-w-0"
                        aria-label="Rotation"
                        title="Rotation"
                        prop:value=format!("{rot_offset:.0}")
                        on:input=move |ev| {
                            let event = Input::from_event(ev);
                            if let Some(new_deg) = event.value::<f32>() {
                                rotation_gesture.update_value(|gesture| {
                                    if gesture.is_none()
                                        && let Some(baseline) = layout.get_untracked()
                                    {
                                        let previous = baseline.zones.clone();
                                        *gesture = Some((
                                            baseline,
                                            compound_rot_offset.get_untracked(),
                                            previous,
                                        ));
                                    }
                                    let Some((baseline, initial_deg, previous)) = gesture.as_mut() else {
                                        return;
                                    };
                                    let mut rotated = baseline.clone();
                                    layout_geometry::rotate_compound(
                                        &mut rotated,
                                        &ids_rot_slider,
                                        (new_deg - *initial_deg).to_radians(),
                                    );
                                    set_layout.update(|current| {
                                        if let Some(current) = current {
                                            current.zones = merge_gesture(
                                                &current.zones,
                                                previous,
                                                &rotated.zones,
                                            );
                                        }
                                    });
                                    // Comparing with the previous preview also restores fields
                                    // when the slider returns to its starting angle.
                                    *previous = rotated.zones;
                                });
                                set_compound_rot_offset.set(new_deg);
                                set_is_dirty.set(true);
                            }
                        }
                        on:change=move |_| rotation_gesture.set_value(None)
                        on:blur=move |_| rotation_gesture.set_value(None)
                        on:pointercancel=move |_| rotation_gesture.set_value(None)
                    />
                    <div class=FIELD_BOX>
                        <input
                            type="number"
                            min="-360"
                            max="360"
                            step="1"
                            class=format!("{FIELD_INPUT} w-9 text-right")
                            prop:value=format!("{rot_offset:.0}")
                            on:change=move |ev| {
                                let event = Change::from_event(ev);
                                if let Some(new_deg) = event.value::<f32>() {
                                    let old_deg = compound_rot_offset.get_untracked();
                                    let delta_rad = (new_deg - old_deg).to_radians();
                                    set_compound_rot_offset.set(new_deg);
                                    set_layout.update(|l| {
                                        if let Some(layout) = l {
                                            layout_geometry::rotate_compound(
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
                        <span class=FIELD_AFFIX>{"\u{00b0}"}</span>
                    </div>
                </div>

                <div
                    class=GROUP
                >
                    <input
                        type="range"
                        min="0.2"
                        max="3.0"
                        step="0.05"
                        class="w-20 min-w-0"
                        aria-label="Scale"
                        title="Scale"
                        prop:value=format!("{scale_factor:.2}")
                        on:input=move |ev| {
                            let event = Input::from_event(ev);
                            if let Some(new_factor) = event.value::<f32>() {
                                let old_factor = compound_scale_factor.get_untracked();
                                if old_factor.abs() > 0.001 {
                                    let ratio = new_factor / old_factor;
                                    set_compound_scale_factor.set(new_factor);
                                    set_layout.update(|l| {
                                        if let Some(layout) = l {
                                            layout_geometry::scale_compound(
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
                    <div class=FIELD_BOX>
                        <input
                            type="number"
                            min="0.2"
                            max="3.0"
                            step="0.05"
                            class=format!("{FIELD_INPUT} w-9 text-right")
                            prop:value=format!("{scale_factor:.2}")
                            on:change=move |ev| {
                                let event = Change::from_event(ev);
                                if let Some(new_factor) = event.value::<f32>() {
                                    let old_factor = compound_scale_factor.get_untracked();
                                    if old_factor.abs() > 0.001 {
                                        let ratio = new_factor / old_factor;
                                        set_compound_scale_factor.set(new_factor);
                                        set_layout.update(|l| {
                                            if let Some(layout) = l {
                                                layout_geometry::scale_compound(
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
                        <span class=FIELD_AFFIX>{"\u{00d7}"}</span>
                    </div>
                </div>

                <div
                    class=OPS
                >
                    <span class=format!("{GROUP_LABEL} pr-1")>"Align"</span>
                    {compound_op_button(LuAlignStartVertical, "Align left edges", move || {
                        let ids = ids_align_left.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_compound(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                    layout_geometry::AlignAnchor::Min,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {compound_op_button(LuAlignCenterVertical, "Align horizontal centers", move || {
                        let ids = ids_align_hc.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_compound(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                    layout_geometry::AlignAnchor::Center,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {compound_op_button(LuAlignEndVertical, "Align right edges", move || {
                        let ids = ids_align_right.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_compound(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                    layout_geometry::AlignAnchor::Max,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    <div class="mx-0.5 h-4 w-px bg-edge-subtle" />
                    {compound_op_button(LuAlignStartHorizontal, "Align top edges", move || {
                        let ids = ids_align_top.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_compound(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::Y,
                                    layout_geometry::AlignAnchor::Min,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {compound_op_button(LuAlignCenterHorizontal, "Align vertical centers", move || {
                        let ids = ids_align_vc.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_compound(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::Y,
                                    layout_geometry::AlignAnchor::Center,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {compound_op_button(LuAlignEndHorizontal, "Align bottom edges", move || {
                        let ids = ids_align_bottom.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::align_compound(
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
                    class=OPS
                    class=("opacity-40", !dist_enabled)
                    class=("pointer-events-none", !dist_enabled)
                    title=move || if dist_enabled {
                        ""
                    } else {
                        "Distribute needs 3+ outputs"
                    }
                >
                    <span class=format!("{GROUP_LABEL} pr-1")>"Distribute"</span>
                    {compound_op_button(LuAlignHorizontalDistributeCenter, "Distribute horizontally (even gaps)", move || {
                        let ids = ids_dist_h.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::distribute_compound(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {compound_op_button(LuAlignVerticalDistributeCenter, "Distribute vertically (even gaps)", move || {
                        let ids = ids_dist_v.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::distribute_compound(
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
                    class=OPS
                >
                    <span class=format!("{GROUP_LABEL} pr-1")>"Pack"</span>
                    {compound_op_button(LuFoldHorizontal, "Pack horizontally (no gaps)", move || {
                        let ids = ids_pack_h.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::pack_compound(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {compound_op_button(LuFoldVertical, "Pack vertically (no gaps)", move || {
                        let ids = ids_pack_v.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::pack_compound(
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
                    class=OPS
                >
                    <span class=format!("{GROUP_LABEL} pr-1")>"Mirror"</span>
                    {compound_op_button(LuFlipHorizontal, "Mirror across vertical axis", move || {
                        let ids = ids_mirror_h.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::mirror_compound(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::X,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                    {compound_op_button(LuFlipVertical, "Mirror across horizontal axis", move || {
                        let ids = ids_mirror_v.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                layout_geometry::mirror_compound(
                                    layout,
                                    &ids,
                                    layout_geometry::AlignAxis::Y,
                                );
                            }
                        });
                        set_is_dirty.set(true);
                    })}
                </div>

                <div class="flex shrink-0 items-center gap-0.5">
                    <OutputLayerButton output_ids=layer_ids />
                    <OutputMoveButton output_ids=move_ids />
                </div>
        </div>
    }
}
