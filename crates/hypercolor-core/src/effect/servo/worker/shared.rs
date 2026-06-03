use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tracing::warn;

use super::super::circuit_breaker::ServoCircuitBreaker;
use super::super::memory::ServoMemoryReportSnapshot;
use super::super::worker_client::{
    ServoWorkerClient, ServoWorkerClientSharedState, WORKER_READY_TIMEOUT, WorkerCommand,
};
use super::ServoWorkerRuntime;
use super::console::panic_payload_message;

enum SharedServoWorkerState {
    Vacant,
    Running(ServoWorker),
    Poisoned { reason: String },
}

static SERVO_WORKER: OnceLock<Mutex<SharedServoWorkerState>> = OnceLock::new();
static SERVO_CIRCUIT_BREAKER: ServoCircuitBreaker = ServoCircuitBreaker::new();

pub(in crate::effect::servo) fn acquire_servo_worker() -> Result<ServoWorkerClient> {
    if !SERVO_CIRCUIT_BREAKER.can_attempt() {
        let cooldown = SERVO_CIRCUIT_BREAKER
            .cooldown_remaining()
            .unwrap_or(Duration::ZERO);
        bail!(
            "Servo worker is cooling down after repeated failures; retry in {}s",
            cooldown.as_secs().max(1)
        );
    }

    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    match &mut *guard {
        SharedServoWorkerState::Running(worker) => {
            let client = worker.client();
            if client.is_ok() {
                SERVO_CIRCUIT_BREAKER.record_success();
            } else {
                SERVO_CIRCUIT_BREAKER.record_failure();
            }
            return client;
        }
        SharedServoWorkerState::Poisoned { reason } => {
            bail!("Servo runtime is unrecoverable until the daemon restarts: {reason}");
        }
        SharedServoWorkerState::Vacant => {}
    }

    match ServoWorker::spawn() {
        Ok(worker) => match worker.client() {
            Ok(client) => {
                *guard = SharedServoWorkerState::Running(worker);
                SERVO_CIRCUIT_BREAKER.record_success();
                Ok(client)
            }
            Err(error) => {
                SERVO_CIRCUIT_BREAKER.record_failure();
                Err(error)
            }
        },
        Err(error) => {
            SERVO_CIRCUIT_BREAKER.record_failure();
            Err(error)
        }
    }
}

pub fn servo_memory_report_snapshot() -> Result<ServoMemoryReportSnapshot> {
    let client = {
        let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
        let mut guard = match slot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        match &mut *guard {
            SharedServoWorkerState::Running(worker) => worker.client(),
            SharedServoWorkerState::Poisoned { reason } => {
                bail!("Servo runtime is unrecoverable until the daemon restarts: {reason}");
            }
            SharedServoWorkerState::Vacant => {
                bail!("Servo worker is not running");
            }
        }?
    };

    client.memory_report()
}

pub(in crate::effect::servo) fn servo_worker_is_fatal_error(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("disconnected")
        || message.contains("timed out waiting for servo worker readiness")
        || message.contains("timed out waiting for servo worker shutdown")
        || message.contains("timed out waiting for servo session creation after")
        || message.contains("timed out waiting for servo page load after")
        || message.contains("timed out waiting for servo session destroy after")
        || message.contains("timed out waiting for servo frame response")
        || message.contains("failed to send create-session command to servo worker")
        || message.contains("failed to send load command to servo worker")
        || message.contains("failed to send load-url command to servo worker")
        || message.contains("failed to send render command to servo worker")
        || message.contains("failed to send unload command to servo worker")
        || message.contains("failed to send destroy-session command to servo worker")
        || message.contains("failed to send detached destroy-session command to servo worker")
}

pub(in crate::effect::servo) fn poison_shared_servo_worker_if_fatal(
    context: &str,
    error: &anyhow::Error,
) {
    if !servo_worker_is_fatal_error(error) {
        SERVO_CIRCUIT_BREAKER.record_failure();
        return;
    }
    let message = format!("{context}: {error}");
    poison_shared_servo_worker(&message);
}

pub(in crate::effect::servo) fn poison_shared_servo_worker(reason: &str) {
    SERVO_CIRCUIT_BREAKER.record_failure();

    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let previous = std::mem::replace(
        &mut *guard,
        SharedServoWorkerState::Poisoned {
            reason: reason.to_owned(),
        },
    );
    drop(guard);

    match previous {
        SharedServoWorkerState::Vacant => {
            warn!(
                reason = reason,
                "Marked shared Servo worker unrecoverable; restart the daemon to use HTML effects again"
            );
        }
        SharedServoWorkerState::Running(mut worker) => {
            let had_command_tx = worker.command_tx.take().is_some();
            let had_thread_handle = worker.thread_handle.take().is_some();
            warn!(
                reason = reason,
                had_command_tx,
                had_thread_handle,
                "Marked shared Servo worker unrecoverable; restart the daemon to use HTML effects again"
            );
        }
        SharedServoWorkerState::Poisoned { .. } => {}
    }
}

