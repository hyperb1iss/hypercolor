//! How a zone canvas and the daemon agree on the zone's layout while
//! edits are still on their way.
//!
//! # The contract
//!
//! Each zone keeps two things.
//!
//! - **Confirmed**: the zone as the daemon last reported it, with the scene
//!   revision of that report. A scene read replaces it when its revision is
//!   at least as new; a write's reply only when it is newer, since a read
//!   at the same revision is the daemon's own account of that revision.
//!   Nothing about it is inferred from values.
//! - **Pending intents**: the local edits the daemon has not confirmed, in
//!   the order they were made. Each names one output, one placement field,
//!   and the value the user gave that field.
//!
//! The canvas shows confirmed with every pending intent applied in order.
//! An edit adds intents only for the fields it changed from what the canvas
//! showed, so a field the user never touched never holds an intent. An
//! intent for an output the zone no longer holds is cancelled, and the
//! caller is told so it can tell the user.
//!
//! A write carries every intent pending when it is sent, each field at its
//! latest value, and is fenced at confirmed's revision. Fields with no
//! intent go out as the daemon stores them in confirmed, which under the
//! fence changes nothing. One write is out at a time, under a key of its
//! own that every retry of it reuses. Its outcome settles it:
//!
//! - **Applied**: the intents it carried retire, and the reply's zone is
//!   adopted if its revision is newer than confirmed.
//! - **Stale**: nothing retires. The zone is read afresh, a field whose
//!   fresh value already equals its latest intent drops its intents (that
//!   is how an edit that depended on a write the daemon already holds
//!   settles), and what remains goes out again.
//! - **Unknown** (no answer came back). The same write goes out again under
//!   the same key until the daemon answers. The daemon applies a key at
//!   most once and answers a repeat with its first answer, waiting for the
//!   first request when that one is still under way, so a request that
//!   commits late is never applied twice, and the answer that comes back
//!   settles the write as above, whatever it is. The daemon keeps an
//!   answer for a while only: a retry it no longer holds the answer for
//!   runs as a new request, and when that one meets a scene that moved
//!   past its fence, the first request may be what moved it (a later
//!   retry may get that refusal back as a replay). The write then lapses
//!   (see below) rather than settle as stale, since sending its intents
//!   again could undo a remote write that landed after it.
//!
//! Between clients the daemon's order decides. A remote write that lands
//! after ours wins; a local intent that reaches the daemon later wins.
//! Undo and redo are edits like any other and make intents the same way.
//!
//! Every read and reply names the daemon run it came from. Revisions only
//! compare within one run, and keys only mean something to the run that
//! saw them. When the daemon starts a new run every zone re-anchors: a read
//! from another run is never taken in, the first read of the new one
//! becomes confirmed whatever its revision, intents stay, and nothing goes
//! out fenced at a revision of the old run until that read comes.
//!
//! A write whose key can no longer settle it has lapsed ([`Lapse`]): one
//! sent in a run that ended or answered by another run, one whose answer
//! the daemon let go, one whose reply could not be read. Nothing it sent
//! can still commit (the old run commits nothing more, and the daemon only
//! lets an answer go once its request is done), so a fresh read settles
//! it: it is taken as landed and its intents retire, since sending it
//! again could undo a remote write that landed after it, and when the
//! fresh zone does not hold what it carried the user is told.
//!
//! Confirmed holds the zone twice: `raw`, as the daemon stores it, which a
//! write starts from; and `shown`, as the canvas draws it (sizes normalized
//! for the canvas), which edits and the fresh-read checks compare against.
//!
//! [`SceneLedgers`] keeps the ledgers of the active scene. A read the
//! writer started before the ledgers last changed scene or run is never
//! admitted; one that shows another scene, at least as new as what the
//! ledgers hold, switches them to it. A read drops a zone's ledger only
//! when it is at least as new as that ledger. Write ids and keys are
//! unique to the page, so a reply can only ever settle the write it
//! answers.

use std::sync::atomic::{AtomicU64, Ordering};

use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::{Orientation, Output};

