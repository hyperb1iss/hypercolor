use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use hypercolor_types::canvas::Canvas;

use crate::effect::traits::EffectRenderOutput;

use super::super::memory::{ServoMemoryReportSnapshot, ServoMemoryReportTotals};
use super::super::worker_client::{
    ServoFramePayload, ServoWorkerClient, ServoWorkerClientSharedState, WorkerCommand,
};
use super::ServoWorker;

pub static SHARED_WORKER_STATE_TEST_LOCK: LazyLock<StdMutex<()>> =
    LazyLock::new(|| StdMutex::new(()));

pub struct RecordedRenderCommand {
    pub scripts: Vec<String>,
    pub frame_payloads: Vec<ServoFramePayload>,
    pub width: u32,
    pub height: u32,
    #[cfg(feature = "servo-gpu-import")]
    pub prefer_gpu: bool,
    #[cfg(feature = "servo-gpu-import")]
    pub reuse_cached_on_no_ready: bool,
}

pub struct RecordedLoadCommand {
    pub width: u32,
    pub height: u32,
}

fn solid_canvas(r: u8, g: u8, b: u8) -> Canvas {
    use hypercolor_types::canvas::{DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH, Rgba};
    let mut canvas = Canvas::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT);
    canvas.fill(Rgba::new(r, g, b, 255));
    canvas
}

fn empty_memory_report() -> ServoMemoryReportSnapshot {
    ServoMemoryReportSnapshot {
        processes: Vec::new(),
        totals: ServoMemoryReportTotals::default(),
    }
}

pub fn spawn_test_worker() -> (ServoWorker, Arc<AtomicBool>) {
    let (command_tx, command_rx) = mpsc::channel();
    let client_state = Arc::new(ServoWorkerClientSharedState::new());
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_clone = Arc::clone(&stopped);
    let thread_handle = thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            match command {
                WorkerCommand::CreateSession { response_tx, .. }
                | WorkerCommand::Load { response_tx, .. }
                | WorkerCommand::LoadUrl { response_tx, .. }
                | WorkerCommand::DestroySession { response_tx, .. } => {
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::Shutdown { response_tx } => {
                    stopped_clone.store(true, Ordering::SeqCst);
                    let _ = response_tx.send(());
                    break;
                }
                WorkerCommand::MemoryReport { response_tx } => {
                    let _ = response_tx.send(Ok(empty_memory_report()));
                }
                WorkerCommand::Render { response_tx, .. } => {
                    let _ = response_tx.send(Ok(EffectRenderOutput::Cpu(solid_canvas(12, 34, 56))));
                }
            }
        }
    });

    (
        ServoWorker {
            command_tx: Some(command_tx),
            thread_handle: Some(thread_handle),
            client_state,
        },
        stopped,
    )
}

