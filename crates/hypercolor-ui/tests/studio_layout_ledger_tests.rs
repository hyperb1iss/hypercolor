//! Contract tests for live layout reconciliation: a small model of the
//! daemon, the network, and one client's zone ledgers, driven through every
//! ordering of edits, remote writes, sends, retries, replies, refreshes,
//! stale rejections, lost requests and replies, requests that commit late,
//! sockets that drop and open again, daemon restarts, and undo.
//!
//! The daemon honors each write's idempotency key: a request may stall and
//! commit late, after the client gave up waiting on it, and a retry under
//! the same key gets the stored answer (waiting for a stalled original)
//! rather than a second apply. Keys live for one daemon run. A restart
//! starts a new run whose revisions start over, so its numbers say nothing
//! about the old run's; it kills the requests still on their way and
//! forgets every key, while the replies and reads the old run already sent
//! still arrive. Every reply and read names its run, the socket's handshake
//! names the run it reaches, and a write's fence names its run as well as
//! its revision: the daemon refuses a fence from another run whatever its
//! number, as `If-Match` refuses an epoch-qualified fence of another run.
//!
//! The model has two outputs and two edited fields (x and y). The canvas
//! draws widths normalized (half again the stored width), so the client's
//! `shown` differs from what the daemon stores. The client runs the real
//! [`SceneLedgers`] for one scene of one zone and follows the writer's
//! loop: take a write and send it; on an answer from the write's own run
//! call `applied`, or `stale` then `refetched`; while no answer comes, send
//! it again under its key; and settle a write its key can no longer settle
//! (answered by another run, or sent in one that ended) with
//! `settle_ended` from a fresh read. Replies and refreshes reach the client
//! in any order, and a refresh may be older than one already seen.
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
//!    not the ledger's bookkeeping). A lost request that commits late and
//!    is then sent again would break this; under its key it cannot.
//! 5. The user's latest value for a field is what the daemon ends with,
//!    unless a remote write to that field landed after ours did, or the
//!    write carrying it died with a daemon run and the client said so.
//!
//! And at every step: within one daemon run the revision the client
//! confirms never goes back, across sockets that open again too; the
//! daemon applies a key at most once; and no write is ever applied under a
//! fence from an earlier run.

use std::collections::{HashMap, HashSet, VecDeque};

use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output, StripDirection};
use hypercolor_ui::components::layout_builder::ledger::{
    Admission, Cancelled, Cause, Field, Intent, SceneLedgers, SceneRead, Value, Write, ZoneLedger,
    ZoneRead,
};

const ZONE: &str = "main";
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

/// The name daemon run `run` goes by.
fn run_name(run: u64) -> String {
    format!("run-{run}")
}

/// A read of the model's one zone, from daemon run `run`.
fn zone_read(scene_id: SceneId, revision: u64, outputs: &[Output], run: u64) -> SceneRead {
    SceneRead {
        scene_id,
        revision,
        run: run_name(run),
        zones: vec![ZoneRead {
            zone_id: ZONE.to_owned(),
            raw: outputs.to_vec(),
            shown: shown(outputs),
        }],
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
        run: u64,
    },
    Stale {
        id: u64,
        run: u64,
    },
    /// No answer came back: the client knows only that the outcome is
    /// unknown.
    Lost {
        id: u64,
    },
    /// A read of the scene, from daemon run `run`.
    Refresh {
        revision: u64,
        outputs: Vec<Output>,
        run: u64,
    },
}

/// A write on its way to the daemon. A retry sends the same one again.
#[derive(Clone, Debug)]
struct Outgoing {
    write: Write,
    /// The user's latest value per field it holds intents for, at sending.
    intended: HashMap<Key, f32>,
    /// The fields it holds intents for.
    carried: Vec<Key>,
    /// The step it was first sent at.
    sent_at: u64,
}

struct Model {
    /// The daemon's run.
    run: u64,
    /// Answers the daemon stored by write key, within one run.
    answers: HashMap<String, Message>,
    /// Requests the daemon holds but has not committed yet.
    stalled: Vec<Outgoing>,
    /// Keys the daemon applied, by run, to catch a second apply.
    applied_keys: HashSet<(u64, String)>,
    /// A write no answer came back for, to send again under its key.
    awaiting_retry: Option<Outgoing>,
    revision: u64,
    server: Vec<Output>,
    /// Server applies so far, in the daemon's order.
    applies: u64,
    step: u64,
    scene: SceneId,
    ledgers: SceneLedgers,
    /// The daemon run the client's socket last named.
    socket_run: u64,
    to_server: VecDeque<Outgoing>,
    to_client: Vec<Message>,
    awaiting_refetch: bool,
    /// A write its key can no longer settle, waiting for a fresh read:
    /// what it carried, and whether the daemon really applied it.
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
    /// Fields whose latest value died with a daemon run, the user told.
    excused: HashSet<Key>,
    undo: Vec<(Key, f32)>,
    seen: u64,
    log: Vec<String>,
}