/// The newest intent sequence number handed out. One sequence serves every
/// ledger of the page, so a mark taken before an operation is older than
/// every edit made after it, in any zone, whatever scene the ledgers hold.
static LAST_SEQ: AtomicU64 = AtomicU64::new(0);

/// A mark that every edit made from now on comes after: given to
/// [`ZoneLedger::forget`], it reaches only edits made before it.
#[must_use]
pub fn edit_mark() -> u64 {
    LAST_SEQ.load(Ordering::Relaxed)
}

/// One placement field of an output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    X,
    Y,
    Width,
    Height,
    Rotation,
    Scale,
    Orientation,
}

/// A placement field's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Number(f32),
    Orientation(Option<Orientation>),
}

impl Field {
    pub const ALL: [Self; 7] = [
        Self::X,
        Self::Y,
        Self::Width,
        Self::Height,
        Self::Rotation,
        Self::Scale,
        Self::Orientation,
    ];

    #[must_use]
    pub fn read(self, output: &Output) -> Value {
        match self {
            Self::X => Value::Number(output.position.x),
            Self::Y => Value::Number(output.position.y),
            Self::Width => Value::Number(output.size.x),
            Self::Height => Value::Number(output.size.y),
            Self::Rotation => Value::Number(output.rotation),
            Self::Scale => Value::Number(output.scale),
            Self::Orientation => Value::Orientation(output.orientation),
        }
    }

    pub fn write(self, output: &mut Output, value: Value) {
        match (self, value) {
            (Self::X, Value::Number(value)) => output.position.x = value,
            (Self::Y, Value::Number(value)) => output.position.y = value,
            (Self::Width, Value::Number(value)) => output.size.x = value,
            (Self::Height, Value::Number(value)) => output.size.y = value,
            (Self::Rotation, Value::Number(value)) => output.rotation = value,
            (Self::Scale, Value::Number(value)) => output.scale = value,
            (Self::Orientation, Value::Orientation(value)) => output.orientation = value,
            _ => {}
        }
    }
}

/// A local edit to one field of one output, not yet confirmed.
#[derive(Clone, Debug, PartialEq)]
pub struct Intent {
    /// Unique to the page, and greater than every edit's made before it.
    pub seq: u64,
    pub output_id: String,
    pub field: Field,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
struct Confirmed {
    revision: u64,
    raw: Vec<Output>,
    shown: Vec<Output>,
}

/// Why a write's key can no longer settle it, so a fresh read must.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lapse {
    /// The daemon run it was sent in ended, or another run answered it.
    Restart,
    /// It was refused for a scene its first request may have changed: the
    /// daemon may have let that request's answer go and run a retry of it
    /// afresh (see [`super::keyed::tells`]).
    Forgotten,
    /// The daemon took it, but its reply could not be read.
    Unreadable,
}

/// How a lapsed write settled against a fresh read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lapsed {
    /// Why its key could not settle it.
    pub lapse: Lapse,
    /// The fresh zone does not hold what it carried, so the user should
    /// check the canvas.
    pub unconfirmed: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Sent {
    write: Write,
    /// The newest intent the write carried.
    through: u64,
    /// No answer has come back for it.
    unknown: bool,
    /// How many of its attempts went out with no answer coming back.
    unanswered: u32,
    /// Its key settles nothing any more, and why.
    lapsed: Option<Lapse>,
}

/// A write to send: the zone's outputs to store, fenced at `fence`, a
/// revision of daemon run `run`, under `key`, which every retry of this
/// write reuses.
#[derive(Clone, Debug, PartialEq)]
pub struct Write {
    pub id: u64,
    pub key: String,
    pub fence: u64,
    pub run: String,
    pub outputs: Vec<Output>,
}

/// One zone's confirmed layout and its pending intents.
#[derive(Clone, Debug, Default)]
pub struct ZoneLedger {
    confirmed: Option<Confirmed>,
    pending: Vec<Intent>,
    sent: Option<Sent>,
    /// The daemon started a new run: the next read is taken whatever its
    /// revision, and nothing goes out until it comes.
    reanchor: bool,
}

