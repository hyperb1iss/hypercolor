//! Per-panel wrapper — drag handle, width cycle, hide button.
//!
//! Each dashboard stats panel is rendered inside a `PanelFrame`. The
//! frame itself is a CSS-grid cell with a `xl:col-span-*` class derived
//! from the panel's current [`PanelWidth`], plus a floating control bar
//! in the top-right that fades in on hover.
//!
//! Drag-and-drop uses the HTML5 DnD API. The grip element is the
//! `draggable` source: dragstart stashes the source panel's index in
//! `dataTransfer`, and every frame listens for dragover (to mark itself
//! as the active drop target) and drop (to fire the `on_drop` callback
//! with the source + target indices). The parent signals a new layout
//! by calling `DashboardLayout::move_panel` and persisting.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::icons::*;

use super::layout::{PanelId, PanelWidth};

/// Wraps a dashboard stats panel with drag, width-cycle, and hide
/// controls. The children are the panel's existing `view!` output; the
/// frame only adds chrome, no padding or border that would conflict
/// with the panel's own card styling.
#[component]
pub fn PanelFrame(
    panel_id: PanelId,
    width: PanelWidth,
    index: usize,
    /// Signal of the currently-dragging panel index, or `None`. Used to
    /// dim the source and highlight valid drop targets while a drag is
    /// in flight. Shared across all frames so they can coordinate.
    drag_source: RwSignal<Option<usize>>,
    on_drop: Callback<(usize, usize)>,
    on_cycle_width: Callback<PanelId>,
    on_hide: Callback<PanelId>,
    children: Children,
) -> impl IntoView {
    let (is_drop_target, set_is_drop_target) = signal(false);
    let is_source = Memo::new(move |_| drag_source.get() == Some(index));
    let is_drag_active = Memo::new(move |_| drag_source.get().is_some());

    let span_class = width.xl_col_span_class();
    let width_label = width.label();

    view! {
        <div
            class=move || {
                let base =
                    "relative group/panel col-span-6 transition-all duration-150 panel-frame";
                let state = if is_source.get() {
                    " opacity-40 scale-[0.98]"
                } else if is_drop_target.get() {
                    " ring-2 ring-accent/60 rounded-xl"
                } else {
                    ""
                };
                let dim = if is_drag_active.get() && !is_source.get() {
                    " cursor-copy"
                } else {
                    ""
                };
                format!("{base} {span_class}{state}{dim}")
            }
            on:dragenter=move |_| {
                if drag_source.get_untracked().is_some() {
                    set_is_drop_target.set(true);
                }
            }
            on:dragleave=move |_| set_is_drop_target.set(false)
            on:dragover=move |ev: ev::DragEvent| {
                // Required to signal "yes, drop here is acceptable".
                ev.prevent_default();
                if let Some(dt) = ev.data_transfer() {
                    dt.set_drop_effect("move");
                }
            }
            on:drop=move |ev: ev::DragEvent| {
                ev.prevent_default();
                set_is_drop_target.set(false);
                let Some(dt) = ev.data_transfer() else { return };
                let Ok(raw) = dt.get_data("text/plain") else { return };
                let Ok(from) = raw.parse::<usize>() else { return };
                if from != index {
                    on_drop.run((from, index));
                }
            }
        >
            // Floating control bar — sits above the panel's own content,
            // fades in on hover so it never competes with the chart
            // visuals at rest.
            <div class="absolute top-2 right-2 z-20 flex items-center gap-1 \
                        opacity-0 group-hover/panel:opacity-100 \
                        transition-opacity duration-200 pointer-events-none">
                // Width cycle
                <button
                    type="button"
                    class="pointer-events-auto p-1.5 rounded-md bg-black/55 backdrop-blur-sm \
                           border border-white/8 text-fg-tertiary hover:text-fg-primary \
                           hover:bg-black/75 hover:border-accent-muted transition-all"
                    title=format!("Width: {width_label} · click to cycle")
                    on:click=move |_| on_cycle_width.run(panel_id)
                >
                    <Icon icon=LuColumns3 width="11px" height="11px" />
                </button>

                // Hide
                <button
                    type="button"
                    class="pointer-events-auto p-1.5 rounded-md bg-black/55 backdrop-blur-sm \
                           border border-white/8 text-fg-tertiary hover:text-fg-primary \
                           hover:bg-black/75 hover:border-accent-muted transition-all"
                    title="Hide panel"
                    on:click=move |_| on_hide.run(panel_id)
                >
                    <Icon icon=LuEyeOff width="11px" height="11px" />
                </button>

                // Drag handle — the only `draggable` element. Dragging
                // the grip initiates the move; the whole frame is the
                // drop target so users get generous hit areas.
                <div
                    class="pointer-events-auto p-1.5 rounded-md bg-black/55 backdrop-blur-sm \
                           border border-white/8 text-fg-tertiary hover:text-fg-primary \
                           hover:bg-black/75 hover:border-accent-muted \
                           cursor-grab active:cursor-grabbing transition-all"
                    draggable="true"
                    title="Drag to reorder"
                    on:dragstart=move |ev: ev::DragEvent| {
                        if let Some(dt) = ev.data_transfer() {
                            let _ = dt.set_data("text/plain", &index.to_string());
                            dt.set_effect_allowed("move");
                        }
                        drag_source.set(Some(index));
                    }
                    on:dragend=move |_| {
                        drag_source.set(None);
                        set_is_drop_target.set(false);
                    }
                >
                    <Icon icon=LuGripVertical width="11px" height="11px" />
                </div>
            </div>

            {children()}
        </div>
    }
}