impl Model {
    fn new() -> Self {
        let server = vec![output("a", 0.2, 0.2), output("b", 0.2, 0.2)];
        let scene = SceneId::new();
        let mut ledgers = SceneLedgers::new("page");
        ledgers.enter(&run_name(1));
        ledgers
            .observe(&zone_read(scene, 1, &server, 1), Some(ZONE))
            .expect("the first read");
        Self {
            run: 1,
            answers: HashMap::new(),
            stalled: Vec::new(),
            applied_keys: HashSet::new(),
            awaiting_retry: None,
            revision: 1,
            server,
            applies: 0,
            step: 0,
            scene,
            ledgers,
            socket_run: 1,
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

    fn ledger(&self) -> &ZoneLedger {
        self.ledgers.zone(ZONE).expect("the zone's ledger")
    }

    fn ledger_mut(&mut self) -> &mut ZoneLedger {
        self.ledgers.ledger_mut(ZONE).expect("the zone's ledger")
    }

    /// The scene as the daemon holds it now.
    fn read_now(&self) -> SceneRead {
        zone_read(self.scene, self.revision, &self.server, self.run)
    }

    /// The writer's own read, under the ledgers' ticket of the moment.
    /// Whether the ledgers took it: a read of a run the socket has not
    /// named yet waits for the socket.
    fn writer_reads(&mut self) -> bool {
        let read = self.read_now();
        let ticket = self.ledgers.ticket().expect("a scene");
        match self.ledgers.admit(ticket, &read) {
            Admission::Taken(_) => true,
            Admission::OtherRun => false,
            other => panic!("one scene's own read came to {other:?}\n{}", self.trace()),
        }
    }

    fn trace(&self) -> String {
        self.log.join("\n")
    }

    fn display(&self) -> Vec<Output> {
        self.ledger().display().expect("confirmed")
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
            run: self.run,
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
        assert!(
            self.ledger_mut().edit(&edited),
            "a changed field makes an intent"
        );
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
        self.ledger_mut().edit(&edited);
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
        if self.awaiting_refetch
            || self.awaiting_settle.is_some()
            || self.awaiting_retry.is_some()
            || self.in_flight.is_some()
        {
            return false;
        }
        self.tick();
        let carried: Vec<Key> = self
            .ledger()
            .pending()
            .iter()
            .map(|intent: &Intent| key(&intent.output_id, intent.field))
            .collect();
        let confirmed = self.ledger().confirmed().expect("confirmed").to_vec();
        let Some(write) = self.ledgers.take_write(ZONE) else {
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
        self.log.push(format!(
            "send #{} fence {} in {}",
            write.id, write.fence, write.run
        ));
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

    /// The daemon commits a write or finds it stale, and keeps the answer
    /// under its key.
    fn process(&mut self, outgoing: &Outgoing) -> Message {
        let id = outgoing.write.id;
        if let Some(answer) = self.answers.get(&outgoing.write.key).cloned() {
            self.log.push(format!("write #{id} replayed"));
            return answer;
        }
        // The fence holds only in its own run, whatever its number.
        if outgoing.write.run != run_name(self.run) || outgoing.write.fence != self.revision {
            self.reached.entry(id).or_insert(false);
            self.log.push(format!("write #{id} stale"));
            let answer = Message::Stale { id, run: self.run };
            self.answers
                .insert(outgoing.write.key.clone(), answer.clone());
            return answer;
        }
        assert!(
            self.applied_keys
                .insert((self.run, outgoing.write.key.clone())),
            "write #{id} applied twice under one key\n{}",
            self.trace()
        );
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
        self.log
            .push(format!("write #{id} landed at {}", self.revision));
        let answer = Message::Applied {
            id,
            revision: self.revision,
            outputs: self.server.clone(),
            run: self.run,
        };
        self.answers
            .insert(outgoing.write.key.clone(), answer.clone());
        answer
    }

    /// The daemon takes the write in. `lose_reply` drops its reply. A key
    /// the daemon has seen gets the answer it stored, the original first
    /// when that is still stalled, rather than a second apply.
    fn deliver_write(&mut self, lose_reply: bool) -> bool {
        let Some(outgoing) = self.to_server.pop_front() else {
            return false;
        };
        self.tick();
        let id = outgoing.write.id;
        let key = outgoing.write.key.clone();
        let answer = if let Some(index) = self.stalled.iter().position(|held| held.write.key == key)
        {
            let original = self.stalled.remove(index);
            self.log.push(format!("write #{id} waits for the original"));
            self.process(&original)
        } else {
            self.process(&outgoing)
        };
        self.to_client.push(if lose_reply {
            self.log.push(format!("reply to #{id} lost"));
            Message::Lost { id }
        } else {
            answer
        });
        true
    }

    /// The request reaches the daemon and stalls there; the client hears
    /// nothing back, and the daemon may commit it later.
    fn stall_request(&mut self) -> bool {
        let Some(outgoing) = self.to_server.pop_front() else {
            return false;
        };
        self.tick();
        let id = outgoing.write.id;
        self.log.push(format!("write #{id} stalled"));
        self.stalled.push(outgoing);
        self.to_client.push(Message::Lost { id });
        true
    }

    /// A stalled request commits, long after the client gave up on it.
    fn commit_stalled(&mut self) -> bool {
        if self.stalled.is_empty() {
            return false;
        }
        self.tick();
        let original = self.stalled.remove(0);
        self.log
            .push(format!("stalled write #{} goes ahead", original.write.id));
        let _ = self.process(&original);
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

    /// The client sends a write no answer came back for again, under its
    /// own key.
    fn retry(&mut self) -> bool {
        let Some(outgoing) = self.awaiting_retry.take() else {
            return false;
        };
        self.tick();
        let write = self.ledger().retry().expect("a write to retry");
        assert_eq!(write, outgoing.write, "a retry is the same write");
        self.log.push(format!("retry #{} under its key", write.id));
        self.in_flight = Some(outgoing.clone());
        self.to_server.push_back(outgoing);
        true
    }

    /// The daemon starts a new run. Requests still on their way or held
    /// die with it, it forgets every key, and its revisions start well
    /// past the old run's. Replies and reads it already sent still arrive.
    /// The client learns of it once its socket opens again.
    fn restart(&mut self) {
        self.tick();
        self.run += 1;
        self.revision = 1;
        for outgoing in self.to_server.drain(..) {
            self.to_client.push(Message::Lost {
                id: outgoing.write.id,
            });
        }
        self.stalled.clear();
        self.answers.clear();
        self.log
            .push(format!("daemon restarts as {}", run_name(self.run)));
    }

    /// The socket opens again and its handshake names the daemon's run:
    /// a new run re-anchors the ledgers. Either way, the shared scene read
    /// the opening starts is on its way.
    fn hello(&mut self) -> bool {
        self.tick();
        if self.socket_run != self.run {
            self.socket_run = self.run;
            assert!(self.ledgers.enter(&run_name(self.run)));
            self.seen = 0;
            if let Some(outgoing) = self.awaiting_retry.take() {
                // Its key means nothing to the new run: a read settles it.
                let applied = self
                    .reached
                    .get(&outgoing.write.id)
                    .copied()
                    .unwrap_or(false);
                self.awaiting_settle = Some((outgoing.carried, applied));
            }
            self.log
                .push(format!("socket names {}", run_name(self.run)));
        } else {
            assert!(!self.ledgers.enter(&run_name(self.run)));
            self.log
                .push("socket opens again on the same run".to_owned());
        }
        self.to_client.push(Message::Refresh {
            revision: self.revision,
            outputs: self.server.clone(),
            run: self.run,
        });
        true
    }

    /// The writer's own read for a ledger waiting on a re-anchor.
    fn anchor(&mut self) -> bool {
        if !self.ledger().reanchoring() {
            return false;
        }
        self.tick();
        if !self.writer_reads() {
            return false;
        }
        self.log.push(format!("anchor read {}", self.revision));
        self.check_monotonic();
        true
    }

    /// The confirmed revision only moves forward within a daemon run.
    fn check_monotonic(&mut self) {
        if self.ledger().reanchoring() {
            return;
        }
        let revision = self.ledger().revision().expect("confirmed");
        assert!(
            revision >= self.seen,
            "confirmed went back from {} to {revision}\n{}",
            self.seen,
            self.trace()
        );
        self.seen = revision;
    }

    /// A write another run answered waits for a fresh read.
    fn strand(&mut self, outgoing: Option<Outgoing>, id: u64) {
        self.ledger_mut().stranded(id);
        if !self.ledger().unsettled() {
            return;
        }
        let applied = self.reached.get(&id).copied().unwrap_or(false);
        let carried = outgoing.map(|outgoing| outgoing.carried);
        self.awaiting_settle = Some((carried.unwrap_or_default(), applied));
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
                run,
            } => {
                self.log.push(format!(
                    "reply #{id} applied {revision} in {}",
                    run_name(run)
                ));
                let outgoing = self.in_flight.take();
                let own = outgoing
                    .as_ref()
                    .is_some_and(|outgoing| outgoing.write.run == run_name(run));
                if own {
                    let taken = self.ledger_mut().applied(
                        id,
                        Some(revision),
                        outputs.clone(),
                        shown(&outputs),
                    );
                    if taken.is_none() && self.ledger().unsettled() {
                        // Sent in a run that has since ended: a read settles it.
                        self.log.push(format!("#{id} left for a read to settle"));
                        let carried = outgoing.map(|outgoing| outgoing.carried);
                        self.awaiting_settle = Some((carried.unwrap_or_default(), true));
                    }
                } else {
                    self.strand(outgoing, id);
                }
            }
            Message::Stale { id, run } => {
                self.log
                    .push(format!("reply #{id} stale in {}", run_name(run)));
                let outgoing = self.in_flight.take();
                let own = outgoing
                    .as_ref()
                    .is_some_and(|outgoing| outgoing.write.run == run_name(run));
                if own {
                    self.ledger_mut().stale(id);
                    self.awaiting_refetch = true;
                } else {
                    self.strand(outgoing, id);
                }
            }
            Message::Lost { id } => {
                self.log.push(format!("no answer for #{id}"));
                let outgoing = self.in_flight.take();
                self.ledger_mut().lost(id);
                if self.ledger().retry().is_some() {
                    self.awaiting_retry = outgoing;
                } else {
                    let applied = self.reached.get(&id).copied().unwrap_or(false);
                    let carried = outgoing.map(|outgoing| outgoing.carried);
                    self.awaiting_settle = Some((carried.unwrap_or_default(), applied));
                }
            }
            Message::Refresh {
                revision,
                outputs,
                run,
            } => {
                let read = zone_read(self.scene, revision, &outputs, run);
                let taken = self.ledgers.observe(&read, Some(ZONE)).is_some();
                self.log.push(format!(
                    "refresh {revision} in {}{}",
                    run_name(run),
                    if taken { "" } else { ", refused" }
                ));
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
        if !self.writer_reads() {
            return false;
        }
        self.awaiting_refetch = false;
        self.log.push(format!("refetch {}", self.revision));
        let (revision, server) = (self.revision, self.server.clone());
        self.ledger_mut()
            .refetched(revision, server.clone(), shown(&server));
        self.check_monotonic();
        true
    }

    /// The writer's fresh read for a write its key can no longer settle.
    fn settle_ended(&mut self) -> bool {
        if self.awaiting_settle.is_none() {
            return false;
        }
        self.tick();
        if !self.writer_reads() {
            return false;
        }
        let Some((carried, applied)) = self.awaiting_settle.take() else {
            return false;
        };
        let (revision, server) = (self.revision, self.server.clone());
        let unconfirmed = self
            .ledger_mut()
            .settle_ended(revision, server.clone(), shown(&server))
            .expect("a write left open");
        self.log
            .push(format!("settled at {revision}: unconfirmed {unconfirmed}"));
        let still_pending: Vec<Key> = self
            .ledger()
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
        self.check_monotonic();
        true
    }

    /// Deliver everything, writing what is pending, then refresh once more.
    fn settle(&mut self) {
        for _ in 0..400 {
            let moved = (self.socket_run != self.run && self.hello())
                || self.deliver(0)
                || self.refetch()
                || self.settle_ended()
                || self.anchor()
                || self.retry()
                || self.deliver_write(false)
                || self.commit_stalled()
                || self.send();
            if !moved {
                break;
            }
        }
        let read = self.read_now();
        self.ledgers
            .observe(&read, Some(ZONE))
            .expect("a read of the daemon's run");
        self.log.push(format!("final refresh {}", self.revision));
    }

    fn check(&self) {
        let trace = self.trace();
        assert!(!self.ledger().has_pending(), "intents left over\n{trace}");
        assert!(
            self.stalled.is_empty(),
            "a stalled request left over\n{trace}"
        );
        assert!(!self.ledger().in_flight(), "a write left out\n{trace}");
        assert!(
            !self.ledger().unsettled(),
            "a write left without an answer\n{trace}"
        );
        assert!(
            !self.ledger().reanchoring(),
            "a ledger never re-anchored\n{trace}"
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
    // Sent again under its key, it gets the first answer back.
    model.settle();
    model.check();
    assert_eq!(
        read(&model.server, "a", Field::X),
        0.7,
        "the later remote write stays"
    );
}

#[test]
fn a_write_lost_before_the_daemon_goes_out_again_under_its_key() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.lose_request();
    while model.deliver(0) {}
    assert_eq!(
        model.ledger().pending().len(),
        1,
        "nothing landed, so it stays"
    );
    assert!(model.ledgers.wants_retry());
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

fn explore(cases: u64, seed: u64) {
    const STEPS: usize = 40;
    let mut rng = Rng(seed);
    for _ in 0..cases {
        let mut model = Model::new();
        for _ in 0..STEPS {
            match rng.below(16) {
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
                    if rng.below(2) == 0 {
                        model.stall_request();
                    } else {
                        model.lose_request();
                    }
                }
                7 | 8 => {
                    let index = rng.below(model.to_client.len().max(1));
                    model.deliver(index);
                }
                9 => {
                    model.refetch();
                }
                10 => {
                    model.settle_ended();
                }
                11 => {
                    model.retry();
                }
                12 => {
                    model.commit_stalled();
                }
                13 => match rng.below(12) {
                    0 => model.restart(),
                    1..=3 => {
                        model.hello();
                    }
                    _ => {}
                },
                14 => {
                    model.anchor();
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

#[test]
fn every_interleaving_converges_and_keeps_both_clients_intents() {
    explore(20_000, 0x9E37_79B9_7F4A_7C15);
}

#[test]
fn every_interleaving_with_late_commits_and_restarts_converges() {
    explore(20_000, 0xD1B5_4A32_D192_ED03);
}

// ── Keys, late commits, and new daemon runs ──────────────────────────────

#[test]
fn a_late_committing_lost_request_sent_again_under_its_key_applies_once() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    // The request stalls in the daemon; the client hears nothing back.
    model.stall_request();
    while model.deliver(0) {}
    // It commits late, and another client writes after it.
    model.commit_stalled();
    model.remote("a", Field::X, 0.6);
    // The retry under the same key gets the stored answer, not a second
    // apply, and no resend follows to undo the later remote write.
    model.retry();
    model.deliver_write(false);
    model.settle();
    model.check();
    assert_eq!(model.applied_keys.len(), 1, "applied once");
    assert_eq!(
        read(&model.server, "a", Field::X),
        0.6,
        "the later remote write stays"
    );
}

#[test]
fn a_retry_of_a_stalled_request_waits_for_it_rather_than_writing_twice() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.stall_request();
    while model.deliver(0) {}
    model.retry();
    model.deliver_write(false);
    assert!(model.stalled.is_empty(), "the original answered the retry");
    model.settle();
    model.check();
    assert_eq!(model.applied_keys.len(), 1);
    assert_eq!(read(&model.server, "a", Field::X), 0.4);
}

#[test]
fn a_socket_that_opens_again_on_the_same_run_keeps_the_key_good() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.stall_request();
    while model.deliver(0) {}
    model.hello();
    assert!(
        !model.ledger().reanchoring(),
        "the same run needs no re-anchor"
    );
    assert!(model.ledgers.wants_retry(), "the key is still good");
    model.retry();
    model.deliver_write(false);
    model.settle();
    model.check();
    assert_eq!(model.applied_keys.len(), 1);
    assert_eq!(read(&model.server, "a", Field::X), 0.4);
}

#[test]
fn a_new_daemon_run_reanchors_and_never_writes_under_an_old_fence() {
    let mut model = Model::new();
    for value in [0.4, 0.6, 0.8, 0.1] {
        model.edit("a", Field::X, value);
        model.settle();
    }
    let before = model.ledger().revision().expect("confirmed");
    model.edit("a", Field::Y, 0.6);
    // Another client's write is read just before the daemon goes away; the
    // read reaches the client only once the socket is open again.
    model.remote("b", Field::X, 0.6);
    model.restart();
    model.hello();
    model.deliver(0);
    assert!(
        model.ledger().reanchoring(),
        "a read of the old run does not re-anchor"
    );
    assert_eq!(model.ledger().revision(), Some(before));
    assert!(
        model.ledgers.take_write(ZONE).is_none(),
        "no old fence goes out"
    );
    // The new run's first read is taken, the edit kept.
    model.deliver(0);
    assert_eq!(model.ledger().revision(), Some(model.revision));
    assert_eq!(
        read(&model.display(), "a", Field::Y),
        0.6,
        "the edit is kept"
    );
    model.settle();
    model.check();
    assert_eq!(read(&model.server, "a", Field::Y), 0.6);
}

#[test]
fn a_reply_from_a_run_that_ended_settles_from_a_read() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.deliver_write(false);
    model.restart();
    model.hello();
    // The reply the old run sent comes in now, with the reads from either
    // side of the restart. It settles nothing by itself, and the zone does
    // not wedge on it.
    while model.deliver(0) {}
    assert!(model.ledger().unsettled(), "a read settles it");
    assert!(model.ledgers.wants_read());
    assert!(
        !model.ledgers.wants_retry(),
        "its key means nothing to the new run"
    );
    model.edit("a", Field::Y, 0.6);
    model.settle();
    model.check();
    assert_eq!(read(&model.server, "a", Field::X), 0.4);
    assert_eq!(read(&model.server, "a", Field::Y), 0.6);
}

#[test]
fn a_reply_after_the_socket_opened_again_on_the_same_run_settles_the_write() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.deliver_write(false);
    model.hello();
    while model.deliver(0) {}
    assert!(
        !model.ledger().has_pending(),
        "its own run's answer is final"
    );
    assert!(!model.ledgers.wants_read());
    model.settle();
    model.check();
}

#[test]
fn a_retry_a_new_run_answers_settles_from_a_read() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.stall_request();
    while model.deliver(0) {}
    // The daemon restarts with the request still held; the retry reaches
    // the new run before the socket names it.
    model.restart();
    model.retry();
    model.deliver_write(false);
    while model.deliver(0) {}
    assert!(model.ledger().unsettled());
    assert!(model.ledgers.wants_read(), "its key settles nothing now");
    model.settle();
    model.check();
}

#[test]
fn a_write_left_open_by_a_new_daemon_run_is_settled_from_a_read() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    // The daemon goes away with the request in hand.
    model.restart();
    model.settle();
    model.check();
}

// ── The active scene's ledgers ───────────────────────────────────────────

fn scene_read(scene_id: SceneId, revision: u64, zones: &[(&str, f32)]) -> SceneRead {
    scene_read_in("run-1", scene_id, revision, zones)
}

/// A read from daemon run `run`.
fn scene_read_in(run: &str, scene_id: SceneId, revision: u64, zones: &[(&str, f32)]) -> SceneRead {
    SceneRead {
        scene_id,
        revision,
        run: run.to_owned(),
        zones: zones
            .iter()
            .map(|(zone_id, x)| {
                let outputs = vec![output("a", *x, 0.2)];
                ZoneRead {
                    zone_id: (*zone_id).to_owned(),
                    raw: outputs.clone(),
                    shown: shown(&outputs),
                }
            })
            .collect(),
    }
}

/// Ledgers in daemon run `run-1`.
fn ledgers() -> SceneLedgers {
    let mut ledgers = SceneLedgers::new("page");
    ledgers.enter("run-1");
    ledgers
}

fn moved(ledgers: &mut SceneLedgers, zone_id: &str, x: f32) {
    let mut edited = ledgers
        .zone(zone_id)
        .and_then(ZoneLedger::display)
        .expect("a confirmed zone");
    set(&mut edited, "a", Field::X, x);
    assert!(ledgers.zone_mut(zone_id).edit(&edited));
}

#[test]
fn a_delayed_read_for_another_scene_never_touches_the_active_ones() {
    let (desk, stage) = (SceneId::new(), SceneId::new());
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 3, &[("main", 0.2)]), Some("main"));
    let ticket = ledgers.ticket().expect("a scene");
    // The user moves to another scene and edits it while a read of the
    // first is still out.
    ledgers.observe(&scene_read(stage, 7, &[("keys", 0.2)]), Some("keys"));
    moved(&mut ledgers, "keys", 0.6);
    assert_eq!(
        ledgers.admit(ticket, &scene_read(desk, 4, &[("main", 0.2)])),
        Admission::Refused
    );
    assert_eq!(ledgers.scene_id(), Some(stage));
    assert!(ledgers.zone("keys").is_some_and(ZoneLedger::has_pending));
}

#[test]
fn switching_back_to_a_scene_does_not_admit_a_read_started_before_the_switch() {
    let (desk, stage) = (SceneId::new(), SceneId::new());
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 3, &[("main", 0.2)]), Some("main"));
    let ticket = ledgers.ticket().expect("a scene");
    ledgers.observe(&scene_read(stage, 7, &[("keys", 0.2)]), None);
    ledgers.observe(&scene_read(desk, 5, &[("main", 0.2)]), Some("main"));
    assert_eq!(
        ledgers.admit(ticket, &scene_read(desk, 4, &[("main", 0.4)])),
        Admission::Refused
    );
}