fn apply(outputs: &mut [Output], intents: &[Intent]) {
    for intent in intents {
        if let Some(output) = outputs
            .iter_mut()
            .find(|output| output.id == intent.output_id)
        {
            intent.field.write(output, intent.value);
        }
    }
}

/// The latest intent per field among `intents`, by index.
fn is_latest(intents: &[Intent], index: usize) -> bool {
    let intent = &intents[index];
    !intents[index + 1..]
        .iter()
        .any(|later| later.output_id == intent.output_id && later.field == intent.field)
}

impl ZoneLedger {
    /// Make `raw` and `shown` confirmed at `revision`, cancelling intents
    /// for outputs it no longer holds. Returns the cancelled outputs' ids.
    fn adopt(&mut self, revision: u64, raw: Vec<Output>, shown: Vec<Output>) -> Vec<String> {
        let mut cancelled: Vec<String> = Vec::new();
        self.pending.retain(|intent| {
            let held = shown.iter().any(|output| output.id == intent.output_id);
            if !held && !cancelled.contains(&intent.output_id) {
                cancelled.push(intent.output_id.clone());
            }
            held
        });
        self.confirmed = Some(Confirmed {
            revision,
            raw,
            shown,
        });
        cancelled
    }

    /// Take in the zone as a scene read reported it at `revision`. A read
    /// as new as confirmed or newer replaces it (the same revision redraws
    /// it, after a canvas resize or over a reply's account of that
    /// revision); an older one changes nothing, unless the ledger waits to
    /// re-anchor in a new daemon run. Returns the ids of outputs whose
    /// intents it cancelled because the zone no longer holds them.
    ///
    /// The caller passes only reads of the ledger's own run:
    /// [`SceneLedgers`] refuses the others.
    pub fn confirm(&mut self, revision: u64, raw: Vec<Output>, shown: Vec<Output>) -> Vec<String> {
        if self.reanchor {
            // A write still out keeps its outcome open: its reply settles
            // nothing, and a fresh read settles it.
            self.reanchor = false;
            return self.adopt(revision, raw, shown);
        }
        if self
            .confirmed
            .as_ref()
            .is_some_and(|confirmed| revision < confirmed.revision)
        {
            return Vec::new();
        }
        self.adopt(revision, raw, shown)
    }

    /// The daemon started a new run, whose revisions say nothing about the
    /// old one's. The next read is confirmed whatever its revision, pending
    /// intents stay, and no write goes out until that read comes, so none
    /// is fenced at a revision of the old run. A write still out can no
    /// longer be settled by its key.
    pub fn reanchor(&mut self) {
        self.reanchor = true;
        if let Some(sent) = self.sent.as_mut() {
            sent.lapsed.get_or_insert(Lapse::Restart);
        }
    }

    /// Whether the ledger waits for its first read of a new daemon run.
    #[must_use]
    pub fn reanchoring(&self) -> bool {
        self.reanchor
    }

    /// Whether a fresh read is needed before anything else can go out: a
    /// write its key can no longer settle, or intents waiting on a
    /// re-anchor.
    #[must_use]
    pub fn wants_read(&self) -> bool {
        self.sent
            .as_ref()
            .is_some_and(|sent| sent.unknown && sent.lapsed.is_some())
            || (self.reanchor && self.has_pending())
    }

    /// Whether the write out has had no answer yet and can go out again
    /// under its key.
    #[must_use]
    pub fn wants_retry(&self) -> bool {
        self.retry().is_some()
    }

    /// Whether [`Self::take_write`] has a write to hand out now.
    #[must_use]
    pub fn write_due(&self) -> bool {
        self.sent.is_none() && !self.reanchor && self.confirmed.is_some() && self.has_pending()
    }

    /// Confirmed's outputs as the daemon stores them.
    #[must_use]
    pub fn confirmed(&self) -> Option<&[Output]> {
        self.confirmed
            .as_ref()
            .map(|confirmed| confirmed.raw.as_slice())
    }

    /// The revision confirmed is at.
    #[must_use]
    pub fn revision(&self) -> Option<u64> {
        self.confirmed.as_ref().map(|confirmed| confirmed.revision)
    }

