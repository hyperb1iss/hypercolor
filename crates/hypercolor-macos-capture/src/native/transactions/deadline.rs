use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::time::Instant;

use super::lock;

type DeadlineCallback = Box<dyn FnOnce() + Send + 'static>;

struct ScheduledDeadline {
    callback: Option<DeadlineCallback>,
}

#[derive(Default)]
struct DeadlineQueue {
    deadlines: BTreeMap<(Instant, u64), ScheduledDeadline>,
    deadline_by_id: HashMap<u64, Instant>,
}

struct DeadlineSchedulerInner {
    next_id: AtomicU64,
    queue: Mutex<DeadlineQueue>,
    ready: Condvar,
}

#[derive(Clone)]
pub(in crate::native) struct DeadlineScheduler {
    inner: Arc<DeadlineSchedulerInner>,
}

pub(in crate::native) struct DeadlineTicket {
    scheduler: Weak<DeadlineSchedulerInner>,
    id: u64,
    armed: bool,
}

impl DeadlineScheduler {
    pub(in crate::native) fn start(thread_name: &str) -> io::Result<Self> {
        let scheduler = Self::manual();
        let inner = Arc::clone(&scheduler.inner);
        thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || deadline_loop(&inner))?;
        Ok(scheduler)
    }

    pub(in crate::native) fn manual() -> Self {
        Self {
            inner: Arc::new(DeadlineSchedulerInner {
                next_id: AtomicU64::new(1),
                queue: Mutex::new(DeadlineQueue::default()),
                ready: Condvar::new(),
            }),
        }
    }

    pub(in crate::native) fn schedule(
        &self,
        deadline: Instant,
        callback: impl FnOnce() + Send + 'static,
    ) -> io::Result<DeadlineTicket> {
        let id = self
            .inner
            .next_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .map_err(|_| io::Error::other("macOS native deadline identity exhausted"))?;
        let mut queue = lock(&self.inner.queue);
        queue.deadline_by_id.insert(id, deadline);
        queue.deadlines.insert(
            (deadline, id),
            ScheduledDeadline {
                callback: Some(Box::new(callback)),
            },
        );
        drop(queue);
        self.inner.ready.notify_one();
        Ok(DeadlineTicket {
            scheduler: Arc::downgrade(&self.inner),
            id,
            armed: true,
        })
    }

    #[cfg(test)]
    pub(in crate::native) fn expire_through(&self, now: Instant) {
        run_due_callbacks(&self.inner, now);
    }

    #[cfg(test)]
    pub(in crate::native) fn pending(&self) -> usize {
        lock(&self.inner.queue).deadlines.len()
    }
}

impl DeadlineTicket {
    fn cancel(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let Some(scheduler) = self.scheduler.upgrade() else {
            return;
        };
        let mut queue = lock(&scheduler.queue);
        if let Some(deadline) = queue.deadline_by_id.remove(&self.id) {
            queue.deadlines.remove(&(deadline, self.id));
        }
        drop(queue);
        scheduler.ready.notify_one();
    }
}

impl Drop for DeadlineTicket {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn deadline_loop(scheduler: &DeadlineSchedulerInner) {
    loop {
        let queue = lock(&scheduler.queue);
        let Some((deadline, _)) = queue.deadlines.first_key_value().map(|(key, _)| *key) else {
            drop(
                scheduler
                    .ready
                    .wait(queue)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            continue;
        };
        let now = Instant::now();
        if deadline > now {
            let (waiting, _) = scheduler
                .ready
                .wait_timeout(queue, deadline.duration_since(now))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            drop(waiting);
            continue;
        }
        drop(queue);
        run_due_callbacks(scheduler, now);
    }
}

fn run_due_callbacks(scheduler: &DeadlineSchedulerInner, now: Instant) {
    let callbacks = {
        let mut queue = lock(&scheduler.queue);
        let due = queue
            .deadlines
            .range(..=(now, u64::MAX))
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        due.into_iter()
            .filter_map(|key| {
                queue.deadline_by_id.remove(&key.1);
                queue
                    .deadlines
                    .remove(&key)
                    .and_then(|mut deadline| deadline.callback.take())
            })
            .collect::<Vec<_>>()
    };
    for callback in callbacks {
        callback();
    }
}
