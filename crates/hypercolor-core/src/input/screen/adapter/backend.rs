use std::time::Duration;

use super::{
    CaptureSession, CaptureSessionDeadline, CaptureSessionReadiness, CaptureSessionTransaction,
    PreparedCaptureSession, ReservedCaptureSessionAuthority,
};

pub(in crate::input::screen) trait CaptureBackend: Sized {
    type Worker: CaptureSession + Send + 'static;
    type Readiness: CaptureSessionReadiness + Send + 'static;
    type SpawnRequest;

    const READINESS_TIMEOUT: Duration;

    fn spawn_worker(
        request: Self::SpawnRequest,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>>;
}

pub(in crate::input::screen) fn prepare_backend_worker<B: CaptureBackend>(
    request: B::SpawnRequest,
    reservation: ReservedCaptureSessionAuthority,
) -> anyhow::Result<PreparedCaptureSession<B::Worker>> {
    B::spawn_worker(request, reservation)?
        .prepare(CaptureSessionDeadline::after(B::READINESS_TIMEOUT))
}