    /// Move confirmed from revision `from` to `to` without changing it,
    /// when the only change between them is known to be elsewhere (another
    /// zone's write that landed at `to`, fenced at `from`). A ledger
    /// waiting to re-anchor holds a revision of another run, so it
    /// stays as it is.
    pub fn advance(&mut self, from: u64, to: u64) {
        if !self.reanchor
            && let Some(confirmed) = self.confirmed.as_mut()
            && confirmed.revision == from
        {
            confirmed.revision = to;
        }
    }

    /// What the canvas shows: confirmed with every pending intent applied.
    #[must_use]
    pub fn display(&self) -> Option<Vec<Output>> {
        let mut shown = self.confirmed.as_ref()?.shown.clone();
        apply(&mut shown, &self.pending);
        Some(shown)
    }

    /// Record the canvas's outputs after an edit: an intent for each field
    /// that differs from what the canvas showed. Reports whether any did.
    pub fn edit(&mut self, edited: &[Output]) -> bool {
        let Some(display) = self.display() else {
            return false;
        };
        let mut changed = false;
        for shown in &display {
            let Some(local) = edited.iter().find(|local| local.id == shown.id) else {
                continue;
            };
            for field in Field::ALL {
                let value = field.read(local);
                if value != field.read(shown) {
                    self.pending.push(Intent {
                        seq: LAST_SEQ.fetch_add(1, Ordering::Relaxed) + 1,
                        output_id: shown.id.clone(),
                        field,
                        value,
                    });
                    changed = true;
                }
            }
        }
        changed
    }

    /// Drop the pending intents for `output_ids` made up to `through`, an
    /// [`edit_mark`] taken as a local operation that placed those outputs
    /// itself went out: an edit made before it must not land over it, not
    /// even through a write already out, whose outcome still settles the
    /// rest. An edit made after it is newer, and goes out as usual.
    pub fn forget(&mut self, output_ids: &[String], through: u64) {
        self.pending
            .retain(|intent| intent.seq > through || !output_ids.contains(&intent.output_id));
    }

