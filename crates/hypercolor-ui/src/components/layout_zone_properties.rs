//! The output properties bar under the Studio canvas: what is selected,
//! its position, size, rotation, and scale, and what can be done to it.

use hypercolor_leptos_ext::events::{Change, Input};
use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::app::DevicesContext;
use crate::async_helpers::spawn_identify;
use crate::icons::*;
use crate::layout_geometry::{self, SizeAxis};
use crate::pages::studio::move_menu::OutputMoveButton;

mod compound_panel;

use compound_panel::CompoundZoneProperties;

/// Zone properties editor (bottom panel of layout builder).
#[component]
pub fn LayoutZoneProperties() -> impl IntoView {
    let editor = expect_context::<crate::components::layout_builder::LayoutEditorContext>();
    let layout = editor.layout;
    let selected_zone_ids = editor.selected_zone_ids;
    let keep_aspect_ratio = editor.keep_aspect_ratio;
    let set_layout = editor.set_layout;
    let set_keep_aspect_ratio = editor.set_keep_aspect_ratio;
    let set_selected_zone_ids = editor.set_selected_zone_ids;
    let set_is_dirty = editor.set_is_dirty;
    let compound_depth = editor.compound_depth;
    let zone_display_ctx =
        expect_context::<crate::components::layout_builder::LayoutZoneDisplayContext>();
    let studio_history = expect_context::<crate::pages::studio::history::StudioHistory>();

    // Canvas pixel dimensions for display conversion
    let canvas_dims = Signal::derive(move || {
        layout.with(|current| {
            current
                .as_ref()
                .map(|l| (l.canvas_width.max(1) as f32, l.canvas_height.max(1) as f32))
                .unwrap_or_else(|| {
                    let (w, h) = crate::render_canvas::DEFAULT_RENDER_CANVAS;
                    (w as f32, h as f32)
                })
        })
    });

    // ── Group transform state (accumulated deltas, reset on selection change) ──
    let (compound_rot_offset, set_compound_rot_offset) = signal(0.0f32);
    let (compound_scale_factor, set_compound_scale_factor) = signal(1.0f32);
    // Track the previous selection set to detect changes
    let (prev_selection, set_prev_selection) = signal(std::collections::HashSet::<String>::new());

    // Derive selected zone snapshot for display (single-selection only for Phase 1)
    let zone_snapshot = Signal::derive(move || {
        let ids = selected_zone_ids.get();
        if ids.len() != 1 {
            return None;
        }
        let id = ids.iter().next()?;
        layout.with(|current| {
            let l = current.as_ref()?;
            let suppressed = crate::layout_utils::suppressed_attachment_source_zone_ids(l);
            if suppressed.contains(id) {
                return None;
            }
            l.zones.iter().find(|z| z.id == *id).cloned()
        })
    });

    // Helper to update a zone field
    let update_zone =
        move |zone_id: String, updater: Box<dyn FnOnce(&mut hypercolor_types::spatial::Output)>| {
            set_layout.update(|l| {
                if let Some(layout) = l
                    && let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id)
                {
                    updater(zone);
                    let (canvas_w, canvas_h) = (layout.canvas_width, layout.canvas_height);
                    zone.size = layout_geometry::normalize_zone_size_for_editor(
                        zone.size,
                        &zone.topology,
                        zone.shape.as_ref(),
                        layout_geometry::canvas_pixel_aspect(canvas_w, canvas_h),
                    );
                }
            });
            set_is_dirty.set(true);
        };

    let update_zone_rotation = move |zone_id: String, rotation_radians: f32| {
        set_layout.update(|l| {
            if let Some(layout) = l {
                let changed =
                    layout_geometry::set_zone_rotation(layout, &zone_id, rotation_radians);
                if changed && let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                    let (canvas_w, canvas_h) = (layout.canvas_width, layout.canvas_height);
                    zone.size = layout_geometry::normalize_zone_size_for_editor(
                        zone.size,
                        &zone.topology,
                        zone.shape.as_ref(),
                        layout_geometry::canvas_pixel_aspect(canvas_w, canvas_h),
                    );
                }
            }
        });
        set_is_dirty.set(true);
    };

    let move_zone_to_position =
        move |zone_id: String, desired_position: hypercolor_types::spatial::NormalizedPosition| {
            set_layout.update(|l| {
                if let Some(layout) = l {
                    let _ = layout_geometry::set_zone_position(layout, &zone_id, desired_position);
                }
            });
            set_is_dirty.set(true);
        };

    // A slider drag is one edit, not one per pixel: the press on a range
    // input opens an interaction and the release (anywhere) closes it, so
    // undo steps back over the whole drag. Keyboard nudges on a slider
    // still record per step, which is what a keyboard user expects.
    let slider_interaction = StoredValue::new(false);
    let close_slider_interaction = move || {
        if slider_interaction.get_value() {
            slider_interaction.set_value(false);
            set_layout.finish_interaction();
        }
    };
    let release = window_event_listener(leptos::ev::pointerup, move |_| close_slider_interaction());
    // A cancelled pointer (touch pan, window switch) never sends pointerup,
    // so it must close the interaction too or history stays suspended.
    let cancel = window_event_listener(leptos::ev::pointercancel, move |_| {
        close_slider_interaction()
    });
    on_cleanup(move || {
        release.remove();
        cancel.remove();
    });

    view! {
        <div
            class="@container flex min-h-12 items-center px-4 py-2 max-md:px-3"
            on:pointerdown=move |ev: web_sys::PointerEvent| {
                let is_range = ev
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
                    .is_some_and(|input| input.type_() == "range");
                if is_range {
                    slider_interaction.set_value(true);
                    set_layout.begin_interaction();
                }
            }
        >
            {move || {
                let ids = selected_zone_ids.get();
                if ids.len() > 1 {
                    // Reset accumulated deltas when selection set changes
                    let prev = prev_selection.get_untracked();
                    if prev != ids {
                        set_prev_selection.set(ids.clone());
                        set_compound_rot_offset.set(0.0);
                        set_compound_scale_factor.set(1.0);
                    }

                    return view! {
                        <CompoundZoneProperties
                            ids=ids
                            layout=layout
                            canvas_dims=canvas_dims
                            set_layout=set_layout
                            set_is_dirty=set_is_dirty
                            compound_depth=compound_depth
                            compound_rot_offset=compound_rot_offset
                            set_compound_rot_offset=set_compound_rot_offset
                            compound_scale_factor=compound_scale_factor
                            set_compound_scale_factor=set_compound_scale_factor
                        />
                    }.into_any();
                }
                let Some(zone) = zone_snapshot.get() else {
                    return view! {
                        <div class="flex min-w-0 items-center gap-2 text-fg-tertiary">
                            <Icon icon=LuMousePointerClick width="14px" height="14px" />
                            <span class="truncate text-xs">"Select an output on the canvas to place it"</span>
                        </div>
                    }.into_any();
                };

                let ctx = expect_context::<DevicesContext>();
                let devices = ctx
                    .devices_resource
                    .get_untracked()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                let attachment_profiles = zone_display_ctx.attachment_profiles.get();
                let zone_display =
                    crate::layout_utils::effective_zone_display(&zone, &devices, &attachment_profiles);

                let zone_id = zone.id.clone();
                // The rail's short name fits the bar; the stored one, with
                // its device in front, is a hover away.
                let rail_name = crate::layout_utils::rail_output_name(&zone, &devices);
                let zone_title = zone_display.label.clone();
                let detail_lead = rail_name
                    .as_ref()
                    .and_then(|rail| rail.card.clone())
                    .or_else(|| zone.zone_name.clone());
                let zone_name = rail_name.map_or_else(|| zone_title.clone(), |rail| rail.name);
                let (cw, ch) = canvas_dims.get_untracked();
                let transform_anchor = layout.with_untracked(|current| {
                    current.as_ref().and_then(|layout| {
                        layout_geometry::zone_transform_anchor(layout, &zone.id)
                    })
                }).unwrap_or(zone.position);
                let pos_x_px = transform_anchor.x * cw;
                let pos_y_px = transform_anchor.y * ch;
                let size_w_px = zone.size.x * cw;
                let size_h_px = zone.size.y * ch;
                let rotation_deg = zone.rotation.to_degrees();
                let scale = zone.scale;
                let meta_line = meta_line(
                    detail_lead.as_deref(),
                    topology_name(&zone.topology),
                    zone.topology.led_count(),
                );
                let identify_target = zone_display.identify_target.clone();
                let reset_device_name =
                    crate::layout_utils::effective_device_name(&zone.device_id, &devices)
                        .unwrap_or_else(|| zone.device_id.clone());

                let zid_pos_x = zone_id.clone();
                let zid_pos_y = zone_id.clone();
                let zid_center_h = zone_id.clone();
                let zid_center_v = zone_id.clone();
                let zid_size_w = zone_id.clone();
                let zid_size_h = zone_id.clone();
                let zid_rotation = zone_id.clone();
                let zid_rotation_input = zone_id.clone();
                let zid_scale_input = zone_id.clone();
                let identify_action = identify_target.clone();
                let zid_reset_defaults = zone_id.clone();
                let reset_device_id = zone.device_id.clone();
                let reset_zone_name = zone.zone_name.clone();
                let move_ids = vec![zone_id.clone()];
                let zid_remove = zone_id;

                view! {
                    <div class="flex w-full min-w-0 flex-wrap items-center gap-x-3 gap-y-2">
                        // What is selected. A canvas edit saves geometry only,
                        // so the name and channel are shown, not edited.
                        // The name has the identity block to itself until
                        // the bar is wide enough for the detail line too;
                        // sharing a narrow block cut both to a few letters.
                        // On a phone the block gives up its width so the
                        // name shares the first row with position: a row
                        // less of bar is a row more of canvas.
                        <div class="flex min-w-0 grow basis-32 items-baseline gap-2 @max-md:basis-16">
                            <span class="min-w-0 max-w-56 truncate text-sm font-medium text-fg-primary" title=zone_title>
                                {zone_name}
                            </span>
                            <span class="hidden min-w-0 truncate text-xs tabular-nums text-fg-tertiary @6xl:inline">
                                {meta_line}
                            </span>
                        </div>

                            // Position
                            <div class=GROUP>
                                {zone_pixel_input("X", pos_x_px, "1", 0, cw, {
                                    let zid = zid_pos_x;
                                    move |px: f32| {
                                        let (cw, _) = canvas_dims.get_untracked();
                                        let norm = (px / cw).clamp(0.0, 1.0);
                                        let anchor = layout.with_untracked(|current| {
                                            current.as_ref().and_then(|layout| {
                                                layout_geometry::zone_transform_anchor(layout, &zid)
                                            })
                                        });
                                        let Some(anchor) = anchor else {
                                            return;
                                        };
                                        move_zone_to_position(
                                            zid.clone(),
                                            hypercolor_types::spatial::NormalizedPosition::new(norm, anchor.y),
                                        );
                                    }
                                })}
                                {zone_pixel_input("Y", pos_y_px, "1", 0, ch, {
                                    let zid = zid_pos_y;
                                    move |px: f32| {
                                        let (_, ch) = canvas_dims.get_untracked();
                                        let norm = (px / ch).clamp(0.0, 1.0);
                                        let anchor = layout.with_untracked(|current| {
                                            current.as_ref().and_then(|layout| {
                                                layout_geometry::zone_transform_anchor(layout, &zid)
                                            })
                                        });
                                        let Some(anchor) = anchor else {
                                            return;
                                        };
                                        move_zone_to_position(
                                            zid.clone(),
                                            hypercolor_types::spatial::NormalizedPosition::new(anchor.x, norm),
                                        );
                                    }
                                })}
                                <button
                                    class=ICON_BUTTON
                                    title="Center horizontally"
                                    on:click=move |_| {
                                        let zid = zid_center_h.clone();
                                        let anchor = layout.with_untracked(|current| {
                                            current.as_ref().and_then(|layout| {
                                                layout_geometry::zone_transform_anchor(layout, &zid)
                                            })
                                        });
                                        if let Some(anchor) = anchor {
                                            move_zone_to_position(
                                                zid,
                                                hypercolor_types::spatial::NormalizedPosition::new(0.5, anchor.y),
                                            );
                                        }
                                    }
                                >
                                    <Icon icon=LuAlignCenterHorizontal width="14px" height="14px" />
                                </button>
                                <button
                                    class=ICON_BUTTON
                                    title="Center vertically"
                                    on:click=move |_| {
                                        let zid = zid_center_v.clone();
                                        let anchor = layout.with_untracked(|current| {
                                            current.as_ref().and_then(|layout| {
                                                layout_geometry::zone_transform_anchor(layout, &zid)
                                            })
                                        });
                                        if let Some(anchor) = anchor {
                                            move_zone_to_position(
                                                zid,
                                                hypercolor_types::spatial::NormalizedPosition::new(anchor.x, 0.5),
                                            );
                                        }
                                    }
                                >
                                    <Icon icon=LuAlignCenterVertical width="14px" height="14px" />
                                </button>
                            </div>

                            // Size
                            <div class=GROUP>
                                {zone_pixel_input("W", size_w_px, "1", 0, cw, {
                                    let zid = zid_size_w;
                                    move |px: f32| {
                                        let (cw, _) = canvas_dims.get_untracked();
                                        let norm = (px / cw).clamp(0.0, 1.0);
                                        let locked = keep_aspect_ratio.get_untracked();
                                        update_zone(zid.clone(), Box::new(move |z| {
                                            z.size = layout_geometry::update_zone_size(
                                                z.size, SizeAxis::Width, norm, locked,
                                            );
                                        }))
                                    }
                                })}
                                {zone_pixel_input("H", size_h_px, "1", 0, ch, {
                                    let zid = zid_size_h;
                                    move |px: f32| {
                                        let (_, ch) = canvas_dims.get_untracked();
                                        let norm = (px / ch).clamp(0.0, 1.0);
                                        let locked = keep_aspect_ratio.get_untracked();
                                        update_zone(zid.clone(), Box::new(move |z| {
                                            z.size = layout_geometry::update_zone_size(
                                                z.size, SizeAxis::Height, norm, locked,
                                            );
                                        }))
                                    }
                                })}
                                <button
                                    class=ICON_BUTTON
                                    class=("text-accent!", move || keep_aspect_ratio.get())
                                    class=("bg-accent/10", move || keep_aspect_ratio.get())
                                    aria-pressed=move || keep_aspect_ratio.get().to_string()
                                    title=move || if keep_aspect_ratio.get() { "Aspect ratio linked" } else { "Aspect ratio free" }
                                    on:click=move |_| {
                                        set_keep_aspect_ratio.update(|locked| *locked = !*locked);
                                    }
                                >
                                    {move || if keep_aspect_ratio.get() {
                                        view! { <Icon icon=LuLink width="14px" height="14px" /> }.into_any()
                                    } else {
                                        view! { <Icon icon=LuUnlink width="14px" height="14px" /> }.into_any()
                                    }}
                                </button>
                            </div>

                            // Rotation
                            <div class=GROUP>
                                <input
                                    type="range"
                                    min="0" max="360" step="1"
                                    class="w-16 min-w-0"
                                    aria-label="Rotation"
                                    title="Rotation"
                                    prop:value=format!("{rotation_deg:.0}")
                                    on:input=move |ev| {
                                        let event = Input::from_event(ev);
                                        if let Some(deg) = event.value::<f32>() {
                                            let rad = deg.to_radians();
                                            let zid = zid_rotation.clone();
                                            update_zone_rotation(zid, rad);
                                        }
                                    }
                                />
                                <div class=FIELD_BOX>
                                    <input
                                        type="number"
                                        min="0" max="360" step="1"
                                        aria-label="Rotation in degrees"
                                        class=format!("{FIELD_INPUT} w-9 text-right")
                                        prop:value=format!("{rotation_deg:.0}")
                                        on:change=move |ev| {
                                            let event = Change::from_event(ev);
                                            if let Some(deg) = event.value::<f32>() {
                                                let rad = deg.to_radians();
                                                let zid = zid_rotation_input.clone();
                                                update_zone_rotation(zid, rad);
                                            }
                                        }
                                    />
                                    <span class=FIELD_AFFIX>{"\u{00b0}"}</span>
                                </div>
                            </div>

                            // Scale
                            <div class=GROUP>
                                <div class=FIELD_BOX>
                                    <input
                                        type="number"
                                        min="0.5" max="3.0" step="0.1"
                                        aria-label="Scale factor"
                                        class=format!("{FIELD_INPUT} w-9 text-right")
                                        prop:value=format!("{scale:.1}")
                                        on:change=move |ev| {
                                            let event = Change::from_event(ev);
                                            if let Some(s) = event.value::<f32>() {
                                                let zid = zid_scale_input.clone();
                                                update_zone(zid, Box::new(move |z| z.scale = s));
                                            }
                                        }
                                    />
                                    <span class=FIELD_AFFIX>{"\u{00d7}"}</span>
                                </div>
                            </div>

                            // Zone actions — destructive separated by divider
                            <div class="flex shrink-0 items-center gap-0.5">
                                <OutputMoveButton output_ids=move_ids />
                                {identify_action.map(|target| view! {
                                    <button
                                        class=ICON_BUTTON
                                        title="Identify output"
                                        on:click=move |_| match target.clone() {
                                            crate::layout_utils::OutputIdentifyTarget::Segment { device_id, segment } => {
                                                spawn_identify(
                                                    "output",
                                                    async move { crate::api::identify_segment(&device_id, &segment).await },
                                                );
                                            }
                                            crate::layout_utils::OutputIdentifyTarget::Attachment {
                                                device_id,
                                                slot_id,
                                                binding_index,
                                                instance,
                                            } => {
                                                spawn_identify(
                                                    "component",
                                                    async move {
                                                        crate::api::identify_attachment(
                                                            &device_id,
                                                            &slot_id,
                                                            binding_index,
                                                            instance,
                                                        )
                                                        .await
                                                    },
                                                );
                                            }
                                        }
                                    >
                                        <Icon icon=LuZap width="14px" height="14px" />
                                    </button>
                                })}
                                <button
                                    class=ICON_BUTTON
                                    title="Reset output to defaults"
                                    on:click=move |_| {
                                        let zid = zid_reset_defaults.clone();
                                        let did = reset_device_id.clone();
                                        let zn = reset_zone_name.clone();
                                        let dname = reset_device_name.clone();
                                        let zone_summary: Option<crate::api::SegmentSummary> = ctx
                                            .devices_resource
                                            .get_untracked()
                                            .and_then(|r| r.ok())
                                            .and_then(|devices| {
                                                devices.iter()
                                                    .find(|d| d.layout_device_id == did)
                                                    .and_then(|d| {
                                                        zn.as_ref().and_then(|name| {
                                                            d.segments
                                                                .iter()
                                                                .find(|z| z.name == *name)
                                                                .cloned()
                                                        })
                                                    })
                                            });
                                        let total_leds = ctx
                                            .devices_resource
                                            .get_untracked()
                                            .and_then(|r| r.ok())
                                            .and_then(|devices| devices.iter().find(|d| d.layout_device_id == did).map(|d| d.total_leds as usize))
                                            .unwrap_or(1);
                                        let (canvas_width, canvas_height) = layout.with_untracked(|current| {
                                            current.as_ref()
                                                .map(|l| (l.canvas_width.max(1), l.canvas_height.max(1)))
                                                .unwrap_or(crate::render_canvas::DEFAULT_RENDER_CANVAS)
                                        });
                                        let defaults = crate::layout_geometry::default_zone_visuals(
                                            &dname,
                                            zone_summary.as_ref(),
                                            total_leds,
                                            canvas_width,
                                            canvas_height,
                                        );
                                        set_layout.update(|l| {
                                            if let Some(layout) = l
                                                && let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zid) {
                                                    zone.position = hypercolor_types::spatial::NormalizedPosition::new(0.5, 0.5);
                                                    zone.size = crate::layout_geometry::normalize_zone_size_for_editor(
                                                        defaults.size,
                                                        &defaults.topology,
                                                        defaults.shape.as_ref(),
                                                        crate::layout_geometry::canvas_pixel_aspect(canvas_width, canvas_height),
                                                    );
                                                    zone.rotation = 0.0;
                                                    zone.scale = 1.0;
                                                }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                >
                                    <Icon icon=LuRotateCcw width="14px" height="14px" />
                                </button>
                                <div class="mx-1 h-4 w-px bg-edge-subtle" />
                                // Membership, not geometry: the output leaves the zone
                                // through the same undoable edit the pane uses.
                                <button
                                    class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-status-error/70 transition-colors btn-press hover:bg-status-error/10 hover:text-status-error disabled:opacity-40"
                                    title="Remove output from zone"
                                    disabled=move || studio_history.busy.get()
                                    on:click=move |_| {
                                        let zid = zid_remove.clone();
                                        let Some(zone_id) = layout.with_untracked(|l| l.as_ref().map(|l| l.id.clone())) else {
                                            return;
                                        };
                                        studio_history.remove_outputs(&zone_id, &[zid]);
                                        set_selected_zone_ids.set(std::collections::HashSet::new());
                                        editor.set_compound_depth.set(crate::compound_selection::CompoundDepth::Root);
                                    }
                                >
                                    <Icon icon=LuTrash2 width="14px" height="14px" />
                                </button>
                            </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

/// A cluster of related controls in the bar; clusters wrap as units.
pub(super) const GROUP: &str = "flex shrink-0 items-center gap-1.5";
/// The word in front of a cluster that needs one.
pub(super) const GROUP_LABEL: &str = "shrink-0 select-none text-[11px] text-fg-tertiary";
/// The box around a number: its letter, the value, and its unit.
pub(super) const FIELD_BOX: &str = "flex h-7 shrink-0 items-center gap-1.5 rounded-md border border-edge-subtle \
     bg-surface-sunken px-2 transition-colors focus-within:border-accent-muted";
/// The letter or unit beside a number, inside its box.
pub(super) const FIELD_AFFIX: &str = "shrink-0 select-none font-mono text-[11px] text-fg-tertiary";
/// The number itself. The box draws the focus ring.
pub(super) const FIELD_INPUT: &str = "prop-number min-w-0 bg-transparent font-mono text-xs tabular-nums text-fg-primary focus:outline-none";
/// A square icon button at the bar's control height.
pub(super) const ICON_BUTTON: &str = "flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-fg-tertiary \
     transition-colors btn-press hover:bg-surface-overlay hover:text-fg-primary disabled:opacity-40";

/// Icon-only button for group align/distribute/pack/mirror operations.
fn compound_op_button(
    icon: icondata_core::Icon,
    title: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <button class=ICON_BUTTON title=title aria-label=title on:click=move |_| on_click()>
            <Icon icon=icon width="14px" height="14px" />
        </button>
    }
}

/// A pixel value in its box, led by the letter that names it.
fn zone_pixel_input(
    label: &'static str,
    value_px: f32,
    step: &'static str,
    precision: usize,
    max_px: f32,
    on_change: impl Fn(f32) + Clone + 'static,
) -> impl IntoView {
    let max_str = format!("{max_px:.0}");
    view! {
        <label class=FIELD_BOX>
            <span class=FIELD_AFFIX>{label}</span>
            <input
                type="number"
                step=step
                min="0"
                max=max_str
                class=format!("{FIELD_INPUT} w-9")
                prop:value=format!("{value_px:.precision$}")
                on:change=move |ev| {
                    let on_change = on_change.clone();
                    let event = Change::from_event(ev);
                    if let Some(v) = event.value::<f32>() {
                        on_change(v);
                    }
                }
            />
        </label>
    }
}

/// What the selection is, in words: where it sits, its shape, its LEDs.
/// `lead` is whichever of the channel and the device the name beside it
/// does not already say.
#[must_use]
pub fn meta_line(lead: Option<&str>, topology: &str, led_count: u32) -> String {
    let leds = if led_count == 1 { "LED" } else { "LEDs" };
    let shape = format!("{topology} \u{00b7} {led_count} {leds}");
    match lead.map(str::trim).filter(|lead| !lead.is_empty()) {
        Some(lead) => format!("{lead} \u{00b7} {shape}"),
        None => shape,
    }
}

/// Human-readable topology name.
fn topology_name(topology: &hypercolor_types::spatial::LedTopology) -> &'static str {
    match topology {
        hypercolor_types::spatial::LedTopology::Strip { .. } => "Strip",
        hypercolor_types::spatial::LedTopology::Matrix { .. } => "Matrix",
        hypercolor_types::spatial::LedTopology::Ring { .. } => "Ring",
        hypercolor_types::spatial::LedTopology::ConcentricRings { .. } => "Concentric Rings",
        hypercolor_types::spatial::LedTopology::PerimeterLoop { .. } => "Perimeter Loop",
        hypercolor_types::spatial::LedTopology::Point => "Point",
        hypercolor_types::spatial::LedTopology::Custom { .. } => "Custom",
    }
}
