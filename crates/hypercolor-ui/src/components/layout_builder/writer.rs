//! Where each zone's [`ZoneLedger`] lives, and the task that writes its
//! intents to the daemon.
//!
//! Both live for the page, not for a Studio mount: an edit made just before
//! Studio closes still lands, and a Studio opened again reads the ledgers
//! its predecessor left rather than starting a second, conflicting copy.
//! The ledgers belong to one scene; selecting another drops them, since a
//! zone of one scene is not a zone of the next.
//!
//! A mounted Studio lends the writer [`WriterHooks`]. Each write then takes
//! its own turn with Studio's other scene operations, lands in Studio's
//! history, and redraws its canvas. With no Studio mounted the writer goes
//! on without them and tells no one.
//!
//! When a write fails, or the scene cannot be read to settle one, the
//! writer pauses rather than retrying in a loop, and says so. A new edit or
//! a socket that opens again resumes it.

use std::cell::RefCell;

use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};
use leptos::prelude::*;

use super::ledger::{Settled, Write, ZoneLedger};
use crate::api;

/// How many stale rejections in a row the writer takes before it pauses.
const STALE_STREAK: u32 = 3;

/// What a mounted Studio lends the writer.
#[derive(Clone, Copy)]
pub(crate) struct WriterHooks {
    /// Claim the scene for one write; `None` while another operation holds
    /// it (that operation wakes the writer when it ends).
    pub begin: Callback<(), Option<u64>>,
    /// Release the claim `begin` handed out.
    pub end: Callback<u64>,
    /// A write landed: the revision it was fenced at, the zone the daemon
    /// replied with, and the revision it landed at.
    pub landed: Callback<(u64, api::ZoneResource, u64)>,
    /// The scene, read afresh to settle a write.
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
    /// Set when the writer stopped on a failure, for this scene.
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

    /// Whether the writer has something to do for its scene now: a write
    /// to send, or a lost one to settle.
    fn has_work(&self) -> bool {
        !self.paused
            && self.zones.iter().any(|(_, ledger)| {
                ledger.unsettled() || (ledger.has_pending() && !ledger.in_flight())
            })
    }

    fn pause(&mut self, scene_id: SceneId) {
        if self.scene_id == Some(scene_id) {
            self.paused = true;
        }
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
/// scene's ledgers and its pause.
pub(crate) fn select_scene(scene_id: SceneId) {
    with_ledgers(|ledgers| {
        if ledgers.scene_id != Some(scene_id) {
            ledgers.scene_id = Some(scene_id);
            ledgers.zones.clear();
            ledgers.paused = false;
        }
    });
}

/// Take in a zone as a scene read reported it at `revision`, drawn for
/// `canvas`.
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
        ledgers
            .zone_mut(&zone.id.to_string())
            .confirm(revision, raw, shown);
    });
}

