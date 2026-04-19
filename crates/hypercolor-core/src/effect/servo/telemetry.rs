use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, Default)]
pub struct ServoTelemetrySnapshot {
    pub soft_stalls_total: u64,
    pub breaker_opens_total: u64,
}

static SERVO_SOFT_STALLS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_BREAKER_OPENS_TOTAL: AtomicU64 = AtomicU64::new(0);

pub(super) fn record_servo_soft_stall() {
    let _ = SERVO_SOFT_STALLS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn record_servo_breaker_open() {
    let _ = SERVO_BREAKER_OPENS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

#[must_use]
pub fn servo_telemetry_snapshot() -> ServoTelemetrySnapshot {
    ServoTelemetrySnapshot {
        soft_stalls_total: SERVO_SOFT_STALLS_TOTAL.load(Ordering::Relaxed),
        breaker_opens_total: SERVO_BREAKER_OPENS_TOTAL.load(Ordering::Relaxed),
    }
}
