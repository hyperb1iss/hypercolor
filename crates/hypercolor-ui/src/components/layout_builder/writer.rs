//! Where each zone's [`ZoneLedger`] lives, and the task that writes its
//! intents to the daemon.
//!
//! Both live for the page, not for a Studio mount: an edit made just before
//! Studio closes still lands, and a Studio opened again reads the ledgers
//! its predecessor left rather than starting a second, conflicting copy.
//! The ledgers belong to one scene; selecting another drops them, since a
//! zone of one scene is not a zone of the next.
//!
//! A mounted Studio lends the writer [`WriterHooks`], so writes take their
//! turn with Studio's other scene operations, land in its history, and
//! redraw its canvas. With no Studio mounted the writer goes on without
//! them and tells no one.

use std::cell::RefCell;

use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};
use leptos::prelude::*;

use super::ledger::{Write, ZoneLedger};
use crate::api;

/// How many stale rejections in a row the writer takes before it leaves
/// the remaining intents for the next edit or wake.
const STALE_STREAK: u32 = 3;

/// What a mounted Studio lends the writer.
#[derive(Clone, Copy)]
pub(crate) struct WriterHooks {
    /// Claim the scene for writing; `None` while another operation holds
    /// it (that operation wakes the writer when it ends).
    pub begin: Callback<(), Option<u64>>,
    /// Release the claim `begin` handed out.
    pub end: Callback<u64>,
    /// A write landed: the revision it was fenced at, the zone the daemon
    /// replied with, and the revision it landed at.
    pub landed: Callback<(u64, api::ZoneResource, u64)>,
    /// The scene, read afresh after a stale rejection.
    pub refetched: Callback<api::SceneDocument>,
    /// A zone's ledger changed, so its canvas may draw differently.
    pub changed: Callback<String>,
    /// Something the user should hear about.
    pub problem: Callback<String>,
}

#[derive(Default)]
struct Ledgers {
    scene_id: Option<SceneId>,
    /// Zone ledgers, in the order their zones were first seen.
    zones: Vec<(String, ZoneLedger)>,
    /// The canvas size the shown outputs are drawn for.
    canvas: Option<(u32, u32)>,
    writing: bool,
    /// Set when a write failed: the writer waits for a new edit or a newer
    /// scene before trying again, rather than retrying in a loop.
    paused: bool,
    hooks: Option<(u64, WriterHooks)>,
    next_hooks: u64,
}

impl Ledgers {
    fn zone_mut(&mut self, zone_id: &str) -> &mut ZoneLedger {
        let index = self
            .zones
            .iter()
            .position(|(id, _)| id == zone_id)
            .unwrap_or_else(|| {
                self.zones.push((zone_id.to_owned(), ZoneLedger::default()));
                self.zones.len() - 1
            });
        &mut self.zones[index].1
    }

    fn zone(&self, zone_id: &str) -> Option<&ZoneLedger> {
        self.zones
            .iter()
            .find(|(id, _)| id == zone_id)
            .map(|(_, ledger)| ledger)
    }

    /// The next zone with a write to send, and that write.
    fn take_next(&mut self, scene_id: SceneId) -> Option<(String, Write)> {
        if self.scene_id != Some(scene_id) {
            return None;
        }
        self.zones
            .iter_mut()
            .find_map(|(id, ledger)| ledger.take_write().map(|write| (id.clone(), write)))
    }

    fn has_write(&self) -> bool {
        !self.paused
            && self
                .zones
                .iter()
                .any(|(_, ledger)| ledger.has_pending() && !ledger.in_flight())
    }
}

thread_local! {
    static LEDGERS: RefCell<Ledgers> = RefCell::new(Ledgers::default());
}

fn with_ledgers<T>(f: impl FnOnce(&mut Ledgers) -> T) -> T {
    LEDGERS.with_borrow_mut(f)
}

fn hooks() -> Option<WriterHooks> {
    with_ledgers(|ledgers| ledgers.hooks.map(|(_, hooks)| hooks))
}

/// The zone's outputs as the daemon stores them and as the canvas draws
/// them at `canvas`.
pub(super) fn zone_outputs(
    zone: &api::ZoneResource,
    canvas: (u32, u32),
) -> (Vec<Output>, Vec<Output>) {
    let mut layout = super::editor_layout_for_zone(zone);
    layout.canvas_width = canvas.0;
    layout.canvas_height = canvas.1;
    let raw = layout.zones.clone();
    let shown = crate::layout_geometry::normalize_layout_for_editor(layout).zones;
    (raw, shown)
}

