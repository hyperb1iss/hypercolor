//! Contract tests for live layout reconciliation: a small model of the
//! daemon, the network, and one client's zone ledger, driven through every
//! ordering of edits, remote writes, sends, replies, refreshes, stale
//! rejections, and undo.
//!
//! The model has two outputs and two fields (x and y). The client side runs
//! the real [`ZoneLedger`] and follows the writer's loop: take a write, send
//! it, and on its outcome call `applied`, or `stale` then `refetched`. The
//! daemon fences each write at its revision. Replies and refreshes reach the
//! client in any order, and a refresh may be older than one already seen.
//!
//! After everything is delivered and a final refresh, these must hold:
//!
//! 1. The client shows exactly what the daemon holds, with nothing pending.
//! 2. No write of ours ever changes a field it carried no intent for.
//! 3. No write carries a changed field without a pending intent for it.
//! 4. The user's latest value for a field is what the daemon ends with,
//!    unless a remote write to that field landed after ours did.
//!
//! And at every step: the revision the client confirms never goes back,
//! so the canvas never falls back to an older daemon state.

use std::collections::{HashMap, VecDeque};

use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output, StripDirection};
use hypercolor_ui::components::layout_builder::ledger::{Field, Intent, Value, Write, ZoneLedger};

const OUTPUTS: [&str; 2] = ["a", "b"];
const FIELDS: [Field; 2] = [Field::X, Field::Y];
const VALUES: [f32; 5] = [0.1, 0.2, 0.4, 0.6, 0.8];

fn output(id: &str, x: f32, y: f32) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: format!("usb:{id}"),
        zone_name: Some("Main".to_owned()),
        position: NormalizedPosition::new(x, y),
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

fn number(value: Value) -> f32 {
    match value {
        Value::Number(value) => value,
        Value::Orientation(_) => f32::NAN,
    }
}

fn read(outputs: &[Output], id: &str, field: Field) -> f32 {
    outputs
        .iter()
        .find(|output| output.id == id)
        .map_or(f32::NAN, |output| number(field.read(output)))
}

fn set(outputs: &mut [Output], id: &str, field: Field, value: f32) {
    if let Some(output) = outputs.iter_mut().find(|output| output.id == id) {
        field.write(output, Value::Number(value));
    }
}

