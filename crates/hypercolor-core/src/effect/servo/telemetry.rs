use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default)]
pub struct ServoTelemetrySnapshot {
    pub soft_stalls_total: u64,
    pub breaker_opens_total: u64,
    pub session_creates_total: u64,
    pub session_create_failures_total: u64,
    pub session_create_wait_total_us: u64,
    pub session_create_wait_max_us: u64,
    pub page_loads_total: u64,
    pub page_load_failures_total: u64,
    pub page_load_wait_total_us: u64,
    pub page_load_wait_max_us: u64,
    pub detached_destroys_total: u64,
    pub detached_destroy_failures_total: u64,
    pub render_requests_total: u64,
    pub render_queue_wait_total_us: u64,
    pub render_queue_wait_max_us: u64,
}

static SERVO_SOFT_STALLS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_BREAKER_OPENS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_SESSION_CREATES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_SESSION_CREATE_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_SESSION_CREATE_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_SESSION_CREATE_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_PAGE_LOADS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_PAGE_LOAD_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_PAGE_LOAD_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_PAGE_LOAD_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_DETACHED_DESTROYS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_DETACHED_DESTROY_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_QUEUE_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_QUEUE_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);

pub(super) fn record_servo_soft_stall() {
    let _ = SERVO_SOFT_STALLS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn record_servo_breaker_open() {
    let _ = SERVO_BREAKER_OPENS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn record_servo_session_create(wait: Duration, success: bool) {
    record_wait(
        wait,
        &SERVO_SESSION_CREATES_TOTAL,
        &SERVO_SESSION_CREATE_WAIT_TOTAL_US,
        &SERVO_SESSION_CREATE_WAIT_MAX_US,
    );
    if !success {
        let _ = SERVO_SESSION_CREATE_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn record_servo_page_load(wait: Duration, success: bool) {
    record_wait(
        wait,
        &SERVO_PAGE_LOADS_TOTAL,
        &SERVO_PAGE_LOAD_WAIT_TOTAL_US,
        &SERVO_PAGE_LOAD_WAIT_MAX_US,
    );
    if !success {
        let _ = SERVO_PAGE_LOAD_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn record_servo_detached_destroy(success: bool) {
    let _ = SERVO_DETACHED_DESTROYS_TOTAL.fetch_add(1, Ordering::Relaxed);
    if !success {
        let _ = SERVO_DETACHED_DESTROY_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn record_servo_render_queue_wait(wait: Duration) {
    record_wait(
        wait,
        &SERVO_RENDER_REQUESTS_TOTAL,
        &SERVO_RENDER_QUEUE_WAIT_TOTAL_US,
        &SERVO_RENDER_QUEUE_WAIT_MAX_US,
    );
}

#[must_use]
pub fn servo_telemetry_snapshot() -> ServoTelemetrySnapshot {
    ServoTelemetrySnapshot {
        soft_stalls_total: SERVO_SOFT_STALLS_TOTAL.load(Ordering::Relaxed),
        breaker_opens_total: SERVO_BREAKER_OPENS_TOTAL.load(Ordering::Relaxed),
        session_creates_total: SERVO_SESSION_CREATES_TOTAL.load(Ordering::Relaxed),
        session_create_failures_total: SERVO_SESSION_CREATE_FAILURES_TOTAL.load(Ordering::Relaxed),
        session_create_wait_total_us: SERVO_SESSION_CREATE_WAIT_TOTAL_US.load(Ordering::Relaxed),
        session_create_wait_max_us: SERVO_SESSION_CREATE_WAIT_MAX_US.load(Ordering::Relaxed),
        page_loads_total: SERVO_PAGE_LOADS_TOTAL.load(Ordering::Relaxed),
        page_load_failures_total: SERVO_PAGE_LOAD_FAILURES_TOTAL.load(Ordering::Relaxed),
        page_load_wait_total_us: SERVO_PAGE_LOAD_WAIT_TOTAL_US.load(Ordering::Relaxed),
        page_load_wait_max_us: SERVO_PAGE_LOAD_WAIT_MAX_US.load(Ordering::Relaxed),
        detached_destroys_total: SERVO_DETACHED_DESTROYS_TOTAL.load(Ordering::Relaxed),
        detached_destroy_failures_total: SERVO_DETACHED_DESTROY_FAILURES_TOTAL
            .load(Ordering::Relaxed),
        render_requests_total: SERVO_RENDER_REQUESTS_TOTAL.load(Ordering::Relaxed),
        render_queue_wait_total_us: SERVO_RENDER_QUEUE_WAIT_TOTAL_US.load(Ordering::Relaxed),
        render_queue_wait_max_us: SERVO_RENDER_QUEUE_WAIT_MAX_US.load(Ordering::Relaxed),
    }
}

fn record_wait(wait: Duration, count: &AtomicU64, total_us: &AtomicU64, max_us: &AtomicU64) {
    let wait_us = u64::try_from(wait.as_micros()).unwrap_or(u64::MAX);
    let _ = count.fetch_add(1, Ordering::Relaxed);
    let _ = total_us.fetch_add(wait_us, Ordering::Relaxed);
    let _ = max_us.fetch_max(wait_us, Ordering::Relaxed);
}
