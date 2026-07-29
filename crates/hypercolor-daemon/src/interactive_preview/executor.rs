use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::{self, JoinHandle};

type PreviewJob = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone)]
pub(crate) struct PreviewWorkerPool {
    inner: Arc<PreviewWorkerPoolInner>,
}

struct PreviewWorkerPoolInner {
    sender: Mutex<Option<mpsc::Sender<PreviewJob>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    worker_count: usize,
}

impl PreviewWorkerPool {
    pub(crate) fn new(name: &'static str, worker_count: usize) -> std::io::Result<Self> {
        let worker_count = worker_count.max(1);
        let (sender, receiver) = mpsc::channel::<PreviewJob>();
        let receiver = Arc::new(Mutex::new(receiver));
        let mut workers = Vec::new();
        workers.try_reserve_exact(worker_count).map_err(|_| {
            std::io::Error::other("failed to reserve interactive preview worker handles")
        })?;
        for index in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            let handle = thread::Builder::new()
                .name(format!("hypercolor-{name}-{index}"))
                .spawn(move || worker_loop(&receiver))?;
            workers.push(handle);
        }
        Ok(Self {
            inner: Arc::new(PreviewWorkerPoolInner {
                sender: Mutex::new(Some(sender)),
                workers: Mutex::new(workers),
                worker_count,
            }),
        })
    }

    pub(crate) fn execute(
        &self,
        job: impl FnOnce() + Send + 'static,
    ) -> Result<(), PreviewWorkerClosed> {
        let sender = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        sender
            .as_ref()
            .ok_or(PreviewWorkerClosed)?
            .send(Box::new(job))
            .map_err(|_| PreviewWorkerClosed)
    }

    #[must_use]
    pub(crate) fn worker_count(&self) -> usize {
        self.inner.worker_count
    }
}

impl std::fmt::Debug for PreviewWorkerPool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreviewWorkerPool")
            .field("worker_count", &self.worker_count())
            .finish_non_exhaustive()
    }
}

impl Drop for PreviewWorkerPoolInner {
    fn drop(&mut self) {
        self.sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let workers = self
            .workers
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        for worker in workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreviewWorkerClosed;

fn worker_loop(receiver: &Mutex<mpsc::Receiver<PreviewJob>>) {
    loop {
        let job = receiver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .recv();
        let Ok(job) = job else {
            return;
        };
        job();
    }
}
