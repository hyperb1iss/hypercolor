//! The Studio drag controller: one pointer gesture for every drag between
//! the device pane, the zone tabs, and the canvas.
//!
//! A drag source claims the press through [`DragController::press`] and
//! captures the pointer on the pressed element, so a press that never
//! travels stays an ordinary click. Once the pointer travels past the
//! threshold (or a touch holds still long enough), capture moves to the
//! page root, which outlives the source: holding over a tab opens it, and
//! the pane rebuilds under the drag without ending it. The page root
//! forwards every pointer event here.
//!
//! Drop targets register through markup: zone tabs carry `data-zone-tab`,
//! "+" carries `data-drop-new-zone`, pane zone sections carry
//! `data-pane-zone`, the Unassigned section carries `data-pane-unassigned`,
//! and the canvas viewport carries `data-studio-canvas`. The element under
//! the pointer picks the target, and only targets that take the payload
//! light up.
//!
//! A box dragged on the canvas stays the canvas's own gesture; the canvas
//! reports it through [`CanvasDragOut`] so a release on another zone's tab
//! moves the box there.

use std::time::Duration;

use hypercolor_leptos_ext::prelude::{now_ms, spawn_timeout};
use hypercolor_types::scene::ZoneRole;
use hypercolor_types::spatial::NormalizedPosition;
use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::app::DevicesContext;
use crate::components::layout_canvas::CanvasDragOut;
use crate::icons::*;
use crate::pointer_gesture::{
    GestureEnd, PointerEnd, PointerGesture, Press, capture_pointer, holds_capture,
    listener_element, suppress_press_defaults,
};
use crate::style_utils::device_accent_colors;

use super::StudioContext;
use super::drag_model::{
    CANCELLED_ANNOUNCEMENT, DragSource, DropTarget, MovePayload, SpringLoad, SpringPhase,
    action_hint, opened_announcement, resolve_drop, springs,
};
use super::moves::{canvas_payload, perform, zone_name};
use super::zone_strip::zone_tab_dom_id;

/// Pointer travel before a press becomes a drag.
const DRAG_THRESHOLD_PX: f64 = 6.0;
/// How long a touch rests before it picks a device up, so a quick swipe
/// still scrolls the pane.
const TOUCH_HOLD_MS: u64 = 350;
/// How close to the pane's top or bottom edge a drag starts scrolling it,
/// and how fast it scrolls at the very edge, in pixels per frame.
const AUTOSCROLL_EDGE_PX: f64 = 48.0;
const AUTOSCROLL_MAX_STEP: f64 = 14.0;
const TARGET_SELECTOR: &str = "[data-zone-tab], [data-drop-new-zone], [data-pane-zone], \
    [data-pane-unassigned], [data-studio-canvas]";

/// Where focus goes once a menu move lands: the element that opened the
/// move, or, when the move took that element away, the fallback. The
/// open tab is the last resort.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FocusReturn {
    pub target: String,
    pub fallback: Option<String>,
}

/// A press that may become a drag.
struct Pending {
    payload: MovePayload,
    pointer_id: i32,
    start: (f64, f64),
    touch: bool,
    dragging: bool,
    /// The element holding capture: the pressed element until the drag
    /// starts, the page root after.
    capture: web_sys::Element,
}

/// How a drop target draws under the current drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropLook {
    /// No drag, or a drag this target would refuse.
    Idle,
    /// The target takes the payload.
    Candidate,
    /// The pointer is over the target and a release would land here.
    Over,
}

