use std::cell::Cell;
use std::collections::HashMap;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::layout_geometry::{self, ResizeHandle};
use hypercolor_types::spatial::{NormalizedPosition, Output, SpatialLayout, ZoneShape};

/// Drag/resize runtime — non-reactive state machine for an in-flight pointer
/// interaction. Owns cached DOM refs, the immutable base snapshot, and a
/// running mutable copy of the dragged zones. The RAF scheduler reads from
/// `pending_mouse`, computes the new geometry, and writes results directly
/// to the cached `HtmlElement`s without going through the layout signal.
pub(super) struct DragRuntime {
    pub(super) kind: InteractionKind,
    pub(super) current_zones: Vec<Output>,
    /// `data-zone-id` → element. Captured at interaction start so the RAF
    /// loop never has to query the DOM.
    pub(super) elements: HashMap<String, web_sys::HtmlElement>,
    /// Latest pointer position (normalized to the viewport rect) waiting to
    /// be processed by the next RAF tick.
    pub(super) pending_mouse: Cell<Option<NormalizedPosition>>,
    /// Have any frames been processed yet for this interaction?
    /// Tracks whether we've actually mutated zones so mouseup can decide
    /// between a no-op release and a real commit.
    pub(super) moved: Cell<bool>,
    /// Last preview push timestamp (browser monotonic ms) for throttling.
    pub(super) last_preview_push_ms: Cell<f64>,
}

pub(super) enum InteractionKind {
    Drag {
        primary_zone_id: String,
        offset_x: f32,
        offset_y: f32,
        initial_positions: Vec<(String, NormalizedPosition)>,
    },
    Resize {
        zone_id: String,
        handle: ResizeHandle,
        start_mouse: NormalizedPosition,
        start_center: NormalizedPosition,
        start_size: NormalizedPosition,
        rotation: f32,
        keep_aspect_ratio: bool,
    },
}

impl DragRuntime {
    /// Apply the latest pending pointer position to the in-flight zone copy
    /// and paint the affected elements directly. Returns true if any zone
    /// position/size actually changed this frame.
    pub(super) fn step(&mut self) -> bool {
        let Some(mouse) = self.pending_mouse.take() else {
            return false;
        };
        // Run the geometry math against an owned `SpatialLayout` borrowed
        // out of `current_zones`, then move the (possibly mutated) zones
        // back. We never clone the zone vec on the hot path.
        let mut working = SpatialLayoutShim {
            zones: std::mem::take(&mut self.current_zones),
        }
        .into_layout();
        let changed = match &self.kind {
            InteractionKind::Drag {
                primary_zone_id,
                offset_x,
                offset_y,
                initial_positions,
            } => {
                if initial_positions.len() > 1 {
                    let primary_initial = initial_positions
                        .iter()
                        .find(|(id, _)| id == primary_zone_id)
                        .map(|(_, pos)| *pos)
                        .unwrap_or(NormalizedPosition::new(0.5, 0.5));
                    let desired_primary = NormalizedPosition::new(
                        (mouse.x - offset_x).clamp(0.0, 1.0),
                        (mouse.y - offset_y).clamp(0.0, 1.0),
                    );
                    let delta = NormalizedPosition::new(
                        desired_primary.x - primary_initial.x,
                        desired_primary.y - primary_initial.y,
                    );
                    layout_geometry::translate_zones(&mut working, initial_positions, delta)
                } else {
                    let norm_x = (mouse.x - offset_x).clamp(0.0, 1.0);
                    let norm_y = (mouse.y - offset_y).clamp(0.0, 1.0);
                    layout_geometry::drag_zone_to_position(
                        &mut working,
                        primary_zone_id,
                        NormalizedPosition::new(norm_x, norm_y),
                    )
                }
            }
            InteractionKind::Resize {
                zone_id,
                handle,
                start_mouse,
                start_center,
                start_size,
                rotation,
                keep_aspect_ratio,
            } => {
                let Some(zone) = working.zones.iter_mut().find(|z| z.id == *zone_id) else {
                    self.current_zones = working.zones;
                    return false;
                };
                let force_locked = matches!(
                    zone.shape,
                    Some(ZoneShape::Ring) | Some(ZoneShape::Arc { .. })
                );
                let (position, size) = layout_geometry::resize_zone_from_handle(
                    *start_center,
                    *start_size,
                    *start_mouse,
                    *handle,
                    mouse,
                    *keep_aspect_ratio || force_locked,
                    *rotation,
                );
                let changed = zone.position != position || zone.size != size;
                if changed {
                    zone.position = position;
                    zone.size = size;
                }
                changed
            }
        };
        self.current_zones = working.zones;

        if changed {
            self.moved.set(true);
            self.paint_affected();
        }
        changed
    }