type Key = (&'static str, Field);

/// What reaches the client.
#[derive(Clone, Debug)]
enum Message {
    Applied {
        id: u64,
        revision: u64,
        outputs: Vec<Output>,
    },
    Stale {
        id: u64,
    },
    Refresh {
        revision: u64,
        outputs: Vec<Output>,
    },
}

/// A write on its way to the daemon, with the user's latest value for
/// each field it holds intents for when it was sent.
#[derive(Clone, Debug)]
struct Outgoing {
    write: Write,
    intended: HashMap<Key, f32>,
}

struct Model {
    revision: u64,
    server: Vec<Output>,
    /// Server applies so far; the index of each is its place in server order.
    applies: u64,
    ledger: ZoneLedger,
    to_server: VecDeque<Outgoing>,
    to_client: Vec<Message>,
    awaiting_refetch: bool,
    /// The user's latest value per field.
    latest_local: HashMap<Key, f32>,
    /// When the daemon last came to hold the user's latest value.
    local_landed: HashMap<Key, u64>,
    /// When a remote write last landed per field.
    remote_landed: HashMap<Key, u64>,
    undo: Vec<(Key, f32)>,
    /// The newest revision the client has confirmed.
    seen: u64,
    log: Vec<String>,
}

fn key(id: &str, field: Field) -> Key {
    let id = OUTPUTS
        .iter()
        .copied()
        .find(|known| *known == id)
        .expect("a model output");
    (id, field)
}

impl Model {
    fn new() -> Self {
        let server = vec![output("a", 0.2, 0.2), output("b", 0.2, 0.2)];
        let mut ledger = ZoneLedger::default();
        ledger.confirm(1, server.clone(), server.clone());
        Self {
            revision: 1,
            server,
            applies: 0,
            ledger,
            to_server: VecDeque::new(),
            to_client: Vec::new(),
            awaiting_refetch: false,
            latest_local: HashMap::new(),
            local_landed: HashMap::new(),
            remote_landed: HashMap::new(),
            undo: Vec::new(),
            seen: 1,
            log: Vec::new(),
        }
    }

    fn display(&self) -> Vec<Output> {
        self.ledger.display().expect("confirmed")
    }

    /// Note when the daemon holds a field at the user's latest value.
    fn note_landings(&mut self) {
        for (key, value) in &self.latest_local {
            if read(&self.server, key.0, key.1) == *value {
                self.local_landed.insert(*key, self.applies);
            }
        }
    }

    fn server_changed(&mut self) {
        self.revision += 1;
        self.applies += 1;
        self.note_landings();
        self.to_client.push(Message::Refresh {
            revision: self.revision,
            outputs: self.server.clone(),
        });
    }

    /// The user sets a field on the canvas to a value it does not show.
    fn edit(&mut self, id: &'static str, field: Field, value: f32) -> bool {
        let mut edited = self.display();
        let before = read(&edited, id, field);
        if before == value {
            return false;
        }
        set(&mut edited, id, field, value);
        assert!(self.ledger.edit(&edited), "a changed field makes an intent");
        let key = key(id, field);
        self.undo.push((key, before));
        self.latest_local.insert(key, value);
        self.local_landed.remove(&key);
        self.note_landings();
        self.log.push(format!("edit {id}.{field:?} = {value}"));
        true
    }

    /// Undo the last edit: the same path, back to the value it replaced.
    fn undo(&mut self) -> bool {
        let Some(((id, field), value)) = self.undo.pop() else {
            return false;
        };
        let mut edited = self.display();
        if read(&edited, id, field) == value {
            return false;
        }
        set(&mut edited, id, field, value);
        self.ledger.edit(&edited);
        self.latest_local.insert((id, field), value);
        self.local_landed.remove(&(id, field));
        self.note_landings();
        self.log.push(format!("undo {id}.{field:?} = {value}"));
        true
    }

    fn remote(&mut self, id: &'static str, field: Field, value: f32) {
        set(&mut self.server, id, field, value);
        self.remote_landed.insert(key(id, field), self.applies + 1);
        self.server_changed();
        self.log.push(format!("remote {id}.{field:?} = {value}"));
    }

    fn send(&mut self) -> bool {
        if self.awaiting_refetch || !self.to_server.is_empty() {
            return false;
        }
        let pending: Vec<Key> = self
            .ledger
            .pending()
            .iter()
            .map(|intent: &Intent| key(&intent.output_id, intent.field))
            .collect();
        let confirmed = self.ledger.confirmed().expect("confirmed").to_vec();
        let Some(write) = self.ledger.take_write() else {
            return false;
        };
        // Invariant 3: the write differs from what the ledger confirmed
        // only on fields it holds intents for.
        for id in OUTPUTS {
            for field in FIELDS {
                if read(&write.outputs, id, field) != read(&confirmed, id, field) {
                    assert!(
                        pending.contains(&(id, field)),
                        "write carries {id}.{field:?} with no intent\n{}",
                        self.log.join("\n")
                    );
                }
            }
        }
        let intended = pending
            .iter()
            .filter_map(|key| self.latest_local.get(key).map(|value| (*key, *value)))
            .collect();
        self.log
            .push(format!("send #{} fence {}", write.id, write.fence));
        self.to_server.push_back(Outgoing { write, intended });
        true
    }

    fn deliver_write(&mut self) -> bool {
        let Some(outgoing) = self.to_server.pop_front() else {
            return false;
        };
        let id = outgoing.write.id;
        if outgoing.write.fence != self.revision {
            self.log.push(format!("write #{id} stale"));
            self.to_client.push(Message::Stale { id });
            return true;
        }
        // Invariant 2: under the fence, a write changes only fields the user
        // edited, each to the user's latest value when it was sent, so a
        // remote change to any other field survives.
        for id in OUTPUTS {
            for field in FIELDS {
                let now = read(&self.server, id, field);
                let written = read(&outgoing.write.outputs, id, field);
                if now != written {
                    assert_eq!(
                        outgoing.intended.get(&(id, field)),
                        Some(&written),
                        "write changed {id}.{field:?} to {written}, not the user's value\n{}",
                        self.log.join("\n")
                    );
                }
            }
        }
        for output in &mut self.server {
            if let Some(written) = outgoing
                .write
                .outputs
                .iter()
                .find(|written| written.id == output.id)
            {
                output.position = written.position;
            }
        }
        self.server_changed();
        self.log
            .push(format!("write #{id} landed at {}", self.revision));
        self.to_client.push(Message::Applied {
            id,
            revision: self.revision,
            outputs: self.server.clone(),
        });
        true
    }

    /// The confirmed revision only moves forward.
    fn check_monotonic(&mut self) {
        let revision = self.ledger.revision().expect("confirmed");
        assert!(
            revision >= self.seen,
            "confirmed went back from {} to {revision}\n{}",
            self.seen,
            self.log.join("\n")
        );
        self.seen = revision;
    }

    fn deliver(&mut self, index: usize) -> bool {
        let delivered = self.deliver_message(index);
        if delivered {
            self.check_monotonic();
        }
        delivered
    }

    fn deliver_message(&mut self, index: usize) -> bool {
        if index >= self.to_client.len() {
            return false;
        }
        match self.to_client.remove(index) {
            Message::Applied {
                id,
                revision,
                outputs,
            } => {
                self.log.push(format!("reply #{id} applied {revision}"));
                self.ledger.applied(id, revision, outputs.clone(), outputs);
            }
            Message::Stale { id } => {
                self.log.push(format!("reply #{id} stale"));
                self.ledger.stale(id);
                self.awaiting_refetch = true;
            }
            Message::Refresh { revision, outputs } => {
                self.log.push(format!("refresh {revision}"));
                self.ledger.confirm(revision, outputs.clone(), outputs);
            }
        }
        true
    }

    fn refetch(&mut self) -> bool {
        if !self.awaiting_refetch {
            return false;
        }
        self.awaiting_refetch = false;
        self.log.push(format!("refetch {}", self.revision));
        self.ledger
            .refetched(self.revision, self.server.clone(), self.server.clone());
        self.check_monotonic();
        true
    }

    /// Deliver everything, writing what is pending, then refresh once more.
    fn settle(&mut self) {
        for _ in 0..200 {
            let moved = self.deliver(0) || self.refetch() || self.deliver_write() || self.send();
            if !moved {
                break;
            }
        }
        self.ledger
            .confirm(self.revision, self.server.clone(), self.server.clone());
        self.log.push(format!("final refresh {}", self.revision));
    }

    fn check(&self) {
        let trace = self.log.join("\n");
        assert!(!self.ledger.has_pending(), "intents left over\n{trace}");
        assert!(!self.ledger.in_flight(), "a write left out\n{trace}");
        let display = self.display();
        for id in OUTPUTS {
            for field in FIELDS {
                let shown = read(&display, id, field);
                let held = read(&self.server, id, field);
                assert_eq!(
                    shown, held,
                    "{id}.{field:?} shows {shown}, daemon has {held}\n{trace}"
                );
            }
        }
        for (key, value) in &self.latest_local {
            let held = read(&self.server, key.0, key.1);
            if held == *value {
                continue;
            }
            let landed = self.local_landed.get(key);
            let remote = self.remote_landed.get(key);
            assert!(
                matches!((landed, remote), (Some(landed), Some(remote)) if remote > landed),
                "{}.{:?}: the user's {value} was lost to {held} with no later remote write\n{trace}",
                key.0,
                key.1
            );
        }
    }
}

// ── Named regressions ────────────────────────────────────────────────────

#[test]
fn a_return_to_the_old_value_lands_whichever_reply_or_refresh_comes_first() {
    for refresh_first in [false, true] {
        let mut model = Model::new();
        model.edit("a", Field::X, 0.4);
        model.send();
        model.edit("a", Field::X, 0.2);
        model.deliver_write();
        // The daemon queued its refresh, then its reply.
        let (refresh, reply) = (0, 1);
        model.deliver(if refresh_first { refresh } else { reply });
        assert_eq!(
            read(&model.display(), "a", Field::X),
            0.2,
            "the canvas never snaps back"
        );
        model.settle();
        model.check();
        assert_eq!(read(&model.server, "a", Field::X), 0.2);
    }
}

#[test]
fn a_stale_write_the_daemon_already_holds_still_settles_the_return_after_it() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.edit("a", Field::X, 0.2);
    // Another client writes 0.4 first, so our write is stale.
    model.remote("a", Field::X, 0.4);
    model.deliver_write();
    let stale = model
        .to_client
        .iter()
        .position(|message| matches!(message, Message::Stale { .. }))
        .expect("stale reply");
    model.deliver(stale);
    model.refetch();
    model.settle();
    model.check();
    assert_eq!(
        read(&model.server, "a", Field::X),
        0.2,
        "the return still lands"
    );
}

