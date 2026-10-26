//! Where the active scene's zone ledgers live, and the task that writes
//! their intents to the daemon.
//!
//! Both live for the page, not for a Studio mount: an edit made just before
//! Studio closes still lands, and a Studio opened again reads the ledgers
//! its predecessor left rather than starting a second, conflicting copy.
//! What the writer has to tell the user goes out as a page toast, so it is
//! heard whether Studio is open or not, and the socket opening again
//! resumes a paused writer and re-anchors the ledgers for as long as the
//! page lives.
//!
//! A mounted Studio lends the writer [`WriterHooks`]. Each write then takes
//! its own turn with Studio's other scene operations, lands in Studio's
//! history, and redraws its canvas. With no Studio mounted the writer goes
//! on without them.
//!
//! When a write fails, or the scene cannot be read to settle one, the
//! writer pauses rather than retrying in a loop, and says so. A new edit or
//! a socket that opens again resumes it.

use std::cell::RefCell;

use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};
use leptos::prelude::*;

use super::ledger::{Cancelled, SceneLedgers, SceneRead, Settled, Ticket, Write, ZoneRead};
use crate::{api, toasts};

/// How many stale rejections in a row the writer takes before it pauses.
const STALE_STREAK: u32 = 3;

/// How many lost replies in a row the writer settles and resends before
/// it pauses.
const LOST_STREAK: u32 = 3;

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
    /// The writer stopped and let the scene go: Studio's own queue can
    /// take it.
    pub idle: Callback<()>,
}

struct Writer {
    ledgers: SceneLedgers,
    /// The canvas size the shown outputs are drawn for.
    canvas: Option<(u32, u32)>,
    writing: bool,
    /// Set when the writer stopped on a failure, for the active scene.
    paused: bool,
    hooks: Option<(u64, WriterHooks)>,
    next_hooks: u64,
    /// The owner of the socket watch, which lives as long as the page.
    watch: Option<Owner>,
}

impl Writer {
    fn new() -> Self {
        Self {
            ledgers: SceneLedgers::new(uuid::Uuid::new_v4().to_string()),
            canvas: None,
            writing: false,
            paused: false,
            hooks: None,
            next_hooks: 0,
            watch: None,
        }
    }

    /// Whether the writer has something to do now: a write to send, or a
    /// lost one to settle.
    fn has_work(&self) -> bool {
        !self.paused
            && self.ledgers.zones().any(|(_, ledger)| {
                ledger.unsettled()
                    || (ledger.has_pending() && !ledger.in_flight() && !ledger.reanchoring())
            })
    }
}

thread_local! {
    static WRITER: RefCell<Writer> = RefCell::new(Writer::new());
}

fn with_writer<T>(f: impl FnOnce(&mut Writer) -> T) -> T {
    WRITER.with_borrow_mut(f)
}