#[test]
fn an_older_read_never_drops_a_zone_made_since() {
    let desk = SceneId::new();
    let mut ledgers = ledgers();
    ledgers.observe(
        &scene_read(desk, 9, &[("main", 0.2), ("fresh", 0.2)]),
        Some("fresh"),
    );
    moved(&mut ledgers, "fresh", 0.4);
    let ticket = ledgers.ticket().expect("a scene");
    // A read from before the zone existed comes back late.
    assert_eq!(
        ledgers.admit(ticket, &scene_read(desk, 8, &[("main", 0.2)])),
        Admission::Taken(Vec::new())
    );
    assert!(ledgers.zone("fresh").is_some_and(ZoneLedger::has_pending));
    // A newer read without it means the zone is gone, and the edit with it.
    assert_eq!(
        ledgers.admit(ticket, &scene_read(desk, 10, &[("main", 0.2)])),
        Admission::Taken(vec![Cancelled {
            zone_id: "fresh".to_owned(),
            output_ids: vec!["a".to_owned()],
            cause: Cause::ZoneGone,
        }])
    );
    assert!(ledgers.zone("fresh").is_none());
}

#[test]
fn an_output_moved_away_elsewhere_reports_the_edit_it_cancels() {
    let desk = SceneId::new();
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 3, &[("main", 0.2)]), Some("main"));
    moved(&mut ledgers, "main", 0.4);
    let mut read = scene_read(desk, 4, &[("main", 0.2)]);
    read.zones[0].raw.clear();
    read.zones[0].shown.clear();
    let cancelled = ledgers.observe(&read, Some("main"));
    assert_eq!(
        cancelled,
        Some(vec![Cancelled {
            zone_id: "main".to_owned(),
            output_ids: vec!["a".to_owned()],
            cause: Cause::OutputLeft,
        }])
    );
}