/// Lend the writer a mounted Studio's hooks. The returned id takes them
/// back.
pub(crate) fn attach(hooks: WriterHooks) -> u64 {
    with_ledgers(|ledgers| {
        ledgers.next_hooks += 1;
        ledgers.hooks = Some((ledgers.next_hooks, hooks));
        ledgers.next_hooks
    })
}

/// Take back the hooks `attach` lent, unless a newer Studio lent its own.
pub(crate) fn detach(id: u64) {
    with_ledgers(|ledgers| {
        if ledgers.hooks.is_some_and(|(current, _)| current == id) {
            ledgers.hooks = None;
        }
    });
}

/// Make `scene_id` the scene the ledgers belong to, dropping another
/// scene's.
pub(crate) fn select_scene(scene_id: SceneId) {
    with_ledgers(|ledgers| {
        if ledgers.scene_id != Some(scene_id) {
            ledgers.scene_id = Some(scene_id);
            ledgers.zones.clear();
        }
    });
}

/// Take in a zone as the daemon reported it at `revision`, drawn for
/// `canvas`. A newer revision than the zone had also lets a paused writer
/// try again: the scene moved on, so the connection is back.
pub(crate) fn confirm(
    scene_id: SceneId,
    zone: &api::ZoneResource,
    revision: u64,
    canvas: (u32, u32),
) {
    select_scene(scene_id);
    let (raw, shown) = zone_outputs(zone, canvas);
    with_ledgers(|ledgers| {
        ledgers.canvas = Some(canvas);
        let ledger = ledgers.zone_mut(&zone.id.to_string());
        let newer = ledger.revision().is_none_or(|known| revision > known);
        ledger.confirm(revision, raw, shown);
        if newer {
            ledgers.paused = false;
        }
    });
}

/// Take in every zone of `scene` that has a ledger, so a zone edited and
/// then left still writes against the scene as it is. A zone the scene no
/// longer holds loses its ledger.
pub(crate) fn confirm_known(scene: &api::SceneDocument, canvas: (u32, u32)) {
    select_scene(scene.id);
    with_ledgers(|ledgers| {
        ledgers
            .zones
            .retain(|(id, _)| scene.zones.iter().any(|zone| zone.id.to_string() == *id));
        let mut newer = false;
        for zone in &scene.zones {
            let id = zone.id.to_string();
            if ledgers.zone(&id).is_none() {
                continue;
            }
            let (raw, shown) = zone_outputs(zone, canvas);
            let ledger = ledgers.zone_mut(&id);
            newer |= ledger.revision().is_none_or(|known| scene.revision > known);
            ledger.confirm(scene.revision, raw, shown);
        }
        if newer {
            ledgers.canvas = Some(canvas);
            ledgers.paused = false;
        }
    });
}

/// What the zone's canvas shows: confirmed with its intents applied.
pub(crate) fn display(scene_id: SceneId, zone_id: &str) -> Option<Vec<Output>> {
    with_ledgers(|ledgers| {
        (ledgers.scene_id == Some(scene_id))
            .then(|| ledgers.zone(zone_id).and_then(ZoneLedger::display))
            .flatten()
    })
}

/// Whether the zone has edits the daemon has not confirmed.
pub(crate) fn pending(scene_id: SceneId, zone_id: &str) -> bool {
    with_ledgers(|ledgers| {
        ledgers.scene_id == Some(scene_id)
            && ledgers.zone(zone_id).is_some_and(ZoneLedger::has_pending)
    })
}

/// Whether the scene has writes to send now or under way.
pub(crate) fn busy(scene_id: SceneId) -> bool {
    with_ledgers(|ledgers| {
        ledgers.scene_id == Some(scene_id) && (ledgers.writing || ledgers.has_write())
    })
}

/// Record a canvas edit to the zone; reports whether it changed anything.
/// A new edit lets a paused writer try again.
pub(crate) fn edit(scene_id: SceneId, zone_id: &str, edited: &[Output]) -> bool {
    with_ledgers(|ledgers| {
        let changed = ledgers.scene_id == Some(scene_id) && ledgers.zone_mut(zone_id).edit(edited);
        if changed {
            ledgers.paused = false;
        }
        changed
    })
}