fn hooks() -> Option<WriterHooks> {
    with_writer(|writer| writer.hooks.map(|(_, hooks)| hooks))
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

/// A scene document as one read of every zone, drawn for `canvas`.
fn scene_read(scene: &api::SceneDocument, canvas: (u32, u32)) -> SceneRead {
    SceneRead {
        scene_id: scene.id,
        revision: scene.revision,
        zones: scene
            .zones
            .iter()
            .map(|zone| {
                let (raw, shown) = zone_outputs(zone, canvas);
                ZoneRead {
                    zone_id: zone.id.to_string(),
                    raw,
                    shown,
                }
            })
            .collect(),
    }
}

/// Tell the user which pending edits the daemon's state cancelled.
fn report(cancelled: &[Cancelled]) {
    let count: usize = cancelled.iter().map(|entry| entry.output_ids.len()).sum();
    match count {
        0 => {}
        1 => toasts::toast_info(
            "A layout change was dropped: its output left the zone before it was saved.",
        ),
        count => toasts::toast_info(&format!(
            "{count} layout changes were dropped: their outputs left the zone before they were saved."
        )),
    }
}

/// Lend the writer a mounted Studio's hooks. The returned id takes them
/// back.
pub(crate) fn attach(hooks: WriterHooks) -> u64 {
    with_writer(|writer| {
        writer.next_hooks += 1;
        writer.hooks = Some((writer.next_hooks, hooks));
        writer.next_hooks
    })
}

/// Take back the hooks `attach` lent, unless a newer Studio lent its own.
pub(crate) fn detach(id: u64) {
    with_writer(|writer| {
        if writer.hooks.is_some_and(|(current, _)| current == id) {
            writer.hooks = None;
        }
    });
}

/// Watch the socket for as long as the page lives: each time it opens
/// again the daemon may have started a new run, so the ledgers re-anchor,
/// and a writer paused on a failure tries again. Watching starts once;
/// later calls change nothing.
pub(crate) fn watch_connection(opened: ReadSignal<u64>) {
    if with_writer(|writer| writer.watch.is_some()) {
        return;
    }
    let owner = Owner::new_root(None);
    owner.with(|| {
        Effect::new(move |previous: Option<u64>| {
            let generation = opened.get();
            if previous.is_some_and(|previous| previous != generation) {
                with_writer(|writer| writer.ledgers.reanchor());
                resume();
            }
            generation
        });
    });
    with_writer(|writer| writer.watch = Some(owner));
}

/// Take in the active scene as a read of it reported, drawn for `canvas`,
/// making sure `wanted` has a ledger. Edits the read cancelled are reported.
pub(crate) fn observe(scene: &api::SceneDocument, canvas: (u32, u32), wanted: Option<&str>) {
    let read = scene_read(scene, canvas);
    let cancelled = with_writer(|writer| {
        writer.canvas = Some(canvas);
        let switching = writer.ledgers.scene_id() != Some(scene.id);
        if switching {
            writer.paused = false;
        }
        writer.ledgers.observe(&read, wanted)
    });
    report(&cancelled);
}

/// What the zone's canvas shows: confirmed with its intents applied.
pub(crate) fn display(scene_id: SceneId, zone_id: &str) -> Option<Vec<Output>> {
    with_writer(|writer| {
        (writer.ledgers.scene_id() == Some(scene_id))
            .then(|| {
                writer
                    .ledgers
                    .zone(zone_id)
                    .and_then(|ledger| ledger.display())
            })
            .flatten()
    })
}

/// Whether the zone has edits the daemon has not confirmed.
pub(crate) fn pending(scene_id: SceneId, zone_id: &str) -> bool {
    with_writer(|writer| {
        writer.ledgers.scene_id() == Some(scene_id)
            && writer
                .ledgers
                .zone(zone_id)
                .is_some_and(|ledger| ledger.has_pending())
    })
}

/// Whether the scene has writes to send now or under way.
pub(crate) fn busy(scene_id: SceneId) -> bool {
    with_writer(|writer| {
        writer.ledgers.scene_id() == Some(scene_id) && (writer.writing || writer.has_work())
    })
}

/// Record a canvas edit to the zone; reports whether it changed anything.
/// A new edit lets a paused writer try again.
pub(crate) fn edit(scene_id: SceneId, zone_id: &str, edited: &[Output]) -> bool {
    with_writer(|writer| {
        let changed = writer.ledgers.scene_id() == Some(scene_id)
            && writer.ledgers.zone_mut(zone_id).edit(edited);
        if changed {
            writer.paused = false;
        }
        changed
    })
}

/// Drop pending placement edits for `output_ids`: a membership write of
/// ours placed those outputs, and a leftover edit must not land over it.
pub(crate) fn forget(scene_id: SceneId, output_ids: &[String]) {
    with_writer(|writer| {
        if writer.ledgers.scene_id() != Some(scene_id) {
            return;
        }
        for (_, ledger) in writer.ledgers.zones_mut() {
            ledger.forget(output_ids);
        }
    });
}

/// Let a paused writer try again, as when the socket opens again.
pub(crate) fn resume() {
    with_writer(|writer| writer.paused = false);
    wake();
}

/// How a run of the writer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ended {
    /// It took at least one turn and has nothing left to do for now; work
    /// that arrived meanwhile starts it again.
    Done,
    /// It found nothing to do, or the ledgers moved on from what it was
    /// doing; nothing starts it again but new work.
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
    let ready = with_writer(|writer| !writer.writing && writer.has_work());
    if !ready {
        return;
    }
    let Some(first) = claim() else {
        return;
    };
    with_writer(|writer| writer.writing = true);
    leptos::task::spawn_local(async move {
        let ended = run(first).await;
        with_writer(|writer| writer.writing = false);
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

/// Stop for a failure, and say so, while the ledgers are still the ones
/// `ticket` was taken for.
fn pause(ticket: Ticket, message: &str) {
    let paused = with_writer(|writer| {
        let ours = writer.ledgers.admits(ticket);
        if ours {
            writer.paused = true;
        }
        ours
    });
    if paused {
        toasts::toast_error(message);
    }
}

/// What one turn of the writer does, for the ledgers `ticket` names.
enum Turn {
    /// Read the scene to settle the zone's lost write.
    Settle(String),
    Send(String, Write),
}

fn next_turn() -> Option<(Ticket, Turn)> {
    with_writer(|writer| {
        if writer.paused {
            return None;
        }
        let ticket = writer.ledgers.ticket()?;
        if let Some((zone_id, _)) = writer
            .ledgers
            .zones()
            .find(|(_, ledger)| ledger.unsettled())
        {
            return Some((ticket, Turn::Settle(zone_id.to_owned())));
        }
        writer
            .ledgers
            .next_write()
            .map(|(zone_id, write)| (ticket, Turn::Send(zone_id, write)))
    })
}

/// Failures in a row, so a writer that keeps failing stops and says so.
#[derive(Default)]
struct Streaks {
    stale: u32,
    lost: u32,
}

/// Write until no zone has anything to send or settle. With a Studio
/// mounted, every turn holds a claim on the scene of its own and releases
/// it after; a claim refused ends the run.
async fn run(first: Option<(WriterHooks, u64)>) -> Ended {
    let mut streaks = Streaks::default();
    let mut held = first;
    let mut turns = 0_u32;
    loop {
        let Some((ticket, turn)) = next_turn() else {
            release(held);
            return if turns == 0 { Ended::Idle } else { Ended::Done };
        };
        turns += 1;
        let next = match turn {
            Turn::Settle(zone_id) => settle_lost(ticket, &zone_id, None, &mut streaks).await,
            Turn::Send(zone_id, write) => send(ticket, &zone_id, write, &mut streaks).await,
        };
        release(held);
        match next {
            Next::Go => {}
            Next::Stop(ended) => return ended,
        }
        let Some(claimed) = claim() else {
            return Ended::Blocked;
        };
        held = claimed;
    }
}

/// What the writer does after a turn.
enum Next {
    Go,
    Stop(Ended),
}

fn canvas() -> (u32, u32) {
    with_writer(|writer| writer.canvas).unwrap_or(crate::render_canvas::DEFAULT_RENDER_CANVAS)
}

/// Read the scene afresh for the ledgers `ticket` names. `None` when the
/// read failed; `Some(None)` when the ledgers moved on while it was out.
async fn read_for(ticket: Ticket) -> Option<Option<(api::SceneDocument, SceneRead)>> {
    let scene = api::fetch_active_scene().await.ok()?;
    let read = scene_read(&scene, canvas());
    let admitted = with_writer(|writer| writer.ledgers.admit(ticket, &read));
    let Some(cancelled) = admitted else {
        return Some(None);
    };
    report(&cancelled);
    Some(Some((scene, read)))
}

fn zone_in<'a>(read: &'a SceneRead, zone_id: &str) -> Option<&'a ZoneRead> {
    read.zones.iter().find(|zone| zone.zone_id == zone_id)
}

/// Hand an admitted read to a mounted Studio.
fn share(scene: api::SceneDocument, zone_id: &str) {
    if let Some(hooks) = hooks() {
        let _ = hooks.refetched.try_run(scene);
    }
    changed(zone_id);
}

/// Read the scene to learn whether a lost write landed. `answered` is the
/// daemon's own error when it replied with one, which means a write the
/// read finds not landed was refused.
async fn settle_lost(
    ticket: Ticket,
    zone_id: &str,
    answered: Option<&str>,
    streaks: &mut Streaks,
) -> Next {
    let Some(read) = read_for(ticket).await else {
        pause(
            ticket,
            "Layout change not confirmed: the scene could not be read. It is checked again when the connection is back or with your next edit.",
        );
        return Next::Stop(Ended::Paused);
    };
    let Some((scene, read)) = read else {
        return Next::Stop(Ended::Idle);
    };
    let settled = with_writer(|writer| {
        let zone = zone_in(&read, zone_id)?;
        writer.ledgers.zone_mut(zone_id).settle_lost(
            read.revision,
            zone.raw.clone(),
            zone.shown.clone(),
        )
    });
    share(scene, zone_id);
    match settled {
        Some(Settled::Resend) => {
            if let Some(error) = answered {
                pause(
                    ticket,
                    &format!(
                        "Layout change not saved: {error}. It goes out again with your next edit."
                    ),
                );
                return Next::Stop(Ended::Paused);
            }
            streaks.lost += 1;
            if streaks.lost >= LOST_STREAK {
                pause(
                    ticket,
                    "Layout change not saved yet: replies keep getting lost. It goes out again when the connection is back or with your next edit.",
                );
                return Next::Stop(Ended::Paused);
            }
            Next::Go
        }
        Some(Settled::Landed { unconfirmed }) => {
            streaks.lost = 0;
            if unconfirmed {
                toasts::toast_error(
                    "A layout change may not have been saved, and the scene has changed since. Check the canvas.",
                );
            }
            Next::Go
        }
        None => Next::Go,
    }
}

/// After a stale rejection: read the scene, settle what it satisfies, and
/// go again, up to a streak.
async fn after_stale(ticket: Ticket, zone_id: &str, id: u64, streaks: &mut Streaks) -> Next {
    with_writer(|writer| {
        if writer.ledgers.admits(ticket) {
            writer.ledgers.zone_mut(zone_id).stale(id);
        }
    });
    streaks.stale += 1;
    let Some(read) = read_for(ticket).await else {
        pause(
            ticket,
            "Layout change not saved yet: the scene could not be read. It goes out again when the connection is back or with your next edit.",
        );
        return Next::Stop(Ended::Paused);
    };
    let Some((scene, read)) = read else {
        return Next::Stop(Ended::Idle);
    };
    let cancelled = with_writer(|writer| {
        zone_in(&read, zone_id).map(|zone| {
            writer.ledgers.zone_mut(zone_id).refetched(
                read.revision,
                zone.raw.clone(),
                zone.shown.clone(),
            )
        })
    });
    if let Some(output_ids) = cancelled {
        report(&[Cancelled {
            zone_id: zone_id.to_owned(),
            output_ids,
        }]);
    }
    share(scene, zone_id);
    if streaks.stale >= STALE_STREAK {
        pause(
            ticket,
            "Layout change not saved yet: the scene keeps changing elsewhere. It goes out again with your next edit.",
        );
        return Next::Stop(Ended::Paused);
    }
    Next::Go
}

/// Send one write and take in its outcome.
async fn send(ticket: Ticket, zone_id: &str, write: Write, streaks: &mut Streaks) -> Next {
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
            let cancelled = with_writer(|writer| {
                if !writer.ledgers.admits(ticket) {
                    return Vec::new();
                }
                let cancelled = writer
                    .ledgers
                    .zone_mut(zone_id)
                    .applied(write.id, revision, raw, shown);
                // Only this write landed between its fence and its revision,
                // so every other zone still stands as confirmed there.
                if revision == write.fence.saturating_add(1) {
                    for (id, ledger) in writer.ledgers.zones_mut() {
                        if id != zone_id {
                            ledger.advance(write.fence, revision);
                        }
                    }
                }
                cancelled
            });
            report(&[Cancelled {
                zone_id: zone_id.to_owned(),
                output_ids: cancelled,
            }]);
            if let Some(hooks) = hooks() {
                let _ = hooks.landed.try_run((write.fence, zone, revision));
            }
            changed(zone_id);
            Next::Go
        }
        Ok(api::zones::ZoneOutcome::Stale { .. }) => {
            after_stale(ticket, zone_id, write.id, streaks).await
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
                Some(409) if raced => after_stale(ticket, zone_id, write.id, streaks).await,
                // A refusal is final: the write did not land.
                Some(status) if (400..500).contains(&status) => {
                    with_writer(|writer| {
                        if writer.ledgers.admits(ticket) {
                            writer.ledgers.zone_mut(zone_id).failed(write.id);
                        }
                    });
                    changed(zone_id);
                    pause(
                        ticket,
                        &format!(
                            "Layout change not saved: {error}. It goes out again with your next edit."
                        ),
                    );
                    Next::Stop(Ended::Paused)
                }
                // Lost, unreadable, or failed after the daemon may have
                // committed it. The daemon does not yet honor the write's
                // key, so a fresh read settles it before anything else goes
                // out.
                _ => {
                    with_writer(|writer| {
                        if writer.ledgers.admits(ticket) {
                            writer.ledgers.zone_mut(zone_id).lost(write.id);
                        }
                    });
                    let answered = status.is_some().then(|| error.to_string());
                    settle_lost(ticket, zone_id, answered.as_deref(), streaks).await
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
            with_writer(|writer| {
                let read = SceneRead {
                    scene_id,
                    revision: 1,
                    zones: vec![ZoneRead {
                        zone_id: "zone".to_owned(),
                        raw: vec![output(0.2)],
                        shown: vec![output(0.2)],
                    }],
                };
                writer.ledgers.observe(&read, Some("zone"));
                assert!(writer.ledgers.zone_mut("zone").edit(&[output(0.4)]));
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
                idle: Callback::new(|()| {}),
            });
            // Woken again and again while an operation holds the scene: each
            // asks once and starts nothing, so nothing spins.
            for _ in 0..3 {
                wake();
            }
            assert_eq!(asked.get_value(), 3);
            assert!(!with_writer(|writer| writer.writing));
            assert!(busy(scene_id), "the edit still waits to go");
            detach(lent);
        });
        owner.cleanup();
    }
}