#[test]
fn a_reanchor_refuses_reads_started_before_it_and_takes_the_next_one() {
    let desk = SceneId::new();
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 500, &[("main", 0.2)]), Some("main"));
    moved(&mut ledgers, "main", 0.4);
    let before = ledgers.ticket().expect("a scene");
    assert!(ledgers.enter("run-2"));
    assert!(!ledgers.enter("run-2"), "the same run is entered once");
    assert!(
        ledgers.take_write("main").is_none(),
        "no old fence goes out"
    );
    assert!(
        ledgers.wants_read(),
        "the writer reads for the waiting edit"
    );
    assert_eq!(
        ledgers.admit(before, &scene_read_in("run-2", desk, 501, &[("main", 0.2)])),
        Admission::Refused
    );
    // The new run's first read, far below confirmed, re-anchors.
    ledgers.observe(
        &scene_read_in("run-2", desk, 6, &[("main", 0.2)]),
        Some("main"),
    );
    let ledger = ledgers.zone("main").expect("kept");
    assert_eq!(ledger.revision(), Some(6));
    assert!(ledger.has_pending(), "the edit is kept");
    let write = ledgers.take_write("main").expect("the edit goes out");
    assert_eq!(write.fence, 6);
    assert_eq!(write.run, "run-2");
    assert!(
        write.key.starts_with("page:"),
        "keys are unique to the page"
    );
}