#[allow(
    clippy::type_complexity,
    reason = "test harness returns all the plumbing in one tuple; breaking it into a named type would only move the noise"
)]
pub fn spawn_render_test_worker() -> (
    ServoWorker,
    Receiver<RecordedRenderCommand>,
    Sender<anyhow::Result<Canvas>>,
    Receiver<()>,
    Receiver<()>,
    Arc<AtomicBool>,
) {
    let (command_tx, command_rx) = mpsc::channel();
    let client_state = Arc::new(ServoWorkerClientSharedState::new());
    let (render_tx, render_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let (delivered_tx, delivered_rx) = mpsc::channel();
    let (unload_tx, unload_rx) = mpsc::channel();
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_clone = Arc::clone(&stopped);
    let thread_handle = thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            match command {
                WorkerCommand::CreateSession { response_tx, .. } => {
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::Render {
                    scripts,
                    frame_payloads,
                    width,
                    height,
                    mode,
                    response_tx,
                    ..
                } => {
                    #[cfg(feature = "servo-gpu-import")]
                    let prefer_gpu = mode.prefers_gpu();
                    #[cfg(feature = "servo-gpu-import")]
                    let reuse_cached_on_no_ready = mode.reuse_cached_gpu_frame_on_no_ready();
                    #[cfg(not(feature = "servo-gpu-import"))]
                    let _ = mode;
                    let _ = render_tx.send(RecordedRenderCommand {
                        scripts,
                        frame_payloads,
                        width,
                        height,
                        #[cfg(feature = "servo-gpu-import")]
                        prefer_gpu,
                        #[cfg(feature = "servo-gpu-import")]
                        reuse_cached_on_no_ready,
                    });
                    let result = result_rx
                        .recv()
                        .unwrap_or_else(|_| Ok(solid_canvas(12, 34, 56)));
                    let _ = response_tx.send(result.map(EffectRenderOutput::Cpu));
                    let _ = delivered_tx.send(());
                }
                WorkerCommand::DestroySession { response_tx, .. } => {
                    let _ = unload_tx.send(());
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::Shutdown { response_tx } => {
                    stopped_clone.store(true, Ordering::SeqCst);
                    let _ = response_tx.send(());
                    break;
                }
                WorkerCommand::Load { response_tx, .. } => {
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::LoadUrl { response_tx, .. } => {
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::MemoryReport { response_tx } => {
                    let _ = response_tx.send(Ok(empty_memory_report()));
                }
            }
        }
    });

    (
        ServoWorker {
            command_tx: Some(command_tx),
            thread_handle: Some(thread_handle),
            client_state,
        },
        render_rx,
        result_tx,
        delivered_rx,
        unload_rx,
        stopped,
    )
}

pub fn spawn_load_test_worker() -> (
    ServoWorker,
    Receiver<RecordedLoadCommand>,
    Receiver<()>,
    Arc<AtomicBool>,
) {
    let (command_tx, command_rx) = mpsc::channel();
    let client_state = Arc::new(ServoWorkerClientSharedState::new());
    let (load_tx, load_rx) = mpsc::channel();
    let (unload_tx, unload_rx) = mpsc::channel();
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_clone = Arc::clone(&stopped);
    let thread_handle = thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            match command {
                WorkerCommand::CreateSession {
                    width,
                    height,
                    response_tx,
                    ..
                } => {
                    let _ = load_tx.send(RecordedLoadCommand { width, height });
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::Load { response_tx, .. } => {
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::LoadUrl { response_tx, .. } => {
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::DestroySession { response_tx, .. } => {
                    let _ = unload_tx.send(());
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::Render { response_tx, .. } => {
                    let _ = response_tx.send(Ok(EffectRenderOutput::Cpu(solid_canvas(12, 34, 56))));
                }
                WorkerCommand::Shutdown { response_tx } => {
                    stopped_clone.store(true, Ordering::SeqCst);
                    let _ = response_tx.send(());
                    break;
                }
                WorkerCommand::MemoryReport { response_tx } => {
                    let _ = response_tx.send(Ok(empty_memory_report()));
                }
            }
        }
    });

    (
        ServoWorker {
            command_tx: Some(command_tx),
            thread_handle: Some(thread_handle),
            client_state,
        },
        load_rx,
        unload_rx,
        stopped,
    )
}

pub fn spawn_blocking_load_test_worker() -> (
    ServoWorker,
    Receiver<RecordedLoadCommand>,
    Sender<()>,
    Receiver<()>,
    Arc<AtomicBool>,
) {
    let (command_tx, command_rx) = mpsc::channel();
    let client_state = Arc::new(ServoWorkerClientSharedState::new());
    let (load_tx, load_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (unload_tx, unload_rx) = mpsc::channel();
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_clone = Arc::clone(&stopped);
    let thread_handle = thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            match command {
                WorkerCommand::CreateSession {
                    width,
                    height,
                    response_tx,
                    ..
                } => {
                    let _ = load_tx.send(RecordedLoadCommand { width, height });
                    let _ = release_rx.recv();
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::Load { response_tx, .. }
                | WorkerCommand::LoadUrl { response_tx, .. } => {
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::DestroySession { response_tx, .. } => {
                    let _ = unload_tx.send(());
                    let _ = response_tx.send(Ok(()));
                }
                WorkerCommand::Render { response_tx, .. } => {
                    let _ = response_tx.send(Ok(EffectRenderOutput::Cpu(solid_canvas(12, 34, 56))));
                }
                WorkerCommand::Shutdown { response_tx } => {
                    stopped_clone.store(true, Ordering::SeqCst);
                    let _ = response_tx.send(());
                    break;
                }
                WorkerCommand::MemoryReport { response_tx } => {
                    let _ = response_tx.send(Ok(empty_memory_report()));
                }
            }
        }
    });

    (
        ServoWorker {
            command_tx: Some(command_tx),
            thread_handle: Some(thread_handle),
            client_state,
        },
        load_rx,
        release_tx,
        unload_rx,
        stopped,
    )
}

pub fn worker_client_from(worker: &ServoWorker) -> ServoWorkerClient {
    worker.client().expect("test worker client")
}
