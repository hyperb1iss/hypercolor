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
//! - **Unknown** (the reply was lost). Where the daemon honors the write's
//!   key, the same write goes out again under the same key until an answer
//!   comes back, and the daemon applies it at most once. Where it does
//!   not, the zone is read afresh first: at the write's own fence nothing
//!   has landed yet, so it goes out again; past it, the write may have
//!   landed and been followed by others, so it is taken as landed and its
//!   intents retire, since sending it again could undo a later remote
//!   write. When the fresh zone does not hold what it carried, the user is
//!   told.
//!
//! Between clients the daemon's order decides. A remote write that lands
//! after ours wins; a local intent that reaches the daemon later wins.
//! Undo and redo are edits like any other and make intents the same way.
//!
//! Revisions only compare within one daemon run, and a new run starts them
//! over. Each new connection to the daemon may lead to a new run, so when
//! one opens every zone re-anchors: a read started on an earlier connection
//! is never taken in again, the next read started on the new one becomes
//! confirmed whatever its revision, intents stay, and nothing goes out
//! fenced at a revision of the old run until that read comes. A write sent
//! before the connection opened is not settled by its reply, whose zone
//! may belong to the run that ended: it settles from a fresh read, as a
//! write whose reply was lost does.
//!
//! Confirmed holds the zone twice: `raw`, as the daemon stores it, which a
//! write starts from; and `shown`, as the canvas draws it (sizes normalized
//! for the canvas), which edits and the fresh-read checks compare against.
//!
//! [`SceneLedgers`] keeps the ledgers of the active scene. A read for any
//! other scene, or one the writer started before the ledgers last changed
//! scene or connection, is never admitted, and a read drops a zone's ledger
//! only when it is at least as new as that ledger. Write ids and keys are
//! unique to the page, so a reply can only ever settle the write it
//! answers.

use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::{Orientation, Output};

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

#[derive(Clone, Debug, PartialEq)]
struct Sent {
    write: Write,
    /// The newest intent the write carried.
    through: u64,
    /// The reply was lost.
    unknown: bool,
    /// Sent before a new connection opened, so possibly to a daemon run
    /// that has since ended: its fence means nothing to a new run, and
    /// neither does its key.
    old_run: bool,
}

/// A write to send: the zone's outputs to store, fenced at `fence`, under
/// `key`, which every retry of this write reuses.
#[derive(Clone, Debug, PartialEq)]
pub struct Write {
    pub id: u64,
    pub key: String,
    pub fence: u64,
    pub outputs: Vec<Output>,
}

/// How a fresh read settled a write whose reply was lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// Nothing had landed: its intents go out again.
    Resend,
    /// Taken as landed. `unconfirmed` when the fresh zone does not hold
    /// what it carried, so the user should check the canvas.
    Landed { unconfirmed: bool },
}

/// One zone's confirmed layout and its pending intents.
#[derive(Clone, Debug, Default)]
pub struct ZoneLedger {
    confirmed: Option<Confirmed>,
    pending: Vec<Intent>,
    sent: Option<Sent>,
    /// A new connection opened: the next read is taken whatever its
    /// revision, and nothing goes out until it comes.
    reanchor: bool,
    next_seq: u64,
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
    /// re-anchor on a new connection. Returns the ids of outputs whose
    /// intents it cancelled because the zone no longer holds them.
    ///
    /// The caller passes only reads of the ledger's own connection:
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

    /// A new connection opened, so the daemon may have started a new run
    /// whose revisions start over. The next read is confirmed whatever its
    /// revision, pending intents stay, and no write goes out until that
    /// read comes, so none is fenced at a revision of the old run.
    pub fn reanchor(&mut self) {
        self.reanchor = true;
        if let Some(sent) = self.sent.as_mut() {
            sent.old_run = true;
        }
    }

    /// Whether the ledger waits for its first read on a new connection.
    #[must_use]
    pub fn reanchoring(&self) -> bool {
        self.reanchor
    }