#[test]
fn a_document_of_the_run_that_ended_never_reanchors_a_ledger() {
    let desk = SceneId::new();
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 50, &[("main", 0.2)]), Some("main"));
    moved(&mut ledgers, "main", 0.4);
    ledgers.enter("run-2");
    // The shared document still holds what the old run reported (here with
    // another client's y), and Studio offers it again while its refetch is
    // out.
    let mut stale = scene_read(desk, 50, &[("main", 0.2)]);
    set(&mut stale.zones[0].raw, "a", Field::Y, 0.9);
    set(&mut stale.zones[0].shown, "a", Field::Y, 0.9);
    assert_eq!(ledgers.observe(&stale, Some("main")), None);
    let ledger = ledgers.zone("main").expect("kept");
    assert!(ledger.reanchoring(), "still waiting on the new run");
    assert_eq!(read(&ledger.display().expect("drawn"), "a", Field::X), 0.4);
    // The new run's read re-anchors, and the old document offered again
    // afterwards, at a higher revision, is still refused.
    ledgers.observe(
        &scene_read_in("run-2", desk, 1, &[("main", 0.2)]),
        Some("main"),
    );
    assert_eq!(ledgers.observe(&stale, Some("main")), None);
    let ledger = ledgers.zone("main").expect("kept");
    assert_eq!(ledger.revision(), Some(1));
    assert_eq!(read(&ledger.display().expect("drawn"), "a", Field::X), 0.4);
    let write = ledgers.take_write("main").expect("the edit goes out");
    assert_eq!(write.fence, 1, "fenced in the new run");
    assert_eq!(
        read(&write.outputs, "a", Field::Y),
        0.2,
        "nothing of the old document goes out with it"
    );
}

