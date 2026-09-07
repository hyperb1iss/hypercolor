mod lifecycle;
mod stream;
mod transactions;

pub use stream::MacosScreenCaptureSession;
pub use transactions::{
    MacosNativeTransactionError, MacosNativeTransactionPhase, MacosStreamDiagnosticTransaction,
    MacosStreamRequestTransaction,
};