    /// Whether a fresh read is needed before anything else can go out: a
    /// lost write to settle, or intents waiting on a re-anchor.
    #[must_use]
    pub fn wants_read(&self) -> bool {
        self.unsettled() || (self.reanchor && self.has_pending())
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
    /// waiting to re-anchor holds a revision of another connection, so it
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
                    self.next_seq += 1;
                    self.pending.push(Intent {
                        seq: self.next_seq,
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

    /// Drop the pending intents for `output_ids`: a newer local operation
    /// placed those outputs itself, so a leftover edit must not land over
    /// it, not even through a write already out, whose outcome still
    /// settles the rest.
    pub fn forget(&mut self, output_ids: &[String]) {
        self.pending
            .retain(|intent| !output_ids.contains(&intent.output_id));
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

    /// Whether a write's reply was lost and its outcome is still open.
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
    /// at confirmed's revision, under the id and key `next` hands out, which
    /// must never repeat.
    pub fn take_write(&mut self, next: impl FnOnce() -> (u64, String)) -> Option<Write> {
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
            outputs,
        };
        self.sent = Some(Sent {
            write: write.clone(),
            through,
            unknown: false,
            old_run: false,
        });
        Some(write)
    }

    /// The write whose reply was lost, to send again under its own key,
    /// where the daemon honors keys and applies a write at most once. None
    /// for a write sent before a new connection opened, whose key a new run
    /// does not know: [`Self::settle_lost`] settles that one from a fresh
    /// read.
    #[must_use]
    pub fn retry(&self) -> Option<Write> {
        self.sent
            .as_ref()
            .filter(|sent| sent.unknown && !sent.old_run)
            .map(|sent| sent.write.clone())
    }

    fn settle(&mut self, id: u64) -> Option<Sent> {
        self.sent.take_if(|sent| sent.write.id == id)
    }

    /// Write `id` landed at `revision`: the intents it carried retire, and
    /// the reply's zone becomes confirmed if it is newer. Returns the ids of
    /// outputs whose intents that cancelled.
    ///
    /// `None` when the reply settles nothing: it answers no write still
    /// out, or a write sent before a new connection opened. That one landed
    /// in a run that may have ended, so its reply's zone and revision are
    /// not taken; it stays open, and [`Self::settle_lost`] settles it from
    /// a fresh read.
    pub fn applied(
        &mut self,
        id: u64,
        revision: u64,
        raw: Vec<Output>,
        shown: Vec<Output>,
    ) -> Option<Vec<String>> {
        let sent = self.sent.as_mut().filter(|sent| sent.write.id == id)?;
        if sent.old_run {
            sent.unknown = true;
            return None;
        }
        let sent = self.settle(id)?;
        self.pending.retain(|intent| intent.seq > sent.through);
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

    /// Write `id`'s reply was lost, so whether it landed is unknown. No new
    /// write goes out until its outcome is known: from the daemon's answer
    /// to [`Self::retry`], or from a fresh read through
    /// [`Self::settle_lost`].
    pub fn lost(&mut self, id: u64) {
        if let Some(sent) = self.sent.as_mut().filter(|sent| sent.write.id == id) {
            sent.unknown = true;
        }
    }

    /// Take in a fresh read after a lost reply, where the daemon does not
    /// honor keys (or the write was sent before a new connection opened),
    /// and settle the write it left open: at the write's fence nothing
    /// landed yet, so its intents stay to go out again; past it, or for a
    /// write sent before the connection opened, whose fence a new run's
    /// revisions say nothing about, the write is taken as landed and its
    /// intents retire. `None` when no write was open.
    pub fn settle_lost(
        &mut self,
        revision: u64,
        raw: Vec<Output>,
        shown: Vec<Output>,
    ) -> Option<Settled> {
        let sent = self.sent.take_if(|sent| sent.unknown)?;
        if !sent.old_run && revision <= sent.write.fence {
            self.confirm(revision, raw, shown);
            return Some(Settled::Resend);
        }
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
        Some(Settled::Landed { unconfirmed })
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
    /// The connection to the daemon the read was started on (see
    /// [`SceneLedgers::connect`]).
    pub connection: u64,
    pub zones: Vec<ZoneRead>,
}

/// Which ledgers a read was started for, so a read that comes back after
/// they changed scene or connection is not admitted.
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
    /// The newest connection to the daemon the ledgers know of.
    connection: u64,
    /// Moves on every change of scene and every new connection.
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

    /// The newest connection to the daemon the ledgers know of.
    #[must_use]
    pub fn connection(&self) -> u64 {
        self.connection
    }

    /// Connection `connection` to the daemon opened. Each new one may lead
    /// to a new daemon run, so every ledger re-anchors, and reads started
    /// before it are refused from now on. Returns whether it was new.
    pub fn connect(&mut self, connection: u64) -> bool {
        if connection <= self.connection {
            return false;
        }
        self.connection = connection;
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
    /// was started on a connection older than the newest one, so it is not
    /// taken in at all. A read started on a newer one opens that connection
    /// first.
    pub fn observe(&mut self, read: &SceneRead, wanted: Option<&str>) -> Option<Vec<Cancelled>> {
        if read.connection < self.connection {
            return None;
        }
        self.connect(read.connection);
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
    /// ones it was started for; `None` when it is not admitted.
    pub fn admit(&mut self, ticket: Ticket, read: &SceneRead) -> Option<Vec<Cancelled>> {
        if !self.admits(ticket)
            || read.scene_id != ticket.scene_id
            || read.connection != self.connection
        {
            return None;
        }
        Some(self.take_in(read))
    }

    /// Whether some ledger needs a fresh read before it can go on.
    #[must_use]
    pub fn wants_read(&self) -> bool {
        self.zones.iter().any(|(_, ledger)| ledger.wants_read())
    }

    /// Whether some ledger has a write to send now.
    #[must_use]
    pub fn write_due(&self) -> bool {
        self.zones.iter().any(|(_, ledger)| ledger.write_due())
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
            ..
        } = self;
        let (_, ledger) = zones.iter_mut().find(|(id, _)| id == zone_id)?;
        ledger.take_write(|| {
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
