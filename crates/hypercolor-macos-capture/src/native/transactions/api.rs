use std::fmt;
use std::time::Instant;

#[cfg(test)]
use crate::MacosCaptureError;
use crate::MacosProtectedSourceState;

use super::{
    MacosNativeTransactionError, MacosNativeTransactionPhase, TransactionCompleter,
    TransactionIdentity, TransactionWaiter,
};

pub struct MacosStreamRequestTransaction {
    generation: u64,
    waiter: Option<TransactionWaiter<()>>,
}

impl MacosStreamRequestTransaction {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn current_deadline(&self) -> Option<Instant> {
        self.waiter
            .as_ref()
            .and_then(TransactionWaiter::current_deadline)
    }

    pub fn wait(mut self) -> Result<(), MacosNativeTransactionError> {
        self.waiter
            .take()
            .expect("macOS stream request transaction waits once")
            .wait()
    }

    pub fn cancel(mut self) -> bool {
        self.waiter
            .take()
            .expect("macOS stream request transaction cancels once")
            .cancel()
    }
}

impl fmt::Debug for MacosStreamRequestTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacosStreamRequestTransaction")
            .field("generation", &self.generation)
            .field("deadline", &self.current_deadline())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl MacosStreamRequestTransaction {
    pub(in crate::native) fn try_recv(
        &self,
    ) -> Result<Result<(), MacosCaptureError>, std::sync::mpsc::TryRecvError> {
        self.waiter
            .as_ref()
            .and_then(TransactionWaiter::try_outcome)
            .map(map_test_request_outcome)
            .ok_or(std::sync::mpsc::TryRecvError::Empty)
    }

    pub(in crate::native) fn recv(
        &self,
    ) -> Result<Result<(), MacosCaptureError>, std::sync::mpsc::RecvError> {
        Ok(map_test_request_outcome(
            self.waiter
                .as_ref()
                .expect("test request transaction retains its waiter")
                .wait_outcome(),
        ))
    }
}

#[cfg(test)]
fn map_test_request_outcome(
    outcome: Result<(), MacosNativeTransactionError>,
) -> Result<(), MacosCaptureError> {
    outcome.map_err(|error| match error {
        MacosNativeTransactionError::Capture(error) => error,
        error => MacosCaptureError::CaptureWorkerStartFailed(error.to_string()),
    })
}

pub struct MacosStreamDiagnosticTransaction {
    generation: u64,
    waiter: Option<TransactionWaiter<MacosProtectedSourceState>>,
}

impl MacosStreamDiagnosticTransaction {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn current_deadline(&self) -> Option<Instant> {
        self.waiter
            .as_ref()
            .and_then(TransactionWaiter::current_deadline)
    }

    pub fn wait(mut self) -> Result<MacosProtectedSourceState, MacosNativeTransactionError> {
        self.waiter
            .take()
            .expect("macOS stream diagnostic transaction waits once")
            .wait()
    }

    pub fn wait_until(
        mut self,
        deadline: Instant,
    ) -> Result<MacosProtectedSourceState, MacosNativeTransactionError> {
        self.waiter
            .take()
            .expect("macOS stream diagnostic transaction waits once")
            .wait_until(deadline)
    }

    pub fn cancel(mut self) -> bool {
        self.waiter
            .take()
            .expect("macOS stream diagnostic transaction cancels once")
            .cancel()
    }
}

impl fmt::Debug for MacosStreamDiagnosticTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacosStreamDiagnosticTransaction")
            .field("generation", &self.generation)
            .field("deadline", &self.current_deadline())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl MacosStreamDiagnosticTransaction {
    pub(in crate::native) fn try_recv(
        &self,
    ) -> Result<MacosProtectedSourceState, std::sync::mpsc::TryRecvError> {
        self.waiter
            .as_ref()
            .and_then(TransactionWaiter::try_outcome)
            .map(|outcome| outcome.expect("fixture diagnostic transaction succeeds"))
            .ok_or(std::sync::mpsc::TryRecvError::Empty)
    }

    pub(in crate::native) fn recv(
        &self,
    ) -> Result<MacosProtectedSourceState, std::sync::mpsc::RecvError> {
        Ok(self
            .waiter
            .as_ref()
            .expect("test diagnostic transaction retains its waiter")
            .wait_outcome()
            .expect("fixture diagnostic transaction succeeds"))
    }
}

pub(in crate::native) fn stream_request_transaction(
    generation: u64,
    deadline: Instant,
) -> (MacosStreamRequestTransaction, TransactionCompleter<()>) {
    let completer = TransactionCompleter::new(
        TransactionIdentity {
            generation,
            phase: MacosNativeTransactionPhase::StreamStart,
        },
        Some(deadline),
    );
    let transaction = MacosStreamRequestTransaction {
        generation,
        waiter: Some(completer.waiter()),
    };
    (transaction, completer)
}

pub(in crate::native) fn stream_diagnostic_transaction(
    generation: u64,
) -> (
    MacosStreamDiagnosticTransaction,
    TransactionCompleter<MacosProtectedSourceState>,
) {
    let completer = TransactionCompleter::new(
        TransactionIdentity {
            generation,
            phase: MacosNativeTransactionPhase::SourceResolution,
        },
        None,
    );
    let transaction = MacosStreamDiagnosticTransaction {
        generation,
        waiter: Some(completer.waiter()),
    };
    (transaction, completer)
}