/// Take in every zone of `scene` that has a ledger, so a zone edited and
/// then left still writes against the scene as it is. A zone the scene no
/// longer holds loses its ledger.
pub(crate) fn confirm_known(scene: &api::SceneDocument, canvas: (u32, u32)) {
    select_scene(scene.id);
    with_ledgers(|ledgers| {
        ledgers.canvas = Some(canvas);
        ledgers
            .zones
            .retain(|(id, _)| scene.zones.iter().any(|zone| zone.id.to_string() == *id));
        for zone in &scene.zones {
            let id = zone.id.to_string();
            if ledgers.zone(&id).is_none() {
                continue;
            }
            let (raw, shown) = zone_outputs(zone, canvas);
            ledgers.zone_mut(&id).confirm(scene.revision, raw, shown);
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
        ledgers.scene_id == Some(scene_id) && (ledgers.writing || ledgers.has_work())
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

/// Let a paused writer try again, as when the socket opens again.
pub(crate) fn resume() {
    with_ledgers(|ledgers| ledgers.paused = false);
    wake();
}

/// Start writing, unless the writer already runs or has nothing to do.
pub(crate) fn wake() {
    let ready = with_ledgers(|ledgers| {
        (!ledgers.writing && ledgers.has_work())
            .then_some(ledgers.scene_id)
            .flatten()
    });
    let Some(scene_id) = ready else {
        return;
    };
    with_ledgers(|ledgers| ledgers.writing = true);
    leptos::task::spawn_local(async move {
        run(scene_id).await;
        with_ledgers(|ledgers| ledgers.writing = false);
        wake();
    });
}

fn changed(zone_id: &str) {
    if let Some(hooks) = hooks() {
        let _ = hooks.changed.try_run(zone_id.to_owned());
    }
}

fn problem(message: &str) {
    if let Some(hooks) = hooks() {
        let _ = hooks.problem.try_run(message.to_owned());
    }
}

/// Stop for a failure, and say so.
fn pause(scene_id: SceneId, message: &str) {
    with_ledgers(|ledgers| ledgers.pause(scene_id));
    problem(message);
}

/// What one turn of the writer does.
enum Turn {
    /// Read the scene to settle the zone's lost write.
    Settle(String),
    Send(String, Write),
}

fn next_turn(scene_id: SceneId) -> Option<Turn> {
    with_ledgers(|ledgers| {
        if ledgers.scene_id != Some(scene_id) || ledgers.paused {
            return None;
        }
        if let Some((id, _)) = ledgers.zones.iter().find(|(_, ledger)| ledger.unsettled()) {
            return Some(Turn::Settle(id.clone()));
        }
        ledgers.zones.iter_mut().find_map(|(id, ledger)| {
            ledger
                .take_write()
                .map(|write| Turn::Send(id.clone(), write))
        })
    })
}

/// The scene read afresh, when it is still `scene_id`.
async fn read_scene(scene_id: SceneId) -> Option<api::SceneDocument> {
    api::fetch_active_scene()
        .await
        .ok()
        .filter(|scene| scene.id == scene_id)
}

/// Write until no zone of `scene_id` has anything to send or settle. With
/// a Studio mounted, every turn claims the scene for itself and releases it
/// after, so Studio's own operations go between writes.
async fn run(scene_id: SceneId) {
    let mut stale_streak = 0;
    loop {
        let claim = match hooks() {
            Some(hooks) => match hooks.begin.try_run(()).flatten() {
                Some(generation) => Some((hooks, generation)),
                None => break,
            },
            None => None,
        };
        let Some(turn) = next_turn(scene_id) else {
            if let Some((hooks, generation)) = claim {
                let _ = hooks.end.try_run(generation);
            }
            break;
        };
        let keep_going = match turn {
            Turn::Settle(zone_id) => settle_lost(scene_id, &zone_id).await,
            Turn::Send(zone_id, write) => send(scene_id, &zone_id, write, &mut stale_streak).await,
        };
        if let Some((hooks, generation)) = claim {
            let _ = hooks.end.try_run(generation);
        }
        if !keep_going {
            break;
        }
    }
}

/// Read the scene to learn whether a lost write landed. Reports whether
/// the writer goes on.
async fn settle_lost(scene_id: SceneId, zone_id: &str) -> bool {
    let canvas = canvas();
    let Some(scene) = read_scene(scene_id).await else {
        pause(
            scene_id,
            "Layout change not confirmed: the scene could not be read. It is checked again when the connection is back or with your next edit.",
        );
        return false;
    };
    let settled = with_ledgers(|ledgers| {
        let zone = scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)?;
        let (raw, shown) = zone_outputs(zone, canvas);
        ledgers
            .zone_mut(zone_id)
            .settle_lost(scene.revision, raw, shown)
    });
    confirm_known(&scene, canvas);
    if let Some(hooks) = hooks() {
        let _ = hooks.refetched.try_run(scene);
    }
    if settled == Some(Settled::Landed { unconfirmed: true }) {
        problem(
            "A layout change may not have been saved, and the scene has changed since. Check the canvas.",
        );
    }
    changed(zone_id);
    true
}

fn canvas() -> (u32, u32) {
    with_ledgers(|ledgers| ledgers.canvas).unwrap_or(crate::render_canvas::DEFAULT_RENDER_CANVAS)
}

/// Send one write and take in its outcome. Reports whether the writer goes
/// on.
async fn send(scene_id: SceneId, zone_id: &str, write: Write, stale_streak: &mut u32) -> bool {
    // The layout route stores each member's placement and nothing else.
    let layout = SpatialLayout {
        id: zone_id.to_owned(),
        name: String::new(),
        description: None,
        canvas_width: 1,
        canvas_height: 1,
        zones: write.outputs.clone(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    };
    let canvas = canvas();
    match api::zones::update_zone_layout(zone_id, &layout, write.fence).await {
        Ok(api::zones::ZoneOutcome::Applied((zone, landed))) => {
            *stale_streak = 0;
            let revision = landed.unwrap_or_else(|| write.fence.saturating_add(1));
            let (raw, shown) = zone_outputs(&zone, canvas);
            with_ledgers(|ledgers| {
                if ledgers.scene_id != Some(scene_id) {
                    return;
                }
                ledgers
                    .zone_mut(zone_id)
                    .applied(write.id, revision, raw, shown);
                // Only this write landed between its fence and its revision,
                // so every other zone still stands as confirmed there.
                if revision == write.fence.saturating_add(1) {
                    for (id, ledger) in &mut ledgers.zones {
                        if id != zone_id {
                            ledger.advance(write.fence, revision);
                        }
                    }
                }
            });
            if let Some(hooks) = hooks() {
                let _ = hooks.landed.try_run((write.fence, zone, revision));
            }
            changed(zone_id);
            true
        }
        Ok(api::zones::ZoneOutcome::Stale { .. }) => {
            with_ledgers(|ledgers| {
                if ledgers.scene_id == Some(scene_id) {
                    ledgers.zone_mut(zone_id).stale(write.id);
                }
            });
            *stale_streak += 1;
            let Some(scene) = read_scene(scene_id).await else {
                pause(
                    scene_id,
                    "Layout change not saved yet: the scene could not be read. It goes out again when the connection is back or with your next edit.",
                );
                return false;
            };
            with_ledgers(|ledgers| {
                if let Some(zone) = scene
                    .zones
                    .iter()
                    .find(|zone| zone.id.to_string() == zone_id)
                {
                    let (raw, shown) = zone_outputs(zone, canvas);
                    ledgers
                        .zone_mut(zone_id)
                        .refetched(scene.revision, raw, shown);
                }
            });
            confirm_known(&scene, canvas);
            if let Some(hooks) = hooks() {
                let _ = hooks.refetched.try_run(scene);
            }
            changed(zone_id);
            if *stale_streak >= STALE_STREAK {
                pause(
                    scene_id,
                    "Layout change not saved yet: the scene keeps changing elsewhere. It goes out again with your next edit.",
                );
                return false;
            }
            true
        }
        Err(error) => {
            // A refusal is final: the write did not land.
            if matches!(&error, api::ApiError::Http { status, .. } if (400..500).contains(status)) {
                with_ledgers(|ledgers| {
                    if ledgers.scene_id == Some(scene_id) {
                        ledgers.zone_mut(zone_id).failed(write.id);
                    }
                });
                changed(zone_id);
                pause(
                    scene_id,
                    &format!(
                        "Layout change not saved: {error}. It goes out again with your next edit."
                    ),
                );
                return false;
            }
            // Lost, unreadable, or failed after the daemon may have
            // committed it: a fresh read settles it before anything else
            // goes out.
            with_ledgers(|ledgers| {
                if ledgers.scene_id == Some(scene_id) {
                    ledgers.zone_mut(zone_id).lost(write.id);
                }
            });
            changed(zone_id);
            true
        }
    }
}