impl DropLook {
    /// Classes for a pane section or row target.
    #[must_use]
    pub const fn section_class(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Candidate => "ring-1 ring-inset ring-accent/40",
            Self::Over => "ring-2 ring-inset ring-accent bg-accent/10",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DragController {
    pub studio: StudioContext,
    pub devices: DevicesContext,
    /// The page root: the capture target while a drag is live.
    pub root: NodeRef<html::Div>,
    gesture: StoredValue<PointerGesture<Pending>, LocalStorage>,
    /// The payload of the live drag, `None` between drags.
    pub payload: RwSignal<Option<MovePayload>>,
    /// The pointer, in client pixels.
    pub pointer: RwSignal<(f64, f64)>,
    /// The target under the pointer, whether or not it takes the payload.
    /// A canvas target's point lives in `canvas_point`.
    pub over: RwSignal<Option<DropTarget>>,
    pub canvas_point: RwSignal<Option<NormalizedPosition>>,
    /// The canvas viewport's client rect while a drag is live.
    pub canvas_rect: RwSignal<Option<(f64, f64, f64, f64)>>,
    spring: StoredValue<SpringLoad>,
    /// The tab whose spring is charging, for its fill ring.
    pub charging: RwSignal<Option<String>>,
    /// The aria-live text.
    pub announcement: RwSignal<String>,
    pending_focus: RwSignal<Option<(u64, FocusReturn)>>,
    /// Set when Escape cancels a canvas box drag that left the canvas, so
    /// its release puts the box back instead of moving it.
    canvas_escaped: StoredValue<bool>,
    /// Pixels per frame the pane scrolls while a drag rests near its edge,
    /// and whether the frame loop doing it is running.
    autoscroll: StoredValue<(f64, bool)>,
}

impl DragController {
    /// Build the controller for the Studio page whose root is `root`.
    pub fn new(studio: StudioContext, devices: DevicesContext, root: NodeRef<html::Div>) -> Self {
        let drag = Self {
            studio,
            devices,
            root,
            gesture: StoredValue::new_local(PointerGesture::new()),
            payload: RwSignal::new(None),
            pointer: RwSignal::new((0.0, 0.0)),
            over: RwSignal::new(None),
            canvas_point: RwSignal::new(None),
            canvas_rect: RwSignal::new(None),
            spring: StoredValue::new(SpringLoad::default()),
            charging: RwSignal::new(None),
            announcement: RwSignal::new(String::new()),
            pending_focus: RwSignal::new(None),
            canvas_escaped: StoredValue::new(false),
            autoscroll: StoredValue::new((0.0, false)),
        };

        // Focus waits for the scene that carries the move, so it lands on
        // the rebuilt pane rather than the rows it replaced.
        Effect::new(move |_| {
            let Some((revision, _)) = drag.pending_focus.get() else {
                return;
            };
            let live = studio
                .active_scene
                .with(|scene| scene.as_ref().map_or(0, |scene| scene.revision));
            if live < revision {
                return;
            }
            let Some((_, focus)) = drag.pending_focus.get_untracked() else {
                return;
            };
            drag.pending_focus.set(None);
            let open_tab = studio
                .selected_surface_id
                .get_untracked()
                .map(|id| zone_tab_dom_id(&id));
            request_animation_frame(move || {
                let candidates = std::iter::once(focus.target)
                    .chain(focus.fallback)
                    .chain(open_tab);
                for id in candidates {
                    if let Some(element) = document()
                        .get_element_by_id(&id)
                        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                    {
                        let _ = element.focus();
                        return;
                    }
                }
            });
        });

        let escape = window_event_listener(ev::keydown, move |ev| {
            if ev.key() == "Escape" && drag.payload.with_untracked(Option::is_some) {
                ev.prevent_default();
                ev.stop_propagation();
                drag.cancel();
            }
        });
        on_cleanup(move || escape.remove());

        provide_context(CanvasDragOut {
            hover: Callback::new(move |(x, y, ids): (f64, f64, Vec<String>)| {
                drag.canvas_hover(x, y, &ids);
            }),
            release: Callback::new(move |(x, y, ids): (f64, f64, Vec<String>)| {
                drag.canvas_release(x, y, &ids)
            }),
            cancel: Callback::new(move |()| {
                drag.canvas_escaped.set_value(false);
                drag.clear();
            }),
        });
        drag
    }

    /// Claim a press on a drag source for `payload`. Call from the
    /// source's `pointerdown`.
    pub fn press(self, ev: &web_sys::PointerEvent, payload: MovePayload) {
        if ev.button() != 0 || self.studio.history.busy.get_untracked() {
            return;
        }
        let Some(element) = listener_element(ev) else {
            return;
        };
        let pointer_id = ev.pointer_id();
        let press = self
            .gesture
            .try_update_value(|gesture| {
                gesture.press(pointer_id, |owner, state| {
                    holds_capture(Some(&state.capture), owner)
                })
            })
            .unwrap_or(Press::Busy);
        match press {
            Press::Busy => return,
            Press::Stale(stale) if stale.dragging => self.clear(),
            Press::Stale(_) | Press::Ready => {}
        }
        suppress_press_defaults(ev);
        capture_pointer(&element, ev);
        let touch = ev.pointer_type() == "touch";
        let pending = Pending {
            payload,
            pointer_id,
            start: (f64::from(ev.client_x()), f64::from(ev.client_y())),
            touch,
            dragging: false,
            capture: element,
        };
        self.gesture
            .update_value(|gesture| gesture.start(pointer_id, ev.button(), pending));
        if touch {
            spawn_timeout(Duration::from_millis(TOUCH_HOLD_MS), move || {
                let resting = self
                    .gesture
                    .try_with_value(|gesture| {
                        gesture
                            .state(pointer_id)
                            .map(|state| (!state.dragging).then_some(state.start))
                    })
                    .flatten()
                    .flatten();
                if let Some((x, y)) = resting {
                    self.begin(pointer_id);
                    self.track(x, y);
                }
            });
        }
    }

    /// Whether a touch press is live, so long-press callouts and context
    /// menus stay out of its way.
    #[must_use]
    pub fn holds_touch(self) -> bool {
        self.gesture
            .try_with_value(|gesture| gesture.current().is_some_and(|state| state.touch))
            .unwrap_or(false)
    }

    /// The page root's `pointermove`.
    pub fn on_move(self, ev: &web_sys::PointerEvent) {
        let pointer_id = ev.pointer_id();
        if self
            .gesture
            .with_value(|gesture| gesture.press_released(pointer_id, ev.buttons()))
        {
            self.on_end(ev, PointerEnd::Up);
            return;
        }
        let Some((start, touch, dragging)) = self.gesture.with_value(|gesture| {
            gesture
                .state(pointer_id)
                .map(|state| (state.start, state.touch, state.dragging))
        }) else {
            return;
        };
        let (x, y) = (f64::from(ev.client_x()), f64::from(ev.client_y()));
        if !dragging {
            let (dx, dy) = (x - start.0, y - start.1);
            if dx.hypot(dy) < DRAG_THRESHOLD_PX {
                return;
            }
            // A touch that leads vertically before its hold is a scroll;
            // the browser takes it and cancels the press.
            if touch && dy.abs() >= dx.abs() {
                return;
            }
            self.begin(pointer_id);
        }
        ev.prevent_default();
        self.track(x, y);
    }

    /// The page root's `pointerup` and `pointercancel`.
    pub fn on_end(self, ev: &web_sys::PointerEvent, how: PointerEnd) {
        let Some((outcome, state)) = self
            .gesture
            .try_update_value(|gesture| gesture.end(ev.pointer_id(), how))
            .flatten()
        else {
            return;
        };
        if !state.dragging {
            return;
        }
        let target = self.target_at(f64::from(ev.client_x()), f64::from(ev.client_y()));
        self.clear();
        let action = (outcome == GestureEnd::Commit)
            .then_some(target)
            .flatten()
            .and_then(|target| resolve_drop(&state.payload, &target));
        match action {
            Some(action) => perform(self, state.payload, action, None),
            None => self.announce(CANCELLED_ANNOUNCEMENT.to_owned()),
        }
    }

    /// The page root's `lostpointercapture`. Capture handed from the
    /// pressed element to the root is not a loss.
    pub fn on_lost_capture(self, ev: &web_sys::PointerEvent) {
        let from_holder = self
            .gesture
            .with_value(|gesture| {
                gesture.state(ev.pointer_id()).map(|state| {
                    ev.target()
                        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                        .is_some_and(|target| target == state.capture)
                })
            })
            .unwrap_or(false);
        if from_holder {
            self.on_end(ev, PointerEnd::LostCapture);
        }
    }

    /// The page root's `touchmove`: once a touch drag is live, the pane
    /// must not scroll under it.
    pub fn on_touch_move(self, ev: &web_sys::TouchEvent) {
        let dragging = self
            .gesture
            .with_value(|gesture| gesture.current().is_some_and(|state| state.dragging));
        if dragging && ev.cancelable() {
            ev.prevent_default();
        }
    }

    /// Drop the live drag without moving anything.
    pub fn cancel(self) {
        let from_canvas = self.payload.with_untracked(|payload| {
            payload
                .as_ref()
                .is_some_and(|payload| payload.source == DragSource::Canvas)
        });
        if from_canvas {
            self.canvas_escaped.set_value(true);
        }
        if let Some(state) = self
            .gesture
            .try_update_value(PointerGesture::abandon)
            .flatten()
            && let Some(root) = self.root.get_untracked()
        {
            let root: &web_sys::Element = &root;
            let _ = root.release_pointer_capture(state.pointer_id);
        }
        self.clear();
        self.announce(CANCELLED_ANNOUNCEMENT.to_owned());
    }

    /// Say `text` through the live region. Clearing first lets the same
    /// words announce twice in a row.
    pub fn announce(self, text: String) {
        self.announcement.set(String::new());
        request_animation_frame(move || {
            self.announcement.try_set(text);
        });
    }

    /// Move focus per `focus` once the scene reaches `revision`.
    pub fn focus_after(self, revision: u64, focus: FocusReturn) {
        self.pending_focus.try_set(Some((revision, focus)));
    }

    /// How `target` draws under the current drag.
    #[must_use]
    pub fn look(self, target: &DropTarget) -> DropLook {
        let takes = self.payload.with(|payload| {
            payload
                .as_ref()
                .is_some_and(|payload| resolve_drop(payload, target).is_some())
        });
        if !takes {
            return DropLook::Idle;
        }
        let over = self.over.with(|over| match (over, target) {
            (Some(DropTarget::Canvas { .. }), DropTarget::Canvas { .. }) => true,
            (Some(over), target) => over == target,
            (None, _) => false,
        });
        if over {
            DropLook::Over
        } else {
            DropLook::Candidate
        }
    }

    /// Turn the press into a drag: capture moves to the page root, the
    /// ghost appears, and the targets light up.
    fn begin(self, pointer_id: i32) {
        let Some(root) = self.root.get_untracked() else {
            return;
        };
        let root: web_sys::Element = (*root).clone().into();
        let payload = self
            .gesture
            .try_update_value(|gesture| {
                let state = gesture.state_mut(pointer_id)?;
                state.dragging = true;
                state.capture = root.clone();
                Some(state.payload.clone())
            })
            .flatten();
        let Some(payload) = payload else {
            return;
        };
        let _ = root.set_pointer_capture(pointer_id);
        self.spring.set_value(SpringLoad::default());
        self.payload.set(Some(payload));
        self.measure_canvas();
        // On a narrow viewport the pane is a drawer over the Stage; the
        // drag needs the tabs and canvas it covers.
        if self.studio.pane_drawer.get_untracked() {
            self.studio.pane_drawer.set(false);
        }
    }

    fn track(self, x: f64, y: f64) {
        self.pointer.set((x, y));
        self.steer_autoscroll(x, y);
        let target = self.target_at(x, y);
        let point = match &target {
            Some(DropTarget::Canvas { point, .. }) => Some(*point),
            _ => None,
        };
        let kind = target.map(|target| match target {
            DropTarget::Canvas { zone_id, .. } => DropTarget::Canvas {
                zone_id,
                point: NormalizedPosition::new(0.5, 0.5),
            },
            other => other,
        });
        if self.over.with_untracked(|over| *over != kind) {
            self.over.set(kind.clone());
        }
        if self
            .canvas_point
            .with_untracked(|current| *current != point)
        {
            self.canvas_point.set(point);
        }
        if !self
            .payload
            .with_untracked(|payload| payload.as_ref().is_some_and(springs))
        {
            return;
        }
        let tab = match &kind {
            Some(DropTarget::Tab(zone_id)) => Some(zone_id.clone()),
            _ => None,
        };
        let open = self.studio.selected_surface_id.get_untracked();
        let phase = self
            .spring
            .try_update_value(|spring| spring.hover(tab.as_deref(), open.as_deref(), now_ms()))
            .unwrap_or(SpringPhase::Idle);
        match phase {
            SpringPhase::Charging(zone_id) => {
                self.charging.set(Some(zone_id));
                // A few milliseconds past the hold, so the tick sees it done.
                let wait = Duration::from_millis(SpringLoad::HOLD_MS as u64 + 16);
                spawn_timeout(wait, move || self.spring_tick());
            }
            SpringPhase::Idle => {
                if self.charging.with_untracked(Option::is_some) {
                    self.charging.set(None);
                }
            }
            SpringPhase::Holding => {}
        }
    }

    /// Scroll the pane while the drag rests near its top or bottom edge,
    /// so a device can travel to a section below the fold.
    fn steer_autoscroll(self, x: f64, y: f64) {
        let step = pane_scroller().map_or(0.0, |pane| {
            let rect = pane.get_bounding_client_rect();
            if x < rect.left() || x > rect.right() {
                return 0.0;
            }
            let ramp = |distance: f64| {
                ((AUTOSCROLL_EDGE_PX - distance) / AUTOSCROLL_EDGE_PX).clamp(0.0, 1.0)
                    * AUTOSCROLL_MAX_STEP
            };
            let (top, bottom) = (y - rect.top(), rect.bottom() - y);
            if (0.0..AUTOSCROLL_EDGE_PX).contains(&top) {
                -ramp(top)
            } else if (0.0..AUTOSCROLL_EDGE_PX).contains(&bottom) {
                ramp(bottom)
            } else {
                0.0
            }
        });
        let start = self
            .autoscroll
            .try_update_value(|(velocity, running)| {
                *velocity = step;
                let start = step != 0.0 && !*running;
                *running |= start;
                start
            })
            .unwrap_or(false);
        if start {
            request_animation_frame(move || self.autoscroll_frame());
        }
    }

    fn autoscroll_frame(self) {
        let dragging = self
            .payload
            .try_with_untracked(Option::is_some)
            .unwrap_or(false);
        let velocity = self.autoscroll.try_with_value(|(velocity, _)| *velocity);
        let (Some(velocity), true) = (velocity, dragging) else {
            self.autoscroll.try_set_value((0.0, false));
            return;
        };
        if velocity == 0.0 {
            self.autoscroll.try_set_value((0.0, false));
            return;
        }
        if let Some(pane) = pane_scroller() {
            #[allow(clippy::cast_possible_truncation)]
            pane.set_scroll_top(pane.scroll_top() + velocity.round() as i32);
        }
        // Content moved under a resting pointer; the target may have too.
        let (x, y) = self.pointer.get_untracked();
        self.track(x, y);
        request_animation_frame(move || self.autoscroll_frame());
    }

    fn spring_tick(self) {
        if self
            .payload
            .try_with_untracked(Option::is_none)
            .unwrap_or(true)
        {
            return;
        }
        let Some(zone_id) = self
            .spring
            .try_update_value(|spring| spring.tick(now_ms()))
            .flatten()
        else {
            return;
        };
        self.charging.set(None);
        self.studio.selected_surface_id.set(Some(zone_id.clone()));
        self.announce(opened_announcement(&zone_name(self.studio, &zone_id)));
        request_animation_frame(move || self.measure_canvas());
    }

    fn measure_canvas(self) {
        let rect = document()
            .query_selector("[data-studio-canvas]")
            .ok()
            .flatten()
            .map(|element| {
                let rect = element.get_bounding_client_rect();
                (rect.left(), rect.top(), rect.width(), rect.height())
            });
        self.canvas_rect.try_set(rect);
    }

    /// The drop target under a client point.
    fn target_at(self, x: f64, y: f64) -> Option<DropTarget> {
        #[allow(clippy::cast_possible_truncation)]
        let element = document().element_from_point(x as f32, y as f32)?;
        let hit = element.closest(TARGET_SELECTOR).ok().flatten()?;
        if let Some(zone_id) = hit.get_attribute("data-zone-tab") {
            return Some(DropTarget::Tab(zone_id));
        }
        if hit.has_attribute("data-drop-new-zone") {
            return Some(DropTarget::NewZone);
        }
        if let Some(zone_id) = hit.get_attribute("data-pane-zone") {
            return Some(DropTarget::PaneZone(zone_id));
        }
        if hit.has_attribute("data-pane-unassigned") {
            return Some(DropTarget::Unassigned);
        }
        let zone_id = self.open_led_zone()?;
        let rect = hit.get_bounding_client_rect();
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        let point = NormalizedPosition::new(
            ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f32,
            ((y - rect.top()) / rect.height()).clamp(0.0, 1.0) as f32,
        );
        Some(DropTarget::Canvas { zone_id, point })
    }

    /// The open tab, when it is an LED zone.
    fn open_led_zone(self) -> Option<String> {
        let selected = self.studio.selected_surface_id.get_untracked()?;
        self.studio.active_scene.with_untracked(|scene| {
            scene.as_ref().and_then(|scene| {
                scene
                    .zones
                    .iter()
                    .any(|zone| zone.id.to_string() == selected && zone.role != ZoneRole::Display)
                    .then_some(selected.clone())
            })
        })
    }

    /// End the drag's visuals.
    fn clear(self) {
        self.lift();
        self.canvas_rect.try_set(None);
    }

    fn lift(self) {
        self.spring.try_set_value(SpringLoad::default());
        self.payload.try_set(None);
        self.over.try_set(None);
        self.canvas_point.try_set(None);
        self.charging.try_set(None);
    }

    /// A canvas box drag moved to a client point. Inside the canvas it is
    /// the canvas's own drag; once it leaves, it carries a payload and the
    /// tabs that would take it light up.
    fn canvas_hover(self, x: f64, y: f64, ids: &[String]) {
        if self.canvas_escaped.get_value() {
            return;
        }
        if self.canvas_rect.with_untracked(Option::is_none) {
            self.measure_canvas();
        }
        let inside = self.canvas_rect.with_untracked(|rect| {
            rect.is_some_and(|(left, top, width, height)| {
                x >= left && x <= left + width && y >= top && y <= top + height
            })
        });
        if inside {
            if self.payload.with_untracked(Option::is_some) {
                self.lift();
            }
            return;
        }
        if self.payload.with_untracked(Option::is_none) {
            let Some(zone_id) = self.open_led_zone() else {
                return;
            };
            let Some(payload) = canvas_payload(self, &zone_id, ids) else {
                return;
            };
            self.payload.set(Some(payload));
        }
        self.track(x, y);
    }

    /// A canvas box drag released at a client point. True when the
    /// release landed on a target that took it; the canvas then puts the
    /// box back and the move takes it away.
    fn canvas_release(self, x: f64, y: f64, ids: &[String]) -> bool {
        let target = self.target_at(x, y);
        self.clear();
        if self.canvas_escaped.get_value() {
            self.canvas_escaped.set_value(false);
            return true;
        }
        let Some(zone_id) = self.open_led_zone() else {
            return false;
        };
        let Some(payload) = canvas_payload(self, &zone_id, ids) else {
            return false;
        };
        let Some(action) = target.and_then(|target| resolve_drop(&payload, &target)) else {
            return false;
        };
        perform(self, payload, action, None);
        true
    }
}

/// Where the ghost sits for a pointer at `(x, y)`: below and right of it,
/// flipping to the other side near the window's right or bottom edge.
fn ghost_transform((x, y): (f64, f64)) -> String {
    const ROOM_X: f64 = 290.0;
    const ROOM_Y: f64 = 72.0;
    let window = window();
    let extent = |value: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>| {
        value
            .ok()
            .and_then(|value| value.as_f64())
            .unwrap_or(f64::MAX)
    };
    let (width, height) = (extent(window.inner_width()), extent(window.inner_height()));
    let (dx, shift_x) = if x + ROOM_X > width {
        (x - 14.0, "-100%")
    } else {
        (x + 14.0, "0")
    };
    let (dy, shift_y) = if y + ROOM_Y > height {
        (y - 12.0, "-100%")
    } else {
        (y + 12.0, "0")
    };
    format!("transform: translate({dx:.0}px, {dy:.0}px) translate({shift_x}, {shift_y})")
}

/// The device pane's scrolling list.
fn pane_scroller() -> Option<web_sys::Element> {
    document()
        .query_selector("[data-pane-scroll]")
        .ok()
        .flatten()
}

/// The page-level drag furniture: the ghost under the pointer, the canvas
/// highlight, and the aria-live region.
#[component]
pub(crate) fn DragLayer() -> impl IntoView {
    let drag = expect_context::<DragController>();
    let studio = drag.studio;

    let ghost = Memo::new(move |_| {
        let payload = drag.payload.get()?;
        let over = drag.over.get();
        let hint = over
            .as_ref()
            .and_then(|target| resolve_drop(&payload, target))
            .map(|action| action_hint(&action, |id| zone_name(studio, id)));
        // A box dragged on its own canvas is its own ghost.
        if payload.source == DragSource::Canvas && hint.is_none() {
            return None;
        }
        Some((payload, hint))
    });
    let canvas_look = Memo::new(move |_| {
        let rect = drag.canvas_rect.get()?;
        let look = drag.look(&DropTarget::Canvas {
            zone_id: String::new(),
            point: NormalizedPosition::new(0.5, 0.5),
        });
        (look != DropLook::Idle).then_some((rect, look))
    });

    view! {
        <div class="sr-only" role="status" aria-live="polite" aria-atomic="true">
            {move || drag.announcement.get()}
        </div>
        {move || {
            canvas_look
                .get()
                .map(|((left, top, width, height), look)| {
                    let over = look == DropLook::Over;
                    view! {
                        <div
                            class="pointer-events-none fixed z-[60] rounded-lg border-2 transition-colors duration-150"
                            class=("border-accent", over)
                            class=("bg-accent/10", over)
                            class=("border-accent/50", !over)
                            class=("border-dashed", !over)
                            style=format!(
                                "left: {left:.0}px; top: {top:.0}px; width: {width:.0}px; height: {height:.0}px",
                            )
                        >
                            {move || {
                                drag.canvas_point
                                    .get()
                                    .map(|point| {
                                        view! {
                                            <span
                                                class="absolute h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-accent bg-surface-base"
                                                style=format!(
                                                    "left: {:.2}%; top: {:.2}%",
                                                    point.x * 100.0,
                                                    point.y * 100.0,
                                                )
                                            />
                                        }
                                    })
                            }}
                        </div>
                    }
                })
        }}
        {move || {
            ghost
                .get()
                .map(|(payload, hint)| {
                    let (primary, secondary) = device_accent_colors(&payload.device_id);
                    let strip = format!(
                        "background: linear-gradient(180deg, rgb({primary}), rgb({secondary}))",
                    );
                    view! {
                        <div
                            class="pointer-events-none fixed left-0 top-0 z-[80]"
                            style=move || ghost_transform(drag.pointer.get())
                            aria-hidden="true"
                        >
                            <div class="flex max-w-[260px] items-center gap-2 rounded-lg border border-edge-subtle bg-surface-overlay/98 py-1.5 pl-2 pr-2.5 backdrop-blur-xl dropdown-glow animate-enter-scale">
                                <span class="h-7 w-1 shrink-0 rounded-full" style=strip />
                                <div class="min-w-0">
                                    <div class="truncate text-[12px] font-medium text-fg-primary">
                                        {payload.label.clone()}
                                    </div>
                                    {hint.map(|hint| {
                                        view! {
                                            <div class="flex items-center gap-1 text-[10px] text-fg-tertiary">
                                                <Icon icon=LuChevronRight width="10px" height="10px" />
                                                <span class="truncate">{hint}</span>
                                            </div>
                                        }
                                    })}
                                </div>
                                {payload.badge().map(|count| {
                                    view! {
                                        <span class="shrink-0 rounded-full border border-accent-muted bg-accent-subtle px-1.5 font-mono text-[10px] tabular-nums text-fg-primary">
                                            {count}
                                        </span>
                                    }
                                })}
                            </div>
                        </div>
                    }
                })
        }}
    }
}
