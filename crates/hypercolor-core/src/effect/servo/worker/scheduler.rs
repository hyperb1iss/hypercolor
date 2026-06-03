use std::collections::{HashMap, VecDeque};
use std::sync::mpsc;
use std::time::Instant;

use anyhow::{Result, anyhow};

use super::super::telemetry::{record_servo_render_queue_depth, record_servo_render_superseded};
use super::super::worker_client::{
    ServoProducerRole, ServoRenderMode, ServoSessionId, WorkerCommand,
};
use crate::effect::traits::EffectRenderOutput;

pub(super) struct PendingRenderCommand {
    pub(super) session_id: ServoSessionId,
    pub(super) producer_role: ServoProducerRole,
    pub(super) scripts: Vec<String>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) mode: ServoRenderMode,
    pub(super) submitted_at: Instant,
    pub(super) response_tx: mpsc::SyncSender<Result<EffectRenderOutput>>,
}

pub(super) enum ScheduledServoWork {
    Command(WorkerCommand),
    Render(PendingRenderCommand),
}

enum ServoWorkKey {
    Command(WorkerCommand),
    Render {
        session_id: ServoSessionId,
        producer_role: ServoProducerRole,
        slot: u64,
    },
}

#[derive(Default)]
pub(super) struct ServoWorkerScheduler {
    queue: VecDeque<ServoWorkKey>,
    pending_renders: HashMap<(ServoSessionId, u64), PendingRenderCommand>,
    open_render_slots: HashMap<ServoSessionId, u64>,
    next_render_slot: u64,
    last_render_role: Option<ServoProducerRole>,
}

impl ServoWorkerScheduler {
    pub(super) fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.pending_renders.is_empty()
    }

    fn depth(&self) -> usize {
        self.queue.len()
    }

    fn record_depth(&self) {
        record_servo_render_queue_depth(self.depth());
    }

    pub(super) fn push(&mut self, command: WorkerCommand) {
        match command {
            WorkerCommand::Render {
                session_id,
                producer_role,
                scripts,
                width,
                height,
                mode,
                submitted_at,
                response_tx,
            } => {
                let pending = PendingRenderCommand {
                    session_id,
                    producer_role,
                    scripts,
                    width,
                    height,
                    mode,
                    submitted_at,
                    response_tx,
                };
                let slot = if let Some(slot) = self.open_render_slots.get(&session_id).copied() {
                    slot
                } else {
                    let slot = self.next_render_slot;
                    self.next_render_slot = self.next_render_slot.saturating_add(1);
                    self.open_render_slots.insert(session_id, slot);
                    self.queue.push_back(ServoWorkKey::Render {
                        session_id,
                        producer_role,
                        slot,
                    });
                    slot
                };
                if let Some(replaced) = self.pending_renders.insert((session_id, slot), pending) {
                    record_servo_render_superseded();
                    let _ = replaced.response_tx.send(Err(anyhow!(
                        "Servo render request superseded by a newer frame"
                    )));
                }
                self.record_depth();
            }
            command => {
                self.open_render_slots.clear();
                self.queue.push_back(ServoWorkKey::Command(command));
                self.record_depth();
            }
        }
    }

    pub(super) fn next(&mut self) -> Option<ScheduledServoWork> {
        loop {
            let Some(front) = self.queue.front() else {
                self.record_depth();
                return None;
            };
            if matches!(front, ServoWorkKey::Command(_)) {
                let Some(ServoWorkKey::Command(command)) = self.queue.pop_front() else {
                    unreachable!("front was checked as a command")
                };
                self.record_depth();
                return Some(ScheduledServoWork::Command(command));
            }

            let Some(index) = self.next_render_index_before_barrier() else {
                let _ = self.queue.pop_front();
                continue;
            };
            let Some(ServoWorkKey::Render {
                session_id, slot, ..
            }) = self.queue.remove(index)
            else {
                continue;
            };
            if let Some(render) = self.pending_renders.remove(&(session_id, slot)) {
                if self
                    .open_render_slots
                    .get(&session_id)
                    .is_some_and(|open_slot| *open_slot == slot)
                {
                    self.open_render_slots.remove(&session_id);
                }
                self.last_render_role = Some(render.producer_role);
                self.record_depth();
                return Some(ScheduledServoWork::Render(render));
            }
        }
    }

    fn next_render_index_before_barrier(&self) -> Option<usize> {
        let mut first_valid = None;
        for (index, key) in self.queue.iter().enumerate() {
            match key {
                ServoWorkKey::Command(_) => break,
                ServoWorkKey::Render {
                    session_id,
                    producer_role,
                    slot,
                } => {
                    if !self.pending_renders.contains_key(&(*session_id, *slot)) {
                        continue;
                    }
                    first_valid.get_or_insert(index);
                    if self
                        .last_render_role
                        .is_some_and(|last_role| *producer_role != last_role)
                    {
                        return Some(index);
                    }
                }
            }
        }
        first_valid
    }
}