#[test]
fn a_read_of_a_run_the_socket_has_not_named_waits_for_it() {
    let desk = SceneId::new();
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 50, &[("main", 0.2)]), Some("main"));
    moved(&mut ledgers, "main", 0.4);
    let ticket = ledgers.ticket().expect("a scene");
    // The daemon restarted, and an HTTP read beat the socket to it.
    let newer = scene_read_in("run-2", desk, 7, &[("main", 0.2)]);
    assert_eq!(ledgers.observe(&newer, Some("main")), None);
    assert_eq!(ledgers.admit(ticket, &newer), Admission::OtherRun);
    assert_eq!(ledgers.run(), "run-1");
    // Once the socket names the run, the same read is taken.
    ledgers.enter("run-2");
    assert!(ledgers.observe(&newer, Some("main")).is_some());
    assert_eq!(ledgers.zone("main").and_then(ZoneLedger::revision), Some(7));
}

#[test]
fn until_a_run_is_named_the_first_read_names_it() {
    let desk = SceneId::new();
    let mut ledgers = SceneLedgers::new("page");
    assert!(
        ledgers
            .observe(
                &scene_read_in("run-7", desk, 3, &[("main", 0.2)]),
                Some("main")
            )
            .is_some()
    );
    assert_eq!(ledgers.run(), "run-7");
    assert_eq!(
        ledgers.observe(
            &scene_read_in("run-8", desk, 4, &[("main", 0.2)]),
            Some("main")
        ),
        None,
        "only the socket moves it on"
    );
}

