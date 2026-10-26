//! Contract tests for live layout reconciliation: a small model of the
//! daemon, the network, and one client's zone ledger, driven through every
//! ordering of edits, remote writes, sends, replies, refreshes, stale
//! rejections, lost requests, lost replies, and undo.
//!
//! The model has two outputs and two edited fields (x and y). The canvas
//! draws widths normalized (half again the stored width), so the client's
//! `shown` differs from what the daemon stores. The client runs the real
//! [`ZoneLedger`] and follows the writer's loop: take a write, send it, and
//! on its outcome call `applied`; or `stale` then `refetched`; or, when the
//! reply is lost, `lost` then `settle_lost` from a fresh read. The daemon
//! fences each write at its revision. Replies and refreshes reach the
//! client in any order, and a refresh may be older than one already seen.
//!
//! After everything is delivered and a final refresh, these must hold:
//!
//! 1. The client shows exactly what the daemon holds, with nothing pending.
//! 2. A write of ours changes only fields the user edited, each to the
//!    user's latest value when it was sent; never a width, which the user
//!    never edits.
//! 3. A write differs from what the ledger confirmed only on fields it
//!    holds intents for.
//! 4. A write changes a field only if the user edited it after the last of
//!    our writes to that field landed (judged from the daemon's own order,
//!    not the ledger's bookkeeping).
//! 5. The user's latest value for a field is what the daemon ends with,
//!    unless a remote write to that field landed after ours did, or the
//!    write carrying it was lost on the way and the client said so.
//!
//! And at every step: the revision the client confirms never goes back,
//! so the canvas never falls back to an older daemon state.

use std::collections::{HashMap, HashSet, VecDeque};

use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output, StripDirection};
use hypercolor_ui::components::layout_builder::ledger::{
    Field, Intent, Settled, Value, Write, ZoneLedger,
};

const OUTPUTS: [&str; 2] = ["a", "b"];
const FIELDS: [Field; 2] = [Field::X, Field::Y];
const CHECKED: [Field; 3] = [Field::X, Field::Y, Field::Width];
const VALUES: [f32; 5] = [0.1, 0.2, 0.4, 0.6, 0.8];
/// How much wider the canvas draws an output than the daemon stores it.
const DRAWN: f32 = 1.5;

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

/// The outputs as the canvas draws them.
fn shown(raw: &[Output]) -> Vec<Output> {
    raw.iter()
        .cloned()
        .map(|mut output| {
            output.size.x *= DRAWN;
            output
        })
        .collect()
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

fn key(id: &str, field: Field) -> Key {
    let id = OUTPUTS
        .iter()
        .copied()
        .find(|known| *known == id)
        .expect("a model output");
    (id, field)
}

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
    /// The reply never came: the client knows only that the outcome is
    /// unknown.
    Lost {
        id: u64,
    },
    Refresh {
        revision: u64,
        outputs: Vec<Output>,
    },
}

/// A write on its way to the daemon.
#[derive(Clone, Debug)]
struct Outgoing {
    write: Write,
    /// The user's latest value per field it holds intents for, at sending.
    intended: HashMap<Key, f32>,
    /// The fields it holds intents for.
    carried: Vec<Key>,
    /// The step it was sent at.
    sent_at: u64,
}

struct Model {
    revision: u64,
    server: Vec<Output>,
    /// Server applies so far, in the daemon's order.
    applies: u64,
    step: u64,
    ledger: ZoneLedger,
    to_server: VecDeque<Outgoing>,
    to_client: Vec<Message>,
    awaiting_refetch: bool,
    /// A lost write waiting for a fresh read: what it carried, and whether
    /// the daemon really applied it.
    awaiting_settle: Option<(Vec<Key>, bool)>,
    /// Writes the daemon has seen, by id, and whether each applied.
    reached: HashMap<u64, bool>,
    in_flight: Option<Outgoing>,
    latest_local: HashMap<Key, f32>,
    /// The step of the user's latest edit per field.
    edited_at: HashMap<Key, u64>,
    /// When our last write to a field landed: the step it was sent at.
    landed_sent_at: HashMap<Key, u64>,
    /// When the daemon last came to hold the user's latest value.
    local_landed: HashMap<Key, u64>,
    /// When a remote write last landed per field.
    remote_landed: HashMap<Key, u64>,
    /// Fields whose latest value was lost with its write, and the user told.
    excused: HashSet<Key>,
    undo: Vec<(Key, f32)>,
    seen: u64,
    log: Vec<String>,
}