    /// Whether any edit waits for the daemon, sent or not.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Whether a write is out and its outcome not yet known.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        self.sent.as_ref().is_some_and(|sent| !sent.unknown)
    }

    /// Whether no answer has come back for the write out, so its outcome
    /// is still open.
    #[must_use]
    pub fn unsettled(&self) -> bool {
        self.sent.as_ref().is_some_and(|sent| sent.unknown)
    }

    /// The pending intents, oldest first.
    #[must_use]
    pub fn pending(&self) -> &[Intent] {
        &self.pending
    }

    /// The next write when one is due (see [`Self::write_due`]): confirmed's
    /// stored outputs with each intent's field at its latest value, fenced
    /// at confirmed's revision, a revision of daemon run `run`, under the id
    /// and key `next` hands out, which must never repeat.
    pub fn take_write(&mut self, run: &str, next: impl FnOnce() -> (u64, String)) -> Option<Write> {
        if !self.write_due() {
            return None;
        }
        let through = self.pending.last()?.seq;
        let confirmed = self.confirmed.as_ref()?;
        let mut outputs = confirmed.raw.clone();
        apply(&mut outputs, &self.pending);
        let (id, key) = next();
        let write = Write {
            id,
            key,
            fence: confirmed.revision,
            run: run.to_owned(),
            outputs,
        };
        self.sent = Some(Sent {
            write: write.clone(),
            through,
            unknown: false,
            unanswered: 0,
            lapsed: None,
        });
        Some(write)
    }

    /// How many attempts of the write out went with no answer coming back,
    /// which says how much an answer to the next one can tell (see
    /// [`super::keyed::tells`]).
    #[must_use]
    pub fn unanswered(&self) -> u32 {
        self.sent.as_ref().map_or(0, |sent| sent.unanswered)
    }

    /// The write no answer came back for, to send again under its own key:
    /// the daemon applies it at most once. None for a write its key can no
    /// longer settle, which [`Self::settle_ended`] settles from a fresh
    /// read.
    #[must_use]
    pub fn retry(&self) -> Option<Write> {
        self.sent
            .as_ref()
            .filter(|sent| sent.unknown && sent.lapsed.is_none())
            .map(|sent| sent.write.clone())
    }

    fn settle(&mut self, id: u64) -> Option<Sent> {
        self.sent.take_if(|sent| sent.write.id == id)
    }

    /// Write `id` landed at `revision`: the intents it carried retire, and
    /// the reply's zone becomes confirmed if it is newer. Returns the ids of
    /// outputs whose intents that cancelled. A reply that names no revision
    /// (`None`) retires the intents and leaves confirmed as it is, since its
    /// zone cannot be ordered among the reads; nothing is guessed for it.
    ///
    /// `None` when the reply settles nothing: it answers no write still
    /// out, or a write sent in a run that has since ended. That one landed
    /// in the old run, whose revisions mean nothing now, so its reply's
    /// zone is not taken; it stays open, and [`Self::settle_ended`] settles
    /// it from a fresh read.
    pub fn applied(
        &mut self,
        id: u64,
        revision: Option<u64>,
        raw: Vec<Output>,
        shown: Vec<Output>,
    ) -> Option<Vec<String>> {
        let sent = self.sent.as_mut().filter(|sent| sent.write.id == id)?;
        if sent.lapsed.is_some() {
            sent.unknown = true;
            return None;
        }
        let sent = self.settle(id)?;
        self.pending.retain(|intent| intent.seq > sent.through);
        let Some(revision) = revision else {
            return Some(Vec::new());
        };
        if self
            .confirmed
            .as_ref()
            .is_none_or(|confirmed| revision > confirmed.revision)
        {
            return Some(self.adopt(revision, raw, shown));
        }
        Some(Vec::new())
    }

    /// Write `id` met a newer revision than its fence. Its intents stay;
    /// [`Self::refetched`] takes in the zone as it is now.
    pub fn stale(&mut self, id: u64) {
        self.settle(id);
    }

    /// Write `id` was refused outright, so it did not land. Its intents
    /// stay for the next write.
    pub fn failed(&mut self, id: u64) {
        self.settle(id);
    }

    /// No answer came back for write `id`, so whether it landed is
    /// unknown. No new write goes out until the daemon answers
    /// [`Self::retry`].
    pub fn lost(&mut self, id: u64) {
        if let Some(sent) = self.sent.as_mut().filter(|sent| sent.write.id == id) {
            sent.unknown = true;
            sent.unanswered += 1;
        }
    }

    /// Write `id` lapsed for `lapse`: its key can no longer say whether it
    /// landed, and [`Self::settle_ended`] settles it from a fresh read.
    pub fn stranded(&mut self, id: u64, lapse: Lapse) {
        if let Some(sent) = self.sent.as_mut().filter(|sent| sent.write.id == id) {
            sent.unknown = true;
            sent.lapsed.get_or_insert(lapse);
        }
    }

    /// Take in a fresh read of the current daemon run and settle the write
    /// that lapsed. Nothing it sent can commit any more, so the read shows
    /// whatever the write did, but not whether a remote write followed it:
    /// the write is taken as landed and its intents retire. `None` when no
    /// such write was open.
    pub fn settle_ended(
        &mut self,
        revision: u64,
        raw: Vec<Output>,
        shown: Vec<Output>,
    ) -> Option<Lapsed> {
        let sent = self
            .sent
            .take_if(|sent| sent.unknown && sent.lapsed.is_some())?;
        let lapse = sent.lapsed?;
        // Given up on: a field the write carried with no newer intent to
        // send after it, which the fresh zone does not hold (a zone without
        // the output at all holds nothing).
        let unconfirmed = self.pending.iter().enumerate().any(|(index, intent)| {
            intent.seq <= sent.through
                && is_latest(&self.pending, index)
                && shown
                    .iter()
                    .find(|output| output.id == intent.output_id)
                    .is_none_or(|output| intent.field.read(output) != intent.value)
        });
        self.pending.retain(|intent| intent.seq > sent.through);
        self.confirm(revision, raw, shown);
        Some(Lapsed { lapse, unconfirmed })
    }

    /// Take in the zone read afresh after a stale rejection, then drop the
    /// intents it already satisfies: a field whose latest intent equals the
    /// fresh value is settled, earlier intents for that field included.
    /// Returns the ids of outputs whose intents it cancelled.
    pub fn refetched(
        &mut self,
        revision: u64,
        raw: Vec<Output>,
        shown: Vec<Output>,
    ) -> Vec<String> {
        let cancelled = self.confirm(revision, raw, shown);
        let Some(confirmed) = self.confirmed.as_ref() else {
            return cancelled;
        };
        let mut settled: Vec<(String, Field)> = Vec::new();
        for (index, intent) in self.pending.iter().enumerate() {
            let satisfied = confirmed
                .shown
                .iter()
                .find(|output| output.id == intent.output_id)
                .is_some_and(|output| intent.field.read(output) == intent.value);
            if is_latest(&self.pending, index) && satisfied {
                settled.push((intent.output_id.clone(), intent.field));
            }
        }
        self.pending.retain(|intent| {
            !settled
                .iter()
                .any(|(output_id, field)| *output_id == intent.output_id && *field == intent.field)
        });
        cancelled
    }
}