/// Start writing, unless the writer already runs or nothing waits. With a
/// Studio mounted the writer first claims the scene; when an operation
/// holds it, that operation wakes the writer again as it ends.
pub(crate) fn wake() {
    let ready = with_ledgers(|ledgers| {
        (!ledgers.writing && ledgers.has_write())
            .then_some(ledgers.scene_id)
            .flatten()
    });
    let Some(scene_id) = ready else {
        return;
    };
    let lent = hooks();
    let claim = match lent {
        Some(hooks) => match hooks.begin.try_run(()).flatten() {
            Some(generation) => Some((hooks, generation)),
            None => return,
        },
        None => None,
    };
    with_ledgers(|ledgers| ledgers.writing = true);
    leptos::task::spawn_local(async move {
        run(scene_id).await;
        with_ledgers(|ledgers| ledgers.writing = false);
        if let Some((hooks, generation)) = claim {
            let _ = hooks.end.try_run(generation);
        }
        wake();
    });
}

fn changed(zone_id: &str) {
    if let Some(hooks) = hooks() {
        let _ = hooks.changed.try_run(zone_id.to_owned());
    }
}

fn problem(message: String) {
    if let Some(hooks) = hooks() {
        let _ = hooks.problem.try_run(message);
    }
}

/// Write until no zone of `scene_id` has a write to send.
async fn run(scene_id: SceneId) {
    let mut stale_streak = 0;
    while let Some((zone_id, write)) = with_ledgers(|ledgers| ledgers.take_next(scene_id)) {
        // The layout route stores each member's placement and nothing else.
        let layout = SpatialLayout {
            id: zone_id.clone(),
            name: String::new(),
            description: None,
            canvas_width: 1,
            canvas_height: 1,
            zones: write.outputs.clone(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            version: 1,
        };
        let canvas = with_ledgers(|ledgers| ledgers.canvas)
            .unwrap_or(crate::render_canvas::DEFAULT_RENDER_CANVAS);
        match api::zones::update_zone_layout(&zone_id, &layout, write.fence).await {
            Ok(api::zones::ZoneOutcome::Applied((zone, landed))) => {
                stale_streak = 0;
                let revision = landed.unwrap_or_else(|| write.fence.saturating_add(1));
                let (raw, shown) = zone_outputs(&zone, canvas);
                with_ledgers(|ledgers| {
                    if ledgers.scene_id == Some(scene_id) {
                        ledgers
                            .zone_mut(&zone_id)
                            .applied(write.id, revision, raw, shown);
                    }
                });
                if let Some(hooks) = hooks() {
                    let _ = hooks.landed.try_run((write.fence, zone, revision));
                }
                changed(&zone_id);
            }
            Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                with_ledgers(|ledgers| {
                    if ledgers.scene_id == Some(scene_id) {
                        ledgers.zone_mut(&zone_id).stale(write.id);
                    }
                });
                stale_streak += 1;
                let scene = match api::fetch_active_scene().await {
                    Ok(scene) if scene.id == scene_id => scene,
                    _ => {
                        with_ledgers(|ledgers| ledgers.paused = true);
                        break;
                    }
                };
                refetched(&scene, &zone_id, canvas);
                if let Some(hooks) = hooks() {
                    let _ = hooks.refetched.try_run(scene);
                }
                changed(&zone_id);
                if stale_streak >= STALE_STREAK {
                    with_ledgers(|ledgers| ledgers.paused = true);
                    problem(
                        "Layout change not saved yet: the scene keeps changing elsewhere. Your next edit sends it again."
                            .to_owned(),
                    );
                    break;
                }
            }
            Err(error) => {
                with_ledgers(|ledgers| {
                    ledgers.paused = true;
                    if ledgers.scene_id == Some(scene_id) {
                        ledgers.zone_mut(&zone_id).failed(write.id);
                    }
                });
                problem(format!(
                    "Layout change not saved: {error}. Your next edit sends it again."
                ));
                changed(&zone_id);
                break;
            }
        }
    }
}

/// Take in the scene read afresh: the stale zone settles what the fresh
/// state already satisfies, every other zone just confirms, and a zone the
/// scene no longer holds loses its ledger.
fn refetched(scene: &api::SceneDocument, stale_zone: &str, canvas: (u32, u32)) {
    with_ledgers(|ledgers| {
        ledgers
            .zones
            .retain(|(id, _)| scene.zones.iter().any(|zone| zone.id.to_string() == *id));
        for zone in &scene.zones {
            let id = zone.id.to_string();
            if ledgers.zone(&id).is_none() {
                continue;
            }
            let (raw, shown) = zone_outputs(zone, canvas);
            let ledger = ledgers.zone_mut(&id);
            if id == stale_zone {
                ledger.refetched(scene.revision, raw, shown);
            } else {
                ledger.confirm(scene.revision, raw, shown);
            }
        }
    });
}
