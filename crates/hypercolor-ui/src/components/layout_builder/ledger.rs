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
//! showed, so a field the user never touched never holds an intent.
//!
//! A write carries every intent pending when it is sent, each field at its
//! latest value, and is fenced at confirmed's revision. Fields with no
//! intent go out as the daemon stores them in confirmed, which under the
//! fence changes nothing. One write is out at a time. Its outcome settles
//! it:
//!
//! - **Applied**: the intents it carried retire, and the reply's zone is
//!   adopted if its revision is newer than confirmed.
//! - **Stale**: nothing retires. The zone is read afresh, a field whose
//!   fresh value already equals its latest intent drops its intents (that
//!   is how an edit that depended on a write the daemon already holds
//!   settles), and what remains goes out again.
//! - **Unknown** (the reply was lost): the zone is read afresh before
//!   anything else goes out. At the write's own fence nothing landed, so it
//!   goes out again. Past it, the write may have landed and been followed
//!   by others, so it is taken as landed and its intents retire: sending it
//!   again could undo a later remote write. When the fresh zone does not
//!   hold what it carried, the user is told.
//!
//! Between clients the daemon's order decides. A remote write that lands
//! after ours wins; a local intent that reaches the daemon later wins.
//! Undo and redo are edits like any other and make intents the same way.
//!
//! Confirmed holds the zone twice: `raw`, as the daemon stores it, which a
//! write starts from; and `shown`, as the canvas draws it (sizes normalized
//! for the canvas), which edits and the fresh-read checks compare against.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sent {
    id: u64,
    fence: u64,
    /// The newest intent the write carried.
    through: u64,
    /// The reply was lost; a fresh read must settle it.
    unknown: bool,
}

/// A write to send: the zone's outputs to store, fenced at `fence`.
#[derive(Clone, Debug, PartialEq)]
pub struct Write {
    pub id: u64,
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
    next_seq: u64,
    next_write: u64,
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

impl ZoneLedger {
    fn adopt(&mut self, revision: u64, raw: Vec<Output>, shown: Vec<Output>) {
        self.pending
            .retain(|intent| shown.iter().any(|output| output.id == intent.output_id));
        self.confirmed = Some(Confirmed {
            revision,
            raw,
            shown,
        });
    }