#[test]
fn a_writer_read_that_shows_another_scene_switches_the_ledgers_to_it() {
    let (desk, snapshot) = (SceneId::new(), SceneId::new());
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 3, &[("main", 0.2)]), Some("main"));
    moved(&mut ledgers, "main", 0.4);
    let ticket = ledgers.ticket().expect("a scene");
    // Another scene became active (a snapshot keeps the same zone ids)
    // with no Studio open to observe it: the writer's own read is the
    // first to show it, and nothing would ever select it otherwise.
    assert_eq!(
        ledgers.admit(ticket, &scene_read(snapshot, 4, &[("main", 0.2)])),
        Admission::Switched(vec![Cancelled {
            zone_id: "main".to_owned(),
            output_ids: vec!["a".to_owned()],
            cause: Cause::SceneChanged,
        }])
    );
    assert_eq!(ledgers.scene_id(), Some(snapshot));
    assert!(
        !ledgers.wants_read() && !ledgers.write_due(),
        "nothing is left to read or send for"
    );
    assert!(
        ledgers.take_write("main").is_none(),
        "the old scene's edit never goes to the new one"
    );
}

#[test]
fn an_older_read_of_another_scene_does_not_switch_the_ledgers() {
    let (desk, stage) = (SceneId::new(), SceneId::new());
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 9, &[("main", 0.2)]), Some("main"));
    moved(&mut ledgers, "main", 0.4);
    let ticket = ledgers.ticket().expect("a scene");
    // The daemon went to another scene and back while the read was out;
    // the read was served in between, below what the ledgers hold.
    assert_eq!(
        ledgers.admit(ticket, &scene_read(stage, 7, &[("keys", 0.2)])),
        Admission::Behind
    );
    assert_eq!(ledgers.scene_id(), Some(desk));
    assert!(ledgers.zone("main").is_some_and(ZoneLedger::has_pending));
    assert!(ledgers.admits(ticket), "the next read is still taken");
}

