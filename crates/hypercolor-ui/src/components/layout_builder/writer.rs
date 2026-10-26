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
    /// The writer stopped and let the scene go: Studio's own queue can
    /// take it.
    pub idle: Callback<()>,
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

/// How a run of the writer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ended {
    /// It took at least one turn and has nothing left to do for now; work
    /// that arrived meanwhile starts it again.
    Done,
    /// It found nothing to do at all, so nothing starts it again.
    Idle,
    /// Another operation holds the scene; it wakes the writer as it ends.
    Blocked,
    /// A failure stopped it; a new edit or a reopened socket resumes it.
    Paused,
}

/// Claim the scene for one turn: `Some(None)` with no Studio to claim it
/// from, `None` while another operation holds it.
fn claim() -> Option<Option<(WriterHooks, u64)>> {
    match hooks() {
        Some(hooks) => hooks
            .begin
            .try_run(())
            .flatten()
            .map(|generation| Some((hooks, generation))),
        None => Some(None),
    }
}

fn release(claim: Option<(WriterHooks, u64)>) {
    if let Some((hooks, generation)) = claim {
        let _ = hooks.end.try_run(generation);
    }
}

/// Start writing, unless the writer already runs or has nothing to do. The
/// first turn's claim is taken here, before anything is spawned, so a scene
/// another operation holds starts nothing at all: that operation wakes the
/// writer as it ends.
pub(crate) fn wake() {
    let ready = with_ledgers(|ledgers| {
        (!ledgers.writing && ledgers.has_work())
            .then_some(ledgers.scene_id)
            .flatten()
    });
    let Some(scene_id) = ready else {
        return;
    };
    let Some(first) = claim() else {
        return;
    };
    with_ledgers(|ledgers| ledgers.writing = true);
    leptos::task::spawn_local(async move {
        let ended = run(scene_id, first).await;
        with_ledgers(|ledgers| ledgers.writing = false);
        // Commits still go first: more work starts the writer again before
        // the queue gets the scene.
        if ended == Ended::Done {
            wake();
        }
        if let Some(hooks) = hooks() {
            let _ = hooks.idle.try_run(());
        }
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

/// Stop for a failure, and say so, while the ledgers still belong to
/// `scene_id`.
fn pause(scene_id: SceneId, message: &str) {
    let paused = with_ledgers(|ledgers| {
        let ours = ledgers.scene_id == Some(scene_id);
        ledgers.pause(scene_id);
        ours
    });
    if paused {
        problem(message);
    }
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

/// Failures in a row, so a writer that keeps failing stops and says so.
#[derive(Default)]
struct Streaks {
    stale: u32,
    lost: u32,
}

/// Write until no zone of `scene_id` has anything to send or settle. With
/// a Studio mounted, every turn holds a claim on the scene of its own and
/// releases it after; a claim refused ends the run.
async fn run(scene_id: SceneId, first: Option<(WriterHooks, u64)>) -> Ended {
    let mut streaks = Streaks::default();
    let mut held = first;
    let mut turns = 0_u32;
    loop {
        let Some(turn) = next_turn(scene_id) else {
            release(held);
            return if turns == 0 { Ended::Idle } else { Ended::Done };
        };
        turns += 1;
        let going = match turn {
            Turn::Settle(zone_id) => settle_lost(scene_id, &zone_id, None, &mut streaks).await,
            Turn::Send(zone_id, write) => send(scene_id, &zone_id, write, &mut streaks).await,
        };
        release(held);
        if !going {
            return Ended::Paused;
        }
        let Some(next) = claim() else {
            return Ended::Blocked;
        };
        held = next;
    }
}

fn canvas() -> (u32, u32) {
    with_ledgers(|ledgers| ledgers.canvas).unwrap_or(crate::render_canvas::DEFAULT_RENDER_CANVAS)
}

/// How many lost replies in a row the writer settles and resends before
/// it pauses.
const LOST_STREAK: u32 = 3;

/// Read the scene to learn whether a lost write landed. `answered` is the
/// daemon's own error when it replied with one, which means a write the
/// read finds not landed was refused. Reports whether the writer goes on.
async fn settle_lost(
    scene_id: SceneId,
    zone_id: &str,
    answered: Option<&str>,
    streaks: &mut Streaks,
) -> bool {
    let canvas = canvas();
    let Some(scene) = read_scene(scene_id).await else {
        pause(
            scene_id,
            "Layout change not confirmed: the scene could not be read. It is checked again when the connection is back or with your next edit.",
        );
        return false;
    };
    let settled = with_ledgers(|ledgers| {
        if ledgers.scene_id != Some(scene_id) {
            return None;
        }
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
    changed(zone_id);
    match settled {
        Some(Settled::Resend) => {
            if let Some(error) = answered {
                pause(
                    scene_id,
                    &format!(
                        "Layout change not saved: {error}. It goes out again with your next edit."
                    ),
                );
                return false;
            }
            streaks.lost += 1;
            if streaks.lost >= LOST_STREAK {
                pause(
                    scene_id,
                    "Layout change not saved yet: replies keep getting lost. It goes out again when the connection is back or with your next edit.",
                );
                return false;
            }
            true
        }
        Some(Settled::Landed { unconfirmed }) => {
            streaks.lost = 0;
            if unconfirmed {
                problem(
                    "A layout change may not have been saved, and the scene has changed since. Check the canvas.",
                );
            }
            true
        }
        None => true,
    }
}

/// After a stale rejection: read the scene, settle what it satisfies, and
/// go again, up to a streak. Reports whether the writer goes on.
async fn after_stale(scene_id: SceneId, zone_id: &str, id: u64, streaks: &mut Streaks) -> bool {
    with_ledgers(|ledgers| {
        if ledgers.scene_id == Some(scene_id) {
            ledgers.zone_mut(zone_id).stale(id);
        }
    });
    streaks.stale += 1;
    let canvas = canvas();
    let Some(scene) = read_scene(scene_id).await else {
        pause(
            scene_id,
            "Layout change not saved yet: the scene could not be read. It goes out again when the connection is back or with your next edit.",
        );
        return false;
    };
    with_ledgers(|ledgers| {
        if ledgers.scene_id != Some(scene_id) {
            return;
        }
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
    if streaks.stale >= STALE_STREAK {
        pause(
            scene_id,
            "Layout change not saved yet: the scene keeps changing elsewhere. It goes out again with your next edit.",
        );
        return false;
    }
    true
}

/// Send one write and take in its outcome. Reports whether the writer goes
/// on.
async fn send(scene_id: SceneId, zone_id: &str, write: Write, streaks: &mut Streaks) -> bool {
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
            streaks.stale = 0;
            streaks.lost = 0;
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
            after_stale(scene_id, zone_id, write.id, streaks).await
        }
        Err(error) => {
            let status = match &error {
                api::ApiError::Http { status, .. } => Some(*status),
                _ => None,
            };
            let raced = matches!(
                &error,
                api::ApiError::Http { status: 409, details: Some(details), .. }
                    if details.get("kind").and_then(serde_json::Value::as_str)
                        == Some("scene_commit_superseded")
            );
            match status {
                // A commit race: another write won, as with a stale fence.
                Some(409) if raced => after_stale(scene_id, zone_id, write.id, streaks).await,
                // A refusal is final: the write did not land.
                Some(status) if (400..500).contains(&status) => {
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
                    false
                }
                // Lost, unreadable, or failed after the daemon may have
                // committed it: a fresh read settles it before anything
                // else goes out.
                _ => {
                    with_ledgers(|ledgers| {
                        if ledgers.scene_id == Some(scene_id) {
                            ledgers.zone_mut(zone_id).lost(write.id);
                        }
                    });
                    let answered = status.is_some().then(|| error.to_string());
                    settle_lost(scene_id, zone_id, answered.as_deref(), streaks).await
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercolor_types::spatial::{LedTopology, NormalizedPosition, StripDirection};

    fn output(x: f32) -> Output {
        Output {
            id: "a".to_owned(),
            name: "a".to_owned(),
            device_id: "usb:a".to_owned(),
            zone_name: None,
            position: NormalizedPosition::new(x, 0.5),
            size: NormalizedPosition::new(0.1, 0.1),
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 10,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            attachment: None,
            brightness: None,
        }
    }

    #[test]
    fn a_held_scene_starts_no_writer_until_its_holder_lets_go() {
        let owner = Owner::new();
        owner.with(|| {
            let scene_id = SceneId::new();
            select_scene(scene_id);
            with_ledgers(|ledgers| {
                let ledger = ledgers.zone_mut("zone");
                ledger.confirm(1, vec![output(0.2)], vec![output(0.2)]);
                assert!(ledger.edit(&[output(0.4)]));
            });
            let asked = StoredValue::new(0_u32);
            let lent = attach(WriterHooks {
                begin: Callback::new(move |()| {
                    asked.update_value(|asked| *asked += 1);
                    None
                }),
                end: Callback::new(|_| {}),
                landed: Callback::new(|_| {}),
                refetched: Callback::new(|_| {}),
                changed: Callback::new(|_| {}),
                problem: Callback::new(|_| {}),
                idle: Callback::new(|()| {}),
            });
            // Woken again and again while an operation holds the scene: each
            // asks once and starts nothing, so nothing spins.
            for _ in 0..3 {
                wake();
            }
            assert_eq!(asked.get_value(), 3);
            assert!(!with_ledgers(|ledgers| ledgers.writing));
            assert!(busy(scene_id), "the edit still waits to go");
            detach(lent);
        });
        owner.cleanup();
    }
}