#[test]
fn a_remote_value_between_our_write_and_its_reply_keeps_the_next_edit() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.deliver_write();
    model.edit("a", Field::X, 0.2);
    model.remote("a", Field::X, 0.6);
    // The remote write's refresh beats our reply.
    let remote_refresh = model.to_client.len() - 1;
    model.deliver(remote_refresh);
    assert_eq!(
        read(&model.display(), "a", Field::X),
        0.2,
        "the canvas keeps the return"
    );
    model.edit("a", Field::Y, 0.8);
    model.settle();
    model.check();
    assert_eq!(
        read(&model.server, "a", Field::X),
        0.2,
        "the return is not erased"
    );
    assert_eq!(read(&model.server, "a", Field::Y), 0.8);
}

#[test]
fn a_remote_move_of_another_output_is_never_written_back() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.edit("a", Field::Y, 0.6);
    model.remote("b", Field::X, 0.4);
    let refresh = model.to_client.len() - 1;
    model.deliver(refresh);
    model.edit("a", Field::Y, 0.8);
    model.remote("b", Field::X, 0.6);
    model.settle();
    model.check();
    assert_eq!(
        read(&model.server, "b", Field::X),
        0.6,
        "the newer remote move stays"
    );
}

#[test]
fn a_remote_write_after_ours_wins_when_its_refresh_beats_our_reply() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.deliver_write();
    model.remote("a", Field::X, 0.8);
    let newest = model.to_client.len() - 1;
    model.deliver(newest);
    model.settle();
    model.check();
    assert_eq!(
        read(&model.server, "a", Field::X),
        0.8,
        "the later remote write wins"
    );
    assert_eq!(read(&model.display(), "a", Field::X), 0.8);
}