/// A zone as one scene read reported it.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneRead {
    pub zone_id: String,
    /// As the daemon stores it.
    pub raw: Vec<Output>,
    /// As the canvas draws it.
    pub shown: Vec<Output>,
}

/// One scene read: every zone of the scene at one revision.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneRead {
    pub scene_id: SceneId,
    pub revision: u64,
    /// The daemon run `revision` belongs to; empty from a daemon that
    /// names no runs.
    pub run: String,
    pub zones: Vec<ZoneRead>,
}

/// Which ledgers a read was started for, so a read that comes back after
/// they changed scene or run is not admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    scene_id: SceneId,
    generation: u64,
}

/// Why pending edits were cancelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The zone no longer holds the output.
    OutputLeft,
    /// The zone is gone from the scene.
    ZoneGone,
    /// Another scene became active before they were confirmed. One of them
    /// may have been on its way, so it may have been saved after all.
    SceneChanged,
}

/// What became of a read the writer started under a [`Ticket`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Taken in. Holds the edits it cancelled.
    Taken(Vec<Cancelled>),
    /// It showed another scene than the one it was started for, while
    /// nothing else changed the ledgers: the active scene changed, so the
    /// ledgers switched to it. Holds the edits the switch cancelled.
    Switched(Vec<Cancelled>),
    /// It showed another scene, but at a revision older than one the
    /// ledgers hold, so it says nothing about which scene is active now
    /// (the daemon went there and back while it was out). Read again.
    Behind,
    /// It came from another daemon run than the ledgers': the daemon
    /// restarted, and the run that answered becomes theirs once the socket
    /// names it (see [`SceneLedgers::enter`]).
    OtherRun,
    /// The ledgers changed scene or run while it was out.
    Refused,
}

/// Pending edits that were cancelled before the daemon confirmed them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cancelled {
    pub zone_id: String,
    pub output_ids: Vec<String>,
    pub cause: Cause,
}

/// The zone ledgers of the active scene.
#[derive(Clone, Debug, Default)]
pub struct SceneLedgers {
    scene_id: Option<SceneId>,
    /// The daemon run the ledgers belong to; empty until one is known.
    run: String,
    /// Moves on every change of scene and of run.
    generation: u64,
    /// Zone ledgers, in the order their zones were first seen.
    zones: Vec<(String, ZoneLedger)>,
    /// The last write id handed out, for the life of the page.
    last_write: u64,
    /// Makes write keys unique to this page.
    key_prefix: String,
}