impl Model {
    fn new() -> Self {
        let server = vec![output("a", 0.2, 0.2), output("b", 0.2, 0.2)];
        let mut ledger = ZoneLedger::default();
        ledger.confirm(1, server.clone(), shown(&server));
        Self {
            revision: 1,
            server,
            applies: 0,
            step: 0,
            ledger,
            to_server: VecDeque::new(),
            to_client: Vec::new(),
            awaiting_refetch: false,
            awaiting_settle: None,
            reached: HashMap::new(),
            in_flight: None,
            latest_local: HashMap::new(),
            edited_at: HashMap::new(),
            landed_sent_at: HashMap::new(),
            local_landed: HashMap::new(),
            remote_landed: HashMap::new(),
            excused: HashSet::new(),
            undo: Vec::new(),
            seen: 1,
            log: Vec::new(),
        }
    }

    fn trace(&self) -> String {
        self.log.join("\n")
    }

    fn display(&self) -> Vec<Output> {
        self.ledger.display().expect("confirmed")
    }

    fn tick(&mut self) {
        self.step += 1;
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

    fn user_sets(&mut self, id: &'static str, field: Field, value: f32) {
        let key = key(id, field);
        self.latest_local.insert(key, value);
        self.edited_at.insert(key, self.step);
        self.local_landed.remove(&key);
        self.excused.remove(&key);
        self.note_landings();
    }

    /// The user sets a field on the canvas to a value it does not show.
    fn edit(&mut self, id: &'static str, field: Field, value: f32) -> bool {
        self.tick();
        let mut edited = self.display();
        let before = read(&edited, id, field);
        if before == value {
            return false;
        }
        set(&mut edited, id, field, value);
        assert!(self.ledger.edit(&edited), "a changed field makes an intent");
        self.undo.push((key(id, field), before));
        self.user_sets(id, field, value);
        self.log.push(format!("edit {id}.{field:?} = {value}"));
        true
    }

    /// Undo the last edit: the same path, back to the value it replaced.
    fn undo(&mut self) -> bool {
        self.tick();
        let Some(((id, field), value)) = self.undo.pop() else {
            return false;
        };
        let mut edited = self.display();
        if read(&edited, id, field) == value {
            return false;
        }
        set(&mut edited, id, field, value);
        self.ledger.edit(&edited);
        self.user_sets(id, field, value);
        self.log.push(format!("undo {id}.{field:?} = {value}"));
        true
    }

    fn remote(&mut self, id: &'static str, field: Field, value: f32) {
        self.tick();
        set(&mut self.server, id, field, value);
        self.remote_landed.insert(key(id, field), self.applies + 1);
        self.server_changed();
        self.log.push(format!("remote {id}.{field:?} = {value}"));
    }

    fn send(&mut self) -> bool {
        if self.awaiting_refetch || self.awaiting_settle.is_some() || self.in_flight.is_some() {
            return false;
        }
        self.tick();
        let carried: Vec<Key> = self
            .ledger
            .pending()
            .iter()
            .map(|intent: &Intent| key(&intent.output_id, intent.field))
            .collect();
        let confirmed = self.ledger.confirmed().expect("confirmed").to_vec();
        let Some(write) = self.ledger.take_write() else {
            return false;
        };
        // Invariant 3.
        for id in OUTPUTS {
            for field in CHECKED {
                if read(&write.outputs, id, field) != read(&confirmed, id, field) {
                    assert!(
                        carried.contains(&(id, field)),
                        "write carries {id}.{field:?} with no intent\n{}",
                        self.trace()
                    );
                }
            }
        }
        let intended = carried
            .iter()
            .filter_map(|key| self.latest_local.get(key).map(|value| (*key, *value)))
            .collect();
        self.log
            .push(format!("send #{} fence {}", write.id, write.fence));
        let outgoing = Outgoing {
            write,
            intended,
            carried,
            sent_at: self.step,
        };
        self.in_flight = Some(outgoing.clone());
        self.to_server.push_back(outgoing);
        true
    }

    /// The daemon takes the write in. `lose_reply` drops its reply.
    fn deliver_write(&mut self, lose_reply: bool) -> bool {
        let Some(outgoing) = self.to_server.pop_front() else {
            return false;
        };
        self.tick();
        let id = outgoing.write.id;
        if outgoing.write.fence != self.revision {
            self.reached.insert(id, false);
            self.log.push(format!("write #{id} stale"));
            self.to_client.push(if lose_reply {
                Message::Lost { id }
            } else {
                Message::Stale { id }
            });
            return true;
        }
        for id in OUTPUTS {
            for field in CHECKED {
                let now = read(&self.server, id, field);
                let written = read(&outgoing.write.outputs, id, field);
                if now == written {
                    continue;
                }
                let key = key(id, field);
                // Invariant 2.
                assert_eq!(
                    outgoing.intended.get(&key),
                    Some(&written),
                    "write changed {id}.{field:?} to {written}, not the user's value\n{}",
                    self.trace()
                );
                // Invariant 4.
                let edited = self.edited_at.get(&key).copied().unwrap_or(0);
                let landed = self.landed_sent_at.get(&key).copied();
                assert!(
                    landed.is_none_or(|landed| edited > landed),
                    "write changed {id}.{field:?} again with no edit since our last write to it\n{}",
                    self.trace()
                );
                self.landed_sent_at.insert(key, outgoing.sent_at);
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
                output.size = written.size;
            }
        }
        self.reached.insert(id, true);
        self.server_changed();
        self.log.push(format!(
            "write #{id} landed at {}{}",
            self.revision,
            if lose_reply { ", reply lost" } else { "" }
        ));
        self.to_client.push(if lose_reply {
            Message::Lost { id }
        } else {
            Message::Applied {
                id,
                revision: self.revision,
                outputs: self.server.clone(),
            }
        });
        true
    }

    /// The request never reaches the daemon; the client hears nothing back.
    fn lose_request(&mut self) -> bool {
        let Some(outgoing) = self.to_server.pop_front() else {
            return false;
        };
        self.tick();
        let id = outgoing.write.id;
        self.log.push(format!("write #{id} lost on the way"));
        self.to_client.push(Message::Lost { id });
        true
    }

    /// The confirmed revision only moves forward.
    fn check_monotonic(&mut self) {
        let revision = self.ledger.revision().expect("confirmed");
        assert!(
            revision >= self.seen,
            "confirmed went back from {} to {revision}\n{}",
            self.seen,
            self.trace()
        );
        self.seen = revision;
    }

    fn deliver(&mut self, index: usize) -> bool {
        if index >= self.to_client.len() {
            return false;
        }
        self.tick();
        match self.to_client.remove(index) {
            Message::Applied {
                id,
                revision,
                outputs,
            } => {
                self.log.push(format!("reply #{id} applied {revision}"));
                self.in_flight = None;
                self.ledger
                    .applied(id, revision, outputs.clone(), shown(&outputs));
            }
            Message::Stale { id } => {
                self.log.push(format!("reply #{id} stale"));
                self.in_flight = None;
                self.ledger.stale(id);
                self.awaiting_refetch = true;
            }
            Message::Lost { id } => {
                self.log.push(format!("reply #{id} never came"));
                let carried = self
                    .in_flight
                    .take()
                    .map(|outgoing| outgoing.carried)
                    .unwrap_or_default();
                let applied = self.reached.get(&id).copied().unwrap_or(false);
                self.ledger.lost(id);
                self.awaiting_settle = Some((carried, applied));
            }
            Message::Refresh { revision, outputs } => {
                self.log.push(format!("refresh {revision}"));
                self.ledger
                    .confirm(revision, outputs.clone(), shown(&outputs));
            }
        }
        self.check_monotonic();
        true
    }

    fn refetch(&mut self) -> bool {
        if !self.awaiting_refetch {
            return false;
        }
        self.tick();
        self.awaiting_refetch = false;
        self.log.push(format!("refetch {}", self.revision));
        self.ledger
            .refetched(self.revision, self.server.clone(), shown(&self.server));
        self.check_monotonic();
        true
    }

    /// The writer's fresh read after a lost reply.
    fn settle_lost(&mut self) -> bool {
        let Some((carried, applied)) = self.awaiting_settle.take() else {
            return false;
        };
        self.tick();
        let settled =
            self.ledger
                .settle_lost(self.revision, self.server.clone(), shown(&self.server));
        self.log
            .push(format!("settle lost at {}: {settled:?}", self.revision));
        if let Some(Settled::Landed { unconfirmed }) = settled {
            let still_pending: Vec<Key> = self
                .ledger
                .pending()
                .iter()
                .map(|intent| key(&intent.output_id, intent.field))
                .collect();
            for key in carried {
                // A newer intent for the field still goes out; nothing was
                // given up on.
                if still_pending.contains(&key) {
                    continue;
                }
                let Some(value) = self.latest_local.get(&key) else {
                    continue;
                };
                if read(&self.server, key.0, key.1) != *value {
                    // A value the daemon does not hold was given up on: the
                    // user must have been told.
                    assert!(
                        unconfirmed,
                        "gave up on {}.{:?} without telling the user\n{}",
                        key.0,
                        key.1,
                        self.trace()
                    );
                    if !applied {
                        self.excused.insert(key);
                    }
                }
            }
        }
        self.check_monotonic();
        true
    }

    /// Deliver everything, writing what is pending, then refresh once more.
    fn settle(&mut self) {
        for _ in 0..400 {
            let moved = self.deliver(0)
                || self.refetch()
                || self.settle_lost()
                || self.deliver_write(false)
                || self.send();
            if !moved {
                break;
            }
        }
        self.ledger
            .confirm(self.revision, self.server.clone(), shown(&self.server));
        self.log.push(format!("final refresh {}", self.revision));
    }

    fn check(&self) {
        let trace = self.trace();
        assert!(!self.ledger.has_pending(), "intents left over\n{trace}");
        assert!(!self.ledger.in_flight(), "a write left out\n{trace}");
        assert!(
            !self.ledger.unsettled(),
            "a lost write left unsettled\n{trace}"
        );
        let display = self.display();
        let held = shown(&self.server);
        for id in OUTPUTS {
            for field in CHECKED {
                let drawn = read(&display, id, field);
                let stored = read(&held, id, field);
                assert_eq!(
                    drawn, stored,
                    "{id}.{field:?} shows {drawn}, daemon has {stored}\n{trace}"
                );
            }
        }
        // Invariant 5.
        for (key, value) in &self.latest_local {
            let held = read(&self.server, key.0, key.1);
            if held == *value || self.excused.contains(key) {
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
        model.deliver_write(false);
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
    model.deliver_write(false);
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
    model.deliver_write(false);
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
    model.deliver_write(false);
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
fn a_write_whose_reply_was_lost_never_undoes_a_later_remote_write() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    // The daemon applies it, but the reply never comes back.
    model.deliver_write(true);
    model.remote("a", Field::X, 0.7);
    while model.deliver(0) {}
    model.settle_lost();
    model.settle();
    model.check();
    assert_eq!(
        read(&model.server, "a", Field::X),
        0.7,
        "the later remote write stays"
    );
}

#[test]
fn a_write_lost_before_the_daemon_goes_out_again_when_nothing_moved() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.lose_request();
    while model.deliver(0) {}
    assert_eq!(
        model.ledger.pending().len(),
        1,
        "nothing landed, so it stays"
    );
    model.settle();
    model.check();
    assert_eq!(read(&model.server, "a", Field::X), 0.4);
}

#[test]
fn a_landed_write_is_never_sent_again_for_intents_it_already_carried() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.edit("a", Field::Y, 0.6);
    model.send();
    model.deliver_write(false);
    let (refresh, reply) = (0, 1);
    model.deliver(refresh);
    model.deliver(reply - 1);
    model.remote("a", Field::Y, 0.8);
    let newest = model.to_client.len() - 1;
    model.deliver(newest);
    model.settle();
    model.check();
    assert_eq!(
        read(&model.server, "a", Field::Y),
        0.8,
        "the remote y stays"
    );
}

#[test]
fn undo_makes_intents_the_same_way_and_lands() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.undo();
    model.deliver_write(false);
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
            match rng.below(11) {
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
                    model.deliver_write(false);
                }
                5 => {
                    model.deliver_write(true);
                }
                6 => {
                    model.lose_request();
                }
                7 => {
                    let index = rng.below(model.to_client.len().max(1));
                    model.deliver(index);
                }
                8 => {
                    model.refetch();
                }
                9 => {
                    model.settle_lost();
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
