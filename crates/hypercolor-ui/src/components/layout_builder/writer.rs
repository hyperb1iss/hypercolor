//! Where the active scene's zone ledgers live, and the task that writes
//! their intents to the daemon.
//!
//! Both live for the page, not for a Studio mount: an edit made just before
//! Studio closes still lands, and a Studio opened again reads the ledgers
//! its predecessor left rather than starting a second, conflicting copy.
//! What the writer has to tell the user goes out as a page toast, so it is
//! heard whether Studio is open or not. The socket watch lives for the page
//! too: each time the socket opens again a writer paused on a failure
//! tries again, and when its handshake names a new daemon run the ledgers
//! re-anchor and the writer reads the scene itself for any edit that waits
//! on that, so nothing waits for a Studio to be opened.
//!
//! Each write goes out under an idempotency key of its own (see
//! [`super::keyed`]), and a write no answer came back for goes out again
//! under the same key, after a short wait each time and without holding the
//! scene meanwhile, until the daemon answers it.
//!
//! A mounted Studio lends the writer [`WriterHooks`]. Each write then takes
//! its own turn with Studio's other scene operations, lands in Studio's
//! history, and redraws its canvas. With no Studio mounted the writer goes
//! on without them.
//!
//! When a write fails, keeps getting no answer, or the scene cannot be read
//! to settle one, the writer pauses rather than retrying in a loop, and
//! says so; the edits stay pending on the canvas. A new edit or a socket
//! that opens again resumes it.

use std::cell::RefCell;
use std::time::Duration;

use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};
use leptos::prelude::*;

use super::keyed::{self, Answer, RETRY_DELAYS};
use super::ledger::{
    Admission, Cancelled, Cause, Lapse, Lapsed, SceneLedgers, SceneRead, Ticket, Write, ZoneLedger,
    ZoneRead,
};
use crate::{api, toasts};

/// How many stale rejections in a row the writer takes before it pauses.
const STALE_STREAK: u32 = 3;

/// How many reads in a row may fall behind what the ledgers hold before
/// the writer pauses.
const BEHIND_STREAK: u32 = 3;

/// What the writer says when it pauses on reads that keep falling behind.
const BEHIND: &str = "Layout change not confirmed: the scene keeps reading older than the layout already confirmed. It is checked again when the connection is back or with your next edit.";

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
    /// The scene, read afresh by the writer.
    pub refetched: Callback<api::SceneDocument>,
    /// The ledgers changed, so a canvas may draw differently.
    pub changed: Callback<()>,
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
    /// Set while it holds for the socket to name a new daemon run, once the
    /// user has been told.
    holding: bool,
    hooks: Option<(u64, WriterHooks)>,
    next_hooks: u64,
    /// Whether the socket watch has started.
    watching: bool,
    /// The newest read refused for coming from a daemon run the socket had
    /// not named yet, offered again once it does: the document, the canvas
    /// it is drawn for, and the zone that wanted a ledger.
    refused: Option<Refused>,
}

struct Refused {
    scene: api::SceneDocument,
    canvas: (u32, u32),
    wanted: Option<String>,
}

impl Writer {
    fn new() -> Self {
        Self {
            ledgers: SceneLedgers::new(uuid::Uuid::new_v4().to_string()),
            canvas: None,
            writing: false,
            paused: false,
            holding: false,
            hooks: None,
            next_hooks: 0,
            watching: false,
            refused: None,
        }
    }