impl SceneLedgers {
    /// Ledgers whose write keys start with `key_prefix`, which must be
    /// unique to this page.
    #[must_use]
    pub fn new(key_prefix: impl Into<String>) -> Self {
        Self {
            key_prefix: key_prefix.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn scene_id(&self) -> Option<SceneId> {
        self.scene_id
    }

    /// The daemon run the ledgers belong to; empty until one is known.
    #[must_use]
    pub fn run(&self) -> &str {
        &self.run
    }

    /// The daemon runs as run `run` now, as the socket's handshake names
    /// it. A run other than theirs means the daemon restarted, so every
    /// ledger re-anchors, and reads and replies of the old run are refused
    /// from now on. Returns whether the run changed.
    pub fn enter(&mut self, run: &str) -> bool {
        if run.is_empty() || run == self.run {
            return false;
        }
        run.clone_into(&mut self.run);
        self.generation += 1;
        for (_, ledger) in &mut self.zones {
            ledger.reanchor();
        }
        true
    }

    /// The ticket a read started now carries.
    #[must_use]
    pub fn ticket(&self) -> Option<Ticket> {
        self.scene_id.map(|scene_id| Ticket {
            scene_id,
            generation: self.generation,
        })
    }

    /// Whether a read started under `ticket` may still be taken in.
    #[must_use]
    pub fn admits(&self, ticket: Ticket) -> bool {
        self.ticket() == Some(ticket)
    }

    /// Make `scene_id` the active scene. Switching drops the other scene's
    /// ledgers; their pending edits are reported cancelled.
    pub fn select(&mut self, scene_id: SceneId) -> Vec<Cancelled> {
        if self.scene_id == Some(scene_id) {
            return Vec::new();
        }
        let dropped = std::mem::take(&mut self.zones)
            .into_iter()
            .filter(|(_, ledger)| ledger.has_pending())
            .map(|(zone_id, ledger)| Cancelled {
                zone_id,
                output_ids: outputs_of(ledger.pending()),
                cause: Cause::SceneChanged,
            })
            .collect();
        self.scene_id = Some(scene_id);
        self.generation += 1;
        dropped
    }

    #[must_use]
    pub fn zone(&self, zone_id: &str) -> Option<&ZoneLedger> {
        self.zones
            .iter()
            .find(|(id, _)| id == zone_id)
            .map(|(_, ledger)| ledger)
    }

    /// The zone's ledger, if it has one.
    pub fn ledger_mut(&mut self, zone_id: &str) -> Option<&mut ZoneLedger> {
        self.zones
            .iter_mut()
            .find(|(id, _)| id == zone_id)
            .map(|(_, ledger)| ledger)
    }

    /// The zone's ledger, made empty when it has none.
    pub fn zone_mut(&mut self, zone_id: &str) -> &mut ZoneLedger {
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

    pub fn zones_mut(&mut self) -> impl Iterator<Item = (&str, &mut ZoneLedger)> {
        self.zones
            .iter_mut()
            .map(|(id, ledger)| (id.as_str(), ledger))
    }

    pub fn zones(&self) -> impl Iterator<Item = (&str, &ZoneLedger)> {
        self.zones.iter().map(|(id, ledger)| (id.as_str(), ledger))
    }

    /// The active scene as a read of it reported: it becomes the active
    /// scene, `wanted` gets a ledger if it has none, and every ledger takes
    /// the read in. Returns the edits it cancelled, or `None` when the read
    /// comes from another daemon run than the ledgers', so it is not taken
    /// in at all. Until a run is known, the first read's run becomes it.
    pub fn observe(&mut self, read: &SceneRead, wanted: Option<&str>) -> Option<Vec<Cancelled>> {
        if !self.of_run(read) {
            return None;
        }
        let mut cancelled = self.select(read.scene_id);
        if let Some(zone_id) = wanted
            && read.zones.iter().any(|zone| zone.zone_id == zone_id)
        {
            self.zone_mut(zone_id);
        }
        cancelled.extend(self.take_in(read));
        Some(cancelled)
    }

    /// A read started under `ticket`, such as the writer's own after a
    /// stale or lost reply. Taken in only while the ledgers are still the
    /// ones it was started for. One daemon run orders every scene's commits
    /// in one sequence of revisions, so a read of another scene under a
    /// ticket that still holds, at least as new as anything the ledgers
    /// hold, means the active scene changed: they switch to it, as
    /// [`Self::observe`] would.
    pub fn admit(&mut self, ticket: Ticket, read: &SceneRead) -> Admission {
        if !self.admits(ticket) {
            return Admission::Refused;
        }
        if !self.of_run(read) {
            return Admission::OtherRun;
        }
        if read.scene_id != ticket.scene_id {
            if self
                .newest_revision()
                .is_some_and(|newest| read.revision < newest)
            {
                return Admission::Behind;
            }
            return Admission::Switched(self.observe(read, None).unwrap_or_default());
        }
        Admission::Taken(self.take_in(read))
    }

    /// Whether `read` belongs to the ledgers' daemon run. Until one is
    /// known, the read's run becomes theirs.
    fn of_run(&mut self, read: &SceneRead) -> bool {
        if self.run.is_empty() {
            self.enter(&read.run);
            return true;
        }
        read.run == self.run
    }

    /// The newest revision a ledger holds in the current run.
    fn newest_revision(&self) -> Option<u64> {
        self.zones
            .iter()
            .filter(|(_, ledger)| !ledger.reanchoring())
            .filter_map(|(_, ledger)| ledger.revision())
            .max()
    }

    /// Whether some ledger needs a fresh read before it can go on.
    #[must_use]
    pub fn wants_read(&self) -> bool {
        self.zones.iter().any(|(_, ledger)| ledger.wants_read())
    }

    /// Whether some ledger has a write to send again under its key.
    #[must_use]
    pub fn wants_retry(&self) -> bool {
        self.zones.iter().any(|(_, ledger)| ledger.wants_retry())
    }

    /// Whether some ledger has a write to send now.
    #[must_use]
    pub fn write_due(&self) -> bool {
        self.zones.iter().any(|(_, ledger)| ledger.write_due())
    }

    /// The first zone with a write to send again under its key, and that
    /// write.
    #[must_use]
    pub fn next_retry(&self) -> Option<(String, Write)> {
        self.zones
            .iter()
            .find_map(|(zone_id, ledger)| ledger.retry().map(|write| (zone_id.clone(), write)))
    }

    /// Every ledger takes in its zone from `read`. A ledger whose zone the
    /// read does not hold goes, but only when the read is at least as new
    /// as that ledger: an older read cannot know of a zone made since.
    fn take_in(&mut self, read: &SceneRead) -> Vec<Cancelled> {
        let mut cancelled = Vec::new();
        self.zones.retain(|(zone_id, ledger)| {
            let held = read.zones.iter().any(|zone| zone.zone_id == *zone_id);
            let older = !ledger.reanchoring()
                && ledger
                    .revision()
                    .is_some_and(|revision| read.revision < revision);
            if held || older {
                return true;
            }
            if ledger.has_pending() {
                cancelled.push(Cancelled {
                    zone_id: zone_id.clone(),
                    output_ids: outputs_of(ledger.pending()),
                    cause: Cause::ZoneGone,
                });
            }
            false
        });
        for zone in &read.zones {
            let Some(ledger) = self.ledger_mut(&zone.zone_id) else {
                continue;
            };
            let output_ids = ledger.confirm(read.revision, zone.raw.clone(), zone.shown.clone());
            if !output_ids.is_empty() {
                cancelled.push(Cancelled {
                    zone_id: zone.zone_id.clone(),
                    output_ids,
                    cause: Cause::OutputLeft,
                });
            }
        }
        cancelled
    }

    /// The next write for `zone_id`, under an id and a key unique to this
    /// page.
    pub fn take_write(&mut self, zone_id: &str) -> Option<Write> {
        let Self {
            zones,
            last_write,
            key_prefix,
            run,
            ..
        } = self;
        let (_, ledger) = zones.iter_mut().find(|(id, _)| id == zone_id)?;
        ledger.take_write(run, || {
            *last_write += 1;
            (*last_write, format!("{key_prefix}:{last_write}"))
        })
    }

    /// The first zone with a write to send, and that write.
    pub fn next_write(&mut self) -> Option<(String, Write)> {
        let zone_id = self
            .zones
            .iter()
            .find(|(_, ledger)| ledger.write_due())
            .map(|(id, _)| id.clone())?;
        self.take_write(&zone_id).map(|write| (zone_id, write))
    }
}

fn outputs_of(intents: &[Intent]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for intent in intents {
        if !ids.contains(&intent.output_id) {
            ids.push(intent.output_id.clone());
        }
    }
    ids
}