#[test]
fn undo_makes_intents_the_same_way_and_lands() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.undo();
    model.deliver_write();
    model.settle();
    model.check();
    assert_eq!(read(&model.server, "a", Field::X), 0.2);
}

// ── Every ordering ───────────────────────────────────────────────────────

/// A small xorshift generator, so every run explores the same cases.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound).unwrap_or(1)).unwrap_or(0)
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

#[test]
fn every_interleaving_converges_and_keeps_both_clients_intents() {
    const CASES: u64 = 20_000;
    const STEPS: usize = 40;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..CASES {
        let mut model = Model::new();
        for _ in 0..STEPS {
            match rng.below(8) {
                0 | 1 => {
                    let id = rng.pick(&OUTPUTS);
                    let field = rng.pick(&FIELDS);
                    let value = rng.pick(&VALUES);
                    model.edit(id, field, value);
                }
                2 => {
                    let id = rng.pick(&OUTPUTS);
                    let field = rng.pick(&FIELDS);
                    let value = rng.pick(&VALUES);
                    model.remote(id, field, value);
                }
                3 => {
                    model.send();
                }
                4 => {
                    model.deliver_write();
                }
                5 => {
                    let index = rng.below(model.to_client.len().max(1));
                    model.deliver(index);
                }
                6 => {
                    model.refetch();
                }
                _ => {
                    model.undo();
                }
            }
        }
        model.settle();
        model.check();
    }
}