#[cfg(test)]
pub(in crate::effect::servo) fn shutdown_shared_servo_worker() -> Result<()> {
    retire_shared_servo_worker(SharedServoWorkerState::Vacant)
}

pub fn shutdown_servo_runtime() -> Result<()> {
    retire_shared_servo_worker(SharedServoWorkerState::Poisoned {
        reason: "Servo runtime was shut down for process exit".to_owned(),
    })
}

fn retire_shared_servo_worker(replacement: SharedServoWorkerState) -> Result<()> {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let previous = std::mem::replace(&mut *guard, replacement);
    drop(guard);

    match previous {
        SharedServoWorkerState::Vacant | SharedServoWorkerState::Poisoned { .. } => Ok(()),
        SharedServoWorkerState::Running(mut worker) => worker.shutdown(),
    }
}

pub(in crate::effect::servo) struct ServoWorker {
    pub(super) command_tx: Option<Sender<WorkerCommand>>,
    pub(super) thread_handle: Option<thread::JoinHandle<()>>,
    pub(super) client_state: std::sync::Arc<ServoWorkerClientSharedState>,
}

impl ServoWorker {
    fn spawn() -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let client_state = std::sync::Arc::new(ServoWorkerClientSharedState::new());

        let thread_handle = thread::Builder::new()
            .name("hypercolor-servo-worker".to_owned())
            .spawn(move || {
                let runtime = match ServoWorkerRuntime::new() {
                    Ok(runtime) => {
                        let _ = ready_tx.send(Ok(()));
                        runtime
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                runtime.run(command_rx);
            })
            .context("failed to spawn Servo worker thread")?;

        let readiness = match ready_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            Ok(readiness) => readiness,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                bail!(
                    "timed out waiting for Servo worker readiness after {}ms",
                    WORKER_READY_TIMEOUT.as_millis()
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("Servo worker exited before reporting readiness");
            }
        };
        readiness?;

        Ok(Self {
            command_tx: Some(command_tx),
            thread_handle: Some(thread_handle),
            client_state,
        })
    }

    pub(super) fn client(&self) -> Result<ServoWorkerClient> {
        Ok(ServoWorkerClient::new(
            self.command_tx()?.clone(),
            std::sync::Arc::clone(&self.client_state),
        ))
    }

    pub(super) fn shutdown(&mut self) -> Result<()> {
        let command_tx = self.command_tx.take();
        if let Some(command_tx) = command_tx {
            let (response_tx, response_rx) = mpsc::sync_channel(1);
            if command_tx
                .send(WorkerCommand::Shutdown { response_tx })
                .is_ok()
            {
                match response_rx.recv_timeout(WORKER_READY_TIMEOUT) {
                    Ok(()) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        bail!(
                            "timed out waiting for Servo worker shutdown after {}ms",
                            WORKER_READY_TIMEOUT.as_millis()
                        );
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        bail!("Servo worker disconnected before acknowledging shutdown");
                    }
                }
            }
        }

        if let Some(thread_handle) = self.thread_handle.take() {
            thread_handle.join().map_err(|panic| {
                anyhow!(
                    "Servo worker thread panicked during shutdown: {}",
                    panic_payload_message(&*panic)
                )
            })?;
        }

        Ok(())
    }

    fn command_tx(&self) -> Result<&Sender<WorkerCommand>> {
        self.command_tx
            .as_ref()
            .ok_or_else(|| anyhow!("Servo worker is not running"))
    }
}

impl Drop for ServoWorker {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            warn!(%error, "Servo worker dropped with shutdown error");
        }
    }
}

#[cfg(test)]
pub(in crate::effect::servo) fn reset_shared_servo_worker_state() {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = SharedServoWorkerState::Vacant;
}

#[cfg(test)]
pub(in crate::effect::servo) fn install_running_shared_worker(worker: ServoWorker) {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = SharedServoWorkerState::Running(worker);
}

#[cfg(test)]
pub(in crate::effect::servo) fn install_poisoned_shared_worker(reason: impl Into<String>) {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = SharedServoWorkerState::Poisoned {
        reason: reason.into(),
    };
}

#[cfg(test)]
pub(in crate::effect::servo) fn shared_worker_is_vacant() -> bool {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    matches!(&*guard, SharedServoWorkerState::Vacant)
}