    /// Take in the zone as a scene read reported it at `revision`. A read
    /// as new as confirmed or newer replaces it (the same revision redraws
    /// it, after a canvas resize or over a reply's account of that
    /// revision); an older one changes nothing. Intents for outputs the
    /// zone no longer holds go with them.
    pub fn confirm(&mut self, revision: u64, raw: Vec<Output>, shown: Vec<Output>) {
        if self
            .confirmed
            .as_ref()
            .is_some_and(|confirmed| revision < confirmed.revision)
        {
            return;
        }
        self.adopt(revision, raw, shown);
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
    /// zone's write that landed at `to`, fenced at `from`).
    pub fn advance(&mut self, from: u64, to: u64) {
        if let Some(confirmed) = self.confirmed.as_mut()
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

    /// Whether any edit waits for the daemon, sent or not.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Whether a write is out and its outcome not yet known.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        self.sent.is_some_and(|sent| !sent.unknown)
    }

    /// Whether a write's reply was lost, so a fresh read must come first.
    #[must_use]
    pub fn unsettled(&self) -> bool {
        self.sent.is_some_and(|sent| sent.unknown)
    }

    /// The pending intents, oldest first.
    #[must_use]
    pub fn pending(&self) -> &[Intent] {
        &self.pending
    }

    /// The next write, when intents wait and no write is out or unsettled:
    /// confirmed's stored outputs with each intent's field at its latest
    /// value, fenced at confirmed's revision.
    pub fn take_write(&mut self) -> Option<Write> {
        if self.sent.is_some() {
            return None;
        }
        let through = self.pending.last()?.seq;
        let confirmed = self.confirmed.as_ref()?;
        let mut outputs = confirmed.raw.clone();
        apply(&mut outputs, &self.pending);
        self.next_write += 1;
        self.sent = Some(Sent {
            id: self.next_write,
            fence: confirmed.revision,
            through,
            unknown: false,
        });
        Some(Write {
            id: self.next_write,
            fence: confirmed.revision,
            outputs,
        })
    }

    fn settle(&mut self, id: u64) -> Option<Sent> {
        let sent = self.sent.filter(|sent| sent.id == id && !sent.unknown)?;
        self.sent = None;
        Some(sent)
    }

    /// Write `id` landed at `revision`: the intents it carried retire, and
    /// the reply's zone becomes confirmed if it is newer.
    pub fn applied(&mut self, id: u64, revision: u64, raw: Vec<Output>, shown: Vec<Output>) {
        let Some(sent) = self.settle(id) else {
            return;
        };
        self.pending.retain(|intent| intent.seq > sent.through);
        if self
            .confirmed
            .as_ref()
            .is_none_or(|confirmed| revision > confirmed.revision)
        {
            self.adopt(revision, raw, shown);
        }
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

    /// Write `id`'s reply was lost, so whether it landed is unknown. No
    /// write goes out until [`Self::settle_lost`] takes in a fresh read.
    pub fn lost(&mut self, id: u64) {
        if let Some(sent) = self.sent.as_mut().filter(|sent| sent.id == id) {
            sent.unknown = true;
        }
    }

    /// Take in a fresh read after a lost reply, and settle the write it
    /// left unknown: at the write's fence nothing landed, so its intents
    /// stay to go out again; past it, the write is taken as landed and its
    /// intents retire. `None` when no write was unknown.
    pub fn settle_lost(
        &mut self,
        revision: u64,
        raw: Vec<Output>,
        shown: Vec<Output>,
    ) -> Option<Settled> {
        let sent = self.sent.filter(|sent| sent.unknown)?;
        self.sent = None;
        if revision <= sent.fence {
            self.confirm(revision, raw, shown);
            return Some(Settled::Resend);
        }
        // Given up on: a field the write carried with no newer intent to
        // send after it, which the fresh zone does not hold.
        let unconfirmed = self.pending.iter().enumerate().any(|(index, intent)| {
            let latest = !self.pending[index + 1..]
                .iter()
                .any(|later| later.output_id == intent.output_id && later.field == intent.field);
            intent.seq <= sent.through
                && latest
                && shown
                    .iter()
                    .find(|output| output.id == intent.output_id)
                    .is_some_and(|output| intent.field.read(output) != intent.value)
        });
        self.pending.retain(|intent| intent.seq > sent.through);
        self.confirm(revision, raw, shown);
        Some(Settled::Landed { unconfirmed })
    }

    /// Take in the zone read afresh after a stale rejection, then drop the
    /// intents it already satisfies: a field whose latest intent equals the
    /// fresh value is settled, earlier intents for that field included.
    pub fn refetched(&mut self, revision: u64, raw: Vec<Output>, shown: Vec<Output>) {
        self.confirm(revision, raw, shown);
        let Some(confirmed) = self.confirmed.as_ref() else {
            return;
        };
        let mut settled: Vec<(String, Field)> = Vec::new();
        for (index, intent) in self.pending.iter().enumerate() {
            let latest = !self.pending[index + 1..]
                .iter()
                .any(|later| later.output_id == intent.output_id && later.field == intent.field);
            let satisfied = confirmed
                .shown
                .iter()
                .find(|output| output.id == intent.output_id)
                .is_some_and(|output| intent.field.read(output) == intent.value);
            if latest && satisfied {
                settled.push((intent.output_id.clone(), intent.field));
            }
        }
        self.pending.retain(|intent| {
            !settled
                .iter()
                .any(|(output_id, field)| *output_id == intent.output_id && *field == intent.field)
        });
    }
}