    /// Whether the writer has something to do now: a read to take, or a
    /// write to send or send again. A run takes a turn exactly when this
    /// holds.
    fn has_work(&self) -> bool {
        !self.paused
            && (self.ledgers.wants_read() || self.ledgers.wants_retry() || self.ledgers.write_due())
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
        run: scene.run_epoch.clone(),
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

/// Tell the user which pending edits were cancelled, and why.
fn report(cancelled: &[Cancelled]) {
    let count = |cause: Cause| -> usize {
        cancelled
            .iter()
            .filter(|entry| entry.cause == cause)
            .map(|entry| entry.output_ids.len())
            .sum()
    };
    match count(Cause::OutputLeft) {
        0 => {}
        1 => toasts::toast_info(
            "A layout change was dropped: its output left the zone before it was saved.",
        ),
        count => toasts::toast_info(&format!(
            "{count} layout changes were dropped: their outputs left the zone before they were saved."
        )),
    }
    match count(Cause::ZoneGone) {
        0 => {}
        1 => toasts::toast_info(
            "A layout change was dropped: its zone was removed before it was saved.",
        ),
        count => toasts::toast_info(&format!(
            "{count} layout changes were dropped: their zone was removed before they were saved."
        )),
    }
    match count(Cause::SceneChanged) {
        0 => {}
        1 => toasts::toast_info(
            "Another scene became active before a layout change was confirmed. It may not have been saved.",
        ),
        count => toasts::toast_info(&format!(
            "Another scene became active before {count} layout changes were confirmed. They may not have been saved."
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

/// Run `f` under the page's root owner, which lives as long as the page,
/// and leave the caller's owner current afterwards.
fn in_page_owner<T>(f: impl FnOnce() -> T) -> T {
    let mut root = Owner::current();
    while let Some(parent) = root.as_ref().and_then(Owner::parent) {
        root = Some(parent);
    }
    match root {
        Some(root) => root.with(f),
        None => f(),
    }
}

/// Watch the socket for as long as the page lives: `opened` counts its
/// openings, and `run` is the daemon run its last handshake named. Each
/// opening lets a paused writer try again, and a new run re-anchors the
/// ledgers first. Watching starts once; later calls change nothing.
pub(crate) fn watch_connection(opened: ReadSignal<u64>, run: ReadSignal<Option<String>>) {
    if with_writer(|writer| std::mem::replace(&mut writer.watching, true)) {
        return;
    }
    in_page_owner(|| {
        Effect::new(move |_| {
            opened.track();
            connected(run.get().as_deref());
        });
    });
}

/// The socket opened, its handshake naming daemon run `run`: the ledgers
/// enter that run, a read of it refused before is taken in now, and a
/// paused writer tries again.
fn connected(run: Option<&str>) {
    let refused = with_writer(|writer| {
        if let Some(run) = run {
            writer.ledgers.enter(run);
        }
        writer.holding = false;
        writer
            .refused
            .take_if(|refused| run == Some(refused.scene.run_epoch.as_str()))
    });
    if let Some(refused) = refused {
        observe(&refused.scene, refused.canvas, refused.wanted.as_deref());
        changed();
    }
    resume();
}

/// Take in the active scene as a read of it reported, drawn for `canvas`,
/// making sure `wanted` has a ledger. A read from another daemon run than
/// the ledgers' is not taken in; the newest such read is kept, and taken in
/// once the socket names its run. Edits the read cancelled are reported.
pub(crate) fn observe(scene: &api::SceneDocument, canvas: (u32, u32), wanted: Option<&str>) {
    let read = scene_read(scene, canvas);
    let taken = with_writer(|writer| {
        writer.canvas = Some(canvas);
        let switching = writer.ledgers.scene_id() != Some(scene.id);
        let Some(cancelled) = writer.ledgers.observe(&read, wanted) else {
            writer.refused = Some(Refused {
                scene: scene.clone(),
                canvas,
                wanted: wanted.map(str::to_owned),
            });
            return None;
        };
        writer.refused = None;
        if switching {
            writer.paused = false;
        }
        Some(cancelled)
    });
    let Some(cancelled) = taken else {
        return;
    };
    report(&cancelled);
    wake();
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

/// A mark that every placement edit made from now on comes after, to take
/// as a membership write goes out.
pub(crate) fn edit_mark() -> u64 {
    super::ledger::edit_mark()
}

/// Drop pending placement edits for `output_ids` made up to `through`, the
/// [`edit_mark`] taken as a membership write of ours went out: it placed
/// those outputs, and an edit made before it must not land over it. Edits
/// made since are newer, and still go out.
pub(crate) fn forget(scene_id: SceneId, output_ids: &[String], through: u64) {
    with_writer(|writer| {
        if writer.ledgers.scene_id() != Some(scene_id) {
            return;
        }
        for (_, ledger) in writer.ledgers.zones_mut() {
            ledger.forget(output_ids, through);
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
    /// It took at least one turn and has nothing left to do for now.
    Done,
    /// It found nothing to do; nothing starts it again but new work.
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
        // Work that came in while the run was out (an edit, a socket that
        // opened again, a pause that no longer held because the ledgers
        // moved on) starts the writer again, ahead of the queue. A run that
        // found nothing to do, or could not claim the scene, does not.
        if matches!(ended, Ended::Done | Ended::Paused) {
            wake();
        }
        if let Some(hooks) = hooks() {
            let _ = hooks.idle.try_run(());
        }
    });
}

fn changed() {
    if let Some(hooks) = hooks() {
        let _ = hooks.changed.try_run(());
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

/// Stop until the socket names the daemon run a read came from. Nothing
/// failed, so the user hears of it once, and only while edits wait.
fn hold(ticket: Ticket) {
    let tell = with_writer(|writer| {
        if !writer.ledgers.admits(ticket) {
            return false;
        }
        writer.paused = true;
        let waiting = writer
            .ledgers
            .zones()
            .any(|(_, ledger)| ledger.has_pending());
        let tell = waiting && !writer.holding;
        writer.holding = true;
        tell
    });
    if tell {
        toasts::toast_info(
            "Layout changes wait to be saved: the daemon restarted, and they go out once the connection catches up.",
        );
    }
}

/// What one turn of the writer does, for the ledgers `ticket` names.
enum Turn {
    /// Read the scene: writes left open by a run that ended settle against
    /// it, and ledgers whose edits wait on a re-anchor take it.
    Read,
    /// Send a write no answer came back for again, under its own key,
    /// with how many of its attempts went unanswered.
    Retry(String, Write, u32),
    Send(String, Write),
}

fn next_turn() -> Option<(Ticket, Turn)> {
    with_writer(|writer| {
        if writer.paused {
            return None;
        }
        let ticket = writer.ledgers.ticket()?;
        if writer.ledgers.wants_read() {
            return Some((ticket, Turn::Read));
        }
        if let Some((zone_id, write)) = writer.ledgers.next_retry() {
            let unanswered = writer
                .ledgers
                .zone(&zone_id)
                .map_or(0, ZoneLedger::unanswered);
            return Some((ticket, Turn::Retry(zone_id, write, unanswered)));
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
    /// Sends no answer came back for.
    lost: u32,
    behind: u32,
}

/// Write until no zone has anything to send or settle. With a Studio
/// mounted, every turn holds a claim on the scene of its own and releases
/// it after; a claim refused ends the run. A wait before a retry holds no
/// claim.
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
            Turn::Read => read_and_settle(ticket, &mut streaks).await,
            Turn::Retry(zone_id, write, unanswered) => {
                send(ticket, &zone_id, write, unanswered, &mut streaks).await
            }
            Turn::Send(zone_id, write) => send(ticket, &zone_id, write, 0, &mut streaks).await,
        };
        release(held);
        match next {
            Next::Stop(ended) => return ended,
            Next::Wait(delay) => api::client::sleep(delay).await,
            Next::Go => {}
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
    /// Wait this long, holding nothing, then go on.
    Wait(Duration),
    Stop(Ended),
}

fn canvas() -> (u32, u32) {
    with_writer(|writer| writer.canvas).unwrap_or(crate::render_canvas::DEFAULT_RENDER_CANVAS)
}

/// What a fresh read for the writer came to.
enum Fresh {
    /// The read failed.
    Failed,
    /// Reads keep coming back older than what the ledgers hold.
    Behind,
    /// It came from another daemon run than the ledgers'.
    OtherRun,
    /// Taken in: the document and its read.
    Taken(Box<(api::SceneDocument, SceneRead)>),
    /// The ledgers are no longer the ones it was started for: they changed
    /// while it was out, or it showed another active scene and they
    /// switched to it.
    Moved,
}

/// Read the scene afresh for the ledgers `ticket` names. A read that
/// falls behind what the ledgers hold is read again, up to a streak.
async fn read_for(ticket: Ticket, streaks: &mut Streaks) -> Fresh {
    let Ok(scene) = api::fetch_active_scene_promptly().await else {
        return Fresh::Failed;
    };
    let read = scene_read(&scene, canvas());
    let admission = with_writer(|writer| {
        let admission = writer.ledgers.admit(ticket, &read);
        if matches!(admission, Admission::Switched(_)) {
            writer.paused = false;
        }
        admission
    });
    match admission {
        Admission::Taken(cancelled) => {
            streaks.behind = 0;
            report(&cancelled);
            Fresh::Taken(Box::new((scene, read)))
        }
        Admission::Switched(cancelled) => {
            report(&cancelled);
            changed();
            Fresh::Moved
        }
        Admission::Behind => {
            streaks.behind += 1;
            if streaks.behind >= BEHIND_STREAK {
                return Fresh::Behind;
            }
            Fresh::Moved
        }
        Admission::OtherRun => Fresh::OtherRun,
        Admission::Refused => Fresh::Moved,
    }
}

fn zone_in<'a>(read: &'a SceneRead, zone_id: &str) -> Option<&'a ZoneRead> {
    read.zones.iter().find(|zone| zone.zone_id == zone_id)
}

/// Hand an admitted read to a mounted Studio.
fn share(scene: api::SceneDocument) {
    if let Some(hooks) = hooks() {
        let _ = hooks.refetched.try_run(scene);
    }
    changed();
}

/// Read the scene for the ledgers `ticket` names and take it in: ledgers
/// waiting to re-anchor take it whatever its revision, and every write a
/// run that ended left open settles against it.
async fn read_and_settle(ticket: Ticket, streaks: &mut Streaks) -> Next {
    let (scene, read) = match read_for(ticket, streaks).await {
        Fresh::Taken(taken) => *taken,
        Fresh::Failed => {
            pause(
                ticket,
                "Layout change not confirmed: the scene could not be read. It is checked again when the connection is back or with your next edit.",
            );
            return Next::Stop(Ended::Paused);
        }
        Fresh::Behind => {
            pause(ticket, BEHIND);
            return Next::Stop(Ended::Paused);
        }
        Fresh::OtherRun => {
            hold(ticket);
            return Next::Stop(Ended::Paused);
        }
        // The ledgers are not the ones the read was for any more; the next
        // turn starts from where they are now.
        Fresh::Moved => return Next::Go,
    };
    let lapsed: Vec<Lapsed> = with_writer(|writer| {
        let open: Vec<String> = writer
            .ledgers
            .zones()
            .filter(|(_, ledger)| ledger.wants_read() && ledger.unsettled())
            .map(|(zone_id, _)| zone_id.to_owned())
            .collect();
        open.into_iter()
            .filter_map(|zone_id| {
                let zone = zone_in(&read, &zone_id)?;
                writer.ledgers.ledger_mut(&zone_id)?.settle_ended(
                    read.revision,
                    zone.raw.clone(),
                    zone.shown.clone(),
                )
            })
            .filter(|lapsed| lapsed.unconfirmed)
            .collect()
    });
    // A write the read could not settle (its zone missing from a read
    // older than its ledger) would only ask for the same read again.
    let still_open = with_writer(|writer| writer.ledgers.wants_read());
    share(scene);
    report_lapsed(&lapsed);
    if still_open {
        pause(
            ticket,
            "Layout change not confirmed: the scene read did not show its zone. It is checked again when the connection is back or with your next edit.",
        );
        return Next::Stop(Ended::Paused);
    }
    Next::Go
}

/// Tell the user which lapsed writes the fresh read did not confirm, and
/// why their keys could not settle them.
fn report_lapsed(lapsed: &[Lapsed]) {
    let count =
        |lapse: Lapse| -> usize { lapsed.iter().filter(|ended| ended.lapse == lapse).count() };
    match count(Lapse::Restart) {
        0 => {}
        1 => toasts::toast_error(
            "A layout change may not have been saved: the daemon restarted while it was on its way. Check the canvas.",
        ),
        count => toasts::toast_error(&format!(
            "{count} layout changes may not have been saved: the daemon restarted while they were on their way. Check the canvas."
        )),
    }
    match count(Lapse::Forgotten) {
        0 => {}
        1 => toasts::toast_error(
            "A layout change may not have been saved: its answer was lost, and the answer to its retry could not say whether it went through. Check the canvas.",
        ),
        count => toasts::toast_error(&format!(
            "{count} layout changes may not have been saved: their answers were lost, and the answers to their retries could not say whether they went through. Check the canvas."
        )),
    }
    match count(Lapse::Unreadable) {
        0 => {}
        1 => toasts::toast_error(
            "A layout change was saved, but its reply could not be read and the scene shows something else now. Check the canvas.",
        ),
        count => toasts::toast_error(&format!(
            "{count} layout changes were saved, but their replies could not be read and the scene shows something else now. Check the canvas."
        )),
    }
}

/// After a stale rejection: read the scene, settle what it satisfies, and
/// go again, up to a streak.
async fn after_stale(zone_id: &str, id: u64, streaks: &mut Streaks) -> Next {
    let ticket = with_writer(|writer| {
        if let Some(ledger) = writer.ledgers.ledger_mut(zone_id) {
            ledger.stale(id);
        }
        writer.ledgers.ticket()
    });
    let Some(ticket) = ticket else {
        return Next::Go;
    };
    streaks.stale += 1;
    let fresh = read_for(ticket, streaks).await;
    match fresh {
        Fresh::Failed => {
            pause(
                ticket,
                "Layout change not saved yet: the scene could not be read. It goes out again when the connection is back or with your next edit.",
            );
            return Next::Stop(Ended::Paused);
        }
        Fresh::Behind => {
            pause(ticket, BEHIND);
            return Next::Stop(Ended::Paused);
        }
        Fresh::OtherRun => {
            hold(ticket);
            return Next::Stop(Ended::Paused);
        }
        Fresh::Taken(taken) => {
            let (scene, read) = *taken;
            let cancelled = with_writer(|writer| {
                let zone = zone_in(&read, zone_id)?;
                Some(writer.ledgers.ledger_mut(zone_id)?.refetched(
                    read.revision,
                    zone.raw.clone(),
                    zone.shown.clone(),
                ))
            });
            if let Some(output_ids) = cancelled {
                report(&[Cancelled {
                    zone_id: zone_id.to_owned(),
                    output_ids,
                    cause: Cause::OutputLeft,
                }]);
            }
            share(scene);
        }
        Fresh::Moved => {}
    }
    // Checked whatever became of the read, so stale rejections can never
    // go on without end.
    if streaks.stale >= STALE_STREAK {
        pause(
            ticket,
            "Layout change not saved yet: the scene keeps changing elsewhere. It goes out again with your next edit.",
        );
        return Next::Stop(Ended::Paused);
    }
    Next::Go
}

/// Send one write, or send it again under its own key after `unanswered`
/// attempts got no answer, and take in what the daemon answers. The answer
/// settles the write by its id, which is unique to the page, whatever the
/// ledgers did while it was out.
async fn send(
    ticket: Ticket,
    zone_id: &str,
    write: Write,
    unanswered: u32,
    streaks: &mut Streaks,
) -> Next {
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
    let fence = api::Fence {
        revision: write.fence,
        run: write.run.clone(),
    };
    let response = api::zones::update_zone_layout(zone_id, &layout, &fence, &write.key).await;
    let (status, outcome) = match keyed::classify(&write.run, unanswered, response) {
        Answer::Final { status, outcome } => (status, outcome),
        Answer::Lost(_) => {
            with_writer(|writer| {
                if let Some(ledger) = writer.ledgers.ledger_mut(zone_id) {
                    ledger.lost(write.id);
                }
            });
            let wait = usize::try_from(streaks.lost)
                .ok()
                .and_then(|lost| RETRY_DELAYS.get(lost).copied());
            streaks.lost += 1;
            let Some(wait) = wait else {
                pause(
                    ticket,
                    "Layout change not confirmed yet: no answer from the daemon. It goes out again when the connection is back or with your next edit.",
                );
                return Next::Stop(Ended::Paused);
            };
            return Next::Wait(wait);
        }
        // Whatever the first request did happened in a run that has ended
        // (the daemon restarted), or the answer cannot say what it was: a
        // fresh read settles it.
        lapsed @ (Answer::OtherRun | Answer::Forgotten) => {
            let lapse = if matches!(lapsed, Answer::OtherRun) {
                Lapse::Restart
            } else {
                Lapse::Forgotten
            };
            with_writer(|writer| {
                if let Some(ledger) = writer.ledgers.ledger_mut(zone_id) {
                    ledger.stranded(write.id, lapse);
                }
            });
            changed();
            return Next::Go;
        }
    };
    streaks.lost = 0;
    match outcome {
        Ok(api::zones::ZoneOutcome::Applied((zone, landed))) => {
            let (raw, shown) = zone_outputs(&zone, canvas);
            let taken = with_writer(|writer| {
                let cancelled = writer
                    .ledgers
                    .ledger_mut(zone_id)?
                    .applied(write.id, landed, raw, shown)?;
                // When the reply's own fence is the very next commit after the
                // write's, only this write landed in between, so every other
                // zone still stands as confirmed there.
                if let Some(landed) = landed
                    && landed.checked_sub(write.fence) == Some(1)
                {
                    for (id, ledger) in writer.ledgers.zones_mut() {
                        if id != zone_id {
                            ledger.advance(write.fence, landed);
                        }
                    }
                }
                Some(cancelled)
            });
            changed();
            let Some(cancelled) = taken else {
                return Next::Go;
            };
            streaks.stale = 0;
            report(&[Cancelled {
                zone_id: zone_id.to_owned(),
                output_ids: cancelled,
                cause: Cause::OutputLeft,
            }]);
            // A reply that names no fence cannot be placed in history's
            // order of documents; the next read brings the scene instead.
            if let (Some(hooks), Some(landed)) = (hooks(), landed) {
                let _ = hooks.landed.try_run((write.fence, zone, landed));
            }
            Next::Go
        }
        Ok(api::zones::ZoneOutcome::Stale { .. }) => after_stale(zone_id, write.id, streaks).await,
        Err(error) => {
            let raced = matches!(
                &error,
                api::ApiError::Http { status: 409, details: Some(details), .. }
                    if details.get("kind").and_then(serde_json::Value::as_str)
                        == Some("scene_commit_superseded")
            );
            let unreadable = matches!(error, api::ApiError::Parse(_));
            match status {
                // A commit race: another write won, as with a stale fence.
                409 if raced => after_stale(zone_id, write.id, streaks).await,
                412 if unreadable => after_stale(zone_id, write.id, streaks).await,
                // It landed, but the reply cannot be read, and a retry would
                // only replay it: a fresh read settles it as landed.
                200..=299 if unreadable => {
                    with_writer(|writer| {
                        if let Some(ledger) = writer.ledgers.ledger_mut(zone_id) {
                            ledger.stranded(write.id, Lapse::Unreadable);
                        }
                    });
                    Next::Go
                }
                // Any other answer is final, and the write did not land.
                _ => {
                    with_writer(|writer| {
                        if let Some(ledger) = writer.ledgers.ledger_mut(zone_id) {
                            ledger.failed(write.id);
                        }
                    });
                    changed();
                    pause(
                        ticket,
                        &format!(
                            "Layout change not saved: {error}. It goes out again with your next edit."
                        ),
                    );
                    Next::Stop(Ended::Paused)
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
                    run: "run".to_owned(),
                    zones: vec![ZoneRead {
                        zone_id: "zone".to_owned(),
                        raw: vec![output(0.2)],
                        shown: vec![output(0.2)],
                    }],
                };
                assert!(writer.ledgers.observe(&read, Some("zone")).is_some());
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
                changed: Callback::new(|()| {}),
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

    #[test]
    fn a_read_of_a_run_the_socket_has_not_named_is_taken_once_it_does() {
        let owner = Owner::new();
        owner.with(|| {
            let scene = |revision: u64, run: &str, x: f32| {
                let mut scene: api::SceneDocument = serde_json::from_value(serde_json::json!({
                    "id": "33333333-3333-4333-8333-333333333333", "name": "Desk",
                    "kind": "named", "is_default": false, "revision": revision,
                    "zones": [{ "id": "11111111-1111-4111-8111-111111111111", "name": "Main",
                        "role": "primary", "enabled": true, "brightness": 1, "layers": [],
                        "members": [{ "id": "a", "device_id": "usb:a", "segment": "Main",
                            "name": "A" }],
                        "layout": { "placements": [{ "member": "a",
                            "position": { "x": x, "y": 0.5 }, "size": { "x": 0.1, "y": 0.1 },
                            "topology": { "type": "strip", "count": 10,
                                "direction": "left_to_right" } }] } }],
                }))
                .expect("valid scene fixture");
                scene.run_epoch = run.to_owned();
                scene
            };
            let zone = "11111111-1111-4111-8111-111111111111";
            let canvas = crate::render_canvas::DEFAULT_RENDER_CANVAS;
            connected(Some("run-a"));
            let before = scene(9, "run-a", 0.2);
            observe(&before, canvas, Some(zone));
            // The daemon restarted: the new run's read comes before the
            // socket's handshake names that run, so it waits.
            let after = scene(2, "run-b", 0.7);
            observe(&after, canvas, Some(zone));
            let revision = || {
                with_writer(|writer| {
                    writer
                        .ledgers
                        .zone(zone)
                        .and_then(super::super::ledger::ZoneLedger::revision)
                })
            };
            assert_eq!(revision(), Some(9));
            // Nothing else reads the scene again; the handshake alone brings
            // the waiting read in.
            connected(Some("run-b"));
            assert_eq!(revision(), Some(2));
            let shown = display(after.id, zone).expect("the zone");
            assert!((shown[0].position.x - 0.7).abs() < f32::EPSILON);
            assert!(with_writer(|writer| writer.refused.is_none()));
        });
        owner.cleanup();
    }

    #[test]
    fn the_page_owner_holds_the_watch_and_the_caller_keeps_its_own() {
        let page = Owner::new();
        page.with(|| {
            let studio = Owner::new();
            studio.with(|| {
                provide_context(7_u32);
                let inside = in_page_owner(|| Owner::current().map(|owner| owner.debug_id()));
                assert_eq!(inside, Some(page.debug_id()), "the watch outlives Studio");
                assert_eq!(
                    Owner::current().map(|owner| owner.debug_id()),
                    Some(studio.debug_id()),
                    "the caller's owner is current again"
                );
                assert_eq!(use_context::<u32>(), Some(7), "and its context resolves");
            });
        });
        page.cleanup();
    }
}