    /// Recompute the inline `style` attribute on every cached element to
    /// reflect the current zone geometry. This is the only DOM write per
    /// frame — it sets the same string Leptos would have produced, so the
    /// reactive flush at mouseup is a clean handoff (matching strings,
    /// no extra paint).
    fn paint_affected(&self) {
        for zone in &self.current_zones {
            let Some(element) = self.elements.get(&zone.id) else {
                continue;
            };
            let style = element.style();
            let x_pct = zone.position.x * 100.0;
            let y_pct = zone.position.y * 100.0;
            let w_pct = zone.size.x * 100.0;
            let h_pct = zone.size.y * 100.0;
            let rotation = zone.rotation.to_degrees();
            let scale = zone.scale;
            let _ = style.set_property("left", &format!("{x_pct:.2}%"));
            let _ = style.set_property("top", &format!("{y_pct:.2}%"));
            let _ = style.set_property("width", &format!("{w_pct:.2}%"));
            let is_circular = matches!(
                zone.shape,
                Some(ZoneShape::Ring) | Some(ZoneShape::Arc { .. })
            );
            if is_circular {
                let _ = style.set_property("aspect-ratio", "1");
                // Browsers ignore stale `height` in the presence of
                // aspect-ratio, but clear it explicitly so the layout signal
                // can re-take ownership cleanly on commit.
                let _ = style.remove_property("height");
            } else {
                let _ = style.set_property("height", &format!("{h_pct:.2}%"));
                let _ = style.remove_property("aspect-ratio");
            }
            let _ = style.set_property(
                "transform",
                &format!("translate(-50%, -50%) rotate({rotation:.1}deg) scale({scale:.3})"),
            );
        }
    }
}

/// Tiny helper so the geometry helpers (which expect `&mut SpatialLayout`)
/// can run against just the zone vec we own during an interaction. Keeps
/// the rest of the layout immutable and out of our hot loop.
struct SpatialLayoutShim {
    zones: Vec<Output>,
}

impl SpatialLayoutShim {
    fn into_layout(self) -> SpatialLayout {
        SpatialLayout {
            id: String::new(),
            name: String::new(),
            description: None,
            canvas_width: 1,
            canvas_height: 1,
            zones: self.zones,
            default_sampling_mode: hypercolor_types::spatial::SamplingMode::Bilinear,
            default_edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }
}

pub(super) fn collect_zone_elements(
    viewport: &web_sys::HtmlElement,
    zone_ids: impl IntoIterator<Item = String>,
) -> HashMap<String, web_sys::HtmlElement> {
    let mut out = HashMap::new();
    for id in zone_ids {
        let selector = format!("[data-zone-id=\"{id}\"]");
        let Ok(Some(node)) = viewport.query_selector(&selector) else {
            continue;
        };
        if let Ok(element) = node.dyn_into::<web_sys::HtmlElement>() {
            out.insert(id, element);
        }
    }
    out
}

pub(super) fn pointer_to_normalized(
    viewport: &web_sys::HtmlElement,
    client_x: i32,
    client_y: i32,
) -> Option<NormalizedPosition> {
    let rect = viewport.get_bounding_client_rect();
    let cw = rect.width();
    let ch = rect.height();
    if cw <= 0.0 || ch <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(NormalizedPosition::new(
        ((f64::from(client_x) - rect.left()) / cw) as f32,
        ((f64::from(client_y) - rect.top()) / ch) as f32,
    ))
}

pub(super) fn update_canvas_slot_size(
    canvas_slot_ref: NodeRef<leptos::html::Div>,
    set_canvas_slot_size: WriteSignal<(f64, f64)>,
) {
    if let Some(slot) = canvas_slot_ref.try_get_untracked().flatten() {
        let rect = slot.get_bounding_client_rect();
        set_canvas_slot_size.set((rect.width(), rect.height()));
    }
}