#[test]
fn write_ids_and_keys_never_repeat_on_one_page() {
    let (desk, stage) = (SceneId::new(), SceneId::new());
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 3, &[("main", 0.2)]), Some("main"));
    moved(&mut ledgers, "main", 0.4);
    let first = ledgers.take_write("main").expect("a write");
    // Away to another scene and back: the zone's ledger starts over.
    ledgers.observe(&scene_read(stage, 7, &[("keys", 0.2)]), None);
    ledgers.observe(&scene_read(desk, 4, &[("main", 0.2)]), Some("main"));
    moved(&mut ledgers, "main", 0.6);
    let second = ledgers.take_write("main").expect("a write");
    assert_ne!(first.id, second.id);
    assert_ne!(first.key, second.key);
    // A late reply to the first write settles nothing in the new ledger.
    let outputs = vec![output("a", 0.4, 0.2)];
    assert_eq!(
        ledgers.ledger_mut("main").expect("a ledger").applied(
            first.id,
            Some(5),
            outputs.clone(),
            shown(&outputs)
        ),
        None
    );
    assert!(ledgers.zone("main").is_some_and(ZoneLedger::in_flight));
}

#[test]
fn switching_scenes_reports_the_edits_it_leaves_behind() {
    let (desk, stage) = (SceneId::new(), SceneId::new());
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 3, &[("main", 0.2)]), Some("main"));
    moved(&mut ledgers, "main", 0.4);
    let cancelled = ledgers.select(stage);
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0].zone_id, "main");
    assert_eq!(cancelled[0].cause, Cause::SceneChanged);
}

#[test]
fn a_membership_move_of_ours_drops_the_leftover_placement_edit() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    model.edit("a", Field::Y, 0.6);
    // A membership write of ours places "a" itself. Studio's history sends
    // it only once the writer has nothing out, unless the writer is paused;
    // the edit that waited behind it must not go out after it.
    model.ledger_mut().forget(&["a".to_owned()]);
    assert!(model.ledger().pending().is_empty());
    assert!(model.ledger().in_flight(), "the write out still settles");
    model.deliver_write(false);
    while model.deliver(0) {}
    assert!(!model.ledger().has_pending());
    assert!(model.ledgers.take_write(ZONE).is_none());
}

#[test]
fn a_reply_ahead_of_its_own_commit_is_taken_whole() {
    // A follow-mode zone's reply carries the revision of the publication
    // after the write, which another writer may have moved past it.
    let desk = SceneId::new();
    let mut ledgers = ledgers();
    ledgers.observe(&scene_read(desk, 10, &[("main", 0.2)]), Some("main"));
    moved(&mut ledgers, "main", 0.4);
    let write = ledgers.take_write("main").expect("a write");
    assert_eq!(write.fence, 10);
    let outputs = vec![output("a", 0.4, 0.2)];
    let ledger = ledgers.ledger_mut("main").expect("a ledger");
    assert_eq!(
        ledger.applied(write.id, Some(13), outputs.clone(), shown(&outputs)),
        Some(Vec::new())
    );
    assert_eq!(ledger.revision(), Some(13));
    assert!(!ledger.has_pending());
    // A read of the revision in between is older, and changes nothing.
    ledgers.observe(&scene_read(desk, 12, &[("main", 0.8)]), Some("main"));
    let ledger = ledgers.zone("main").expect("kept");
    assert_eq!(ledger.revision(), Some(13));
    assert_eq!(read(&ledger.display().expect("drawn"), "a", Field::X), 0.4);
}

#[test]
fn a_fence_from_another_run_is_refused_even_when_its_number_matches() {
    let mut model = Model::new();
    model.edit("a", Field::X, 0.4);
    model.send();
    let fence = model.in_flight.as_ref().expect("a write out").write.fence;
    model.stall_request();
    while model.deliver(0) {}
    // The daemon restarts and its new run counts up to the very number the
    // old run's write is fenced at, before the socket names the new run.
    model.restart();
    while model.revision < fence {
        model.remote(
            "b",
            Field::Y,
            if model.revision.is_multiple_of(2) {
                0.4
            } else {
                0.6
            },
        );
    }
    assert_eq!(model.revision, fence, "the numbers match across runs");
    // The retry reaches the new run, which refuses it for its run alone.
    model.retry();
    model.deliver_write(false);
    while model.deliver(0) {}
    assert!(
        model.applied_keys.is_empty(),
        "no write applied under the old run's fence"
    );
    assert!(model.ledgers.wants_read(), "its key settles nothing now");
    // Once the socket names the new run the ledger re-anchors on it. The
    // old run is gone, so the write it never took is given up on and the
    // user told, rather than sent again over what the new run may hold.
    model.hello();
    assert!(model.ledger().reanchoring());
    model.settle();
    model.check();
    assert!(model.excused.contains(&("a", Field::X)));
    assert_eq!(read(&model.server, "a", Field::X), 0.2);
}
