use std::alloc::Layout;
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use hypercolor_macos_capture::{MACOS_STREAM_QUEUE_DEPTH, MacosCaptureError};

use super::{MacosScreenRuntimeTelemetry, lock};
use crate::input::screen::{
    ScreenByteAdmissionCoordinator, ScreenByteAdmissionError, ScreenByteLease,
    ScreenByteReservation,
};

#[derive(Clone)]
pub(super) struct MacosSurfacePool {
    inner: Arc<MacosSurfacePoolInner>,
}

struct MacosSurfacePoolInner {
    coordinator: ScreenByteAdmissionCoordinator,
    telemetry: Arc<MacosScreenRuntimeTelemetry>,
    metadata_lease: ScreenByteLease,
    state: Mutex<MacosSurfacePoolState>,
}

struct MacosSurfacePoolState {
    initial_surface_reserve: ScreenByteReservation,
    live: Option<Box<LiveSurface>>,
    next_generation: u64,
}

struct LiveSurface {
    iosurface_id: u32,
    generation: u64,
    token: Weak<MacosSurfaceAdmissionToken>,
    next: Option<Box<Self>>,
}

#[cfg(test)]
std::thread_local! {
    static POOL_DROP_EVENTS: std::cell::RefCell<Vec<&'static str>> = const {
        std::cell::RefCell::new(Vec::new())
    };
    static TOP_UP_EVENTS: std::cell::RefCell<Vec<&'static str>> = const {
        std::cell::RefCell::new(Vec::new())
    };
    static TOP_UP_PEAK_SNAPSHOTS: std::cell::RefCell<Vec<(&'static str, u64, u64)>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

#[cfg(test)]
impl Drop for LiveSurface {
    fn drop(&mut self) {
        record_pool_drop_event("live_surface_drop");
    }
}

#[cfg(test)]
fn record_pool_drop_event(event: &'static str) {
    POOL_DROP_EVENTS.with(|events| events.borrow_mut().push(event));
}

#[cfg(test)]
fn record_top_up_event(event: &'static str) {
    TOP_UP_EVENTS.with(|events| events.borrow_mut().push(event));
}

#[cfg(test)]
fn record_top_up_peak_snapshot(
    phase: &'static str,
    coordinator: &ScreenByteAdmissionCoordinator,
    telemetry: &MacosScreenRuntimeTelemetry,
) {
    TOP_UP_PEAK_SNAPSHOTS.with(|snapshots| {
        snapshots.borrow_mut().push((
            phase,
            coordinator.snapshot().reserved_bytes(),
            telemetry.admitted_native_bytes.load(Ordering::Acquire),
        ));
    });
}

pub(super) struct MacosSurfaceAdmissionToken {
    pool: Mutex<Option<Weak<MacosSurfacePoolInner>>>,
    telemetry: Arc<MacosScreenRuntimeTelemetry>,
    iosurface_id: u32,
    allocation_bytes: u64,
    admitted_bytes: AtomicU64,
    generation: u64,
    lease: ScreenByteLease,
}

impl MacosSurfacePool {
    pub(super) fn reserve(
        coordinator: &ScreenByteAdmissionCoordinator,
        telemetry: Arc<MacosScreenRuntimeTelemetry>,
        conservative_surface_bytes: u64,
        native_metadata_bytes: u64,
    ) -> Result<Self, MacosCaptureError> {
        let queue_depth = u64::try_from(MACOS_STREAM_QUEUE_DEPTH)
            .map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
        let tracking_bytes = pool_tracking_bytes()?;
        let metadata_bytes = native_metadata_bytes
            .checked_add(tracking_bytes)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let initial_surface_bytes = conservative_surface_bytes
            .checked_add(live_surface_tracking_bytes()?)
            .and_then(|bytes| bytes.checked_mul(queue_depth))
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let total_bytes = metadata_bytes
            .checked_add(initial_surface_bytes)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let mut reservation = coordinator
            .try_acquire(total_bytes)
            .map_err(map_admission_error)?;
        let metadata_lease = reservation
            .split_off(metadata_bytes)
            .expect("metadata is a checked subset of the pool quote")
            .freeze();
        telemetry
            .admitted_native_bytes
            .fetch_add(total_bytes, Ordering::AcqRel);
        Ok(Self {
            inner: Arc::new(MacosSurfacePoolInner {
                coordinator: coordinator.clone(),
                telemetry,
                metadata_lease,
                state: Mutex::new(MacosSurfacePoolState {
                    initial_surface_reserve: reservation,
                    live: None,
                    next_generation: 1,
                }),
            }),
        })
    }

    pub(super) fn observe(
        &self,
        iosurface_id: u32,
        allocation_bytes: u64,
    ) -> Result<Arc<MacosSurfaceAdmissionToken>, MacosCaptureError> {
        if iosurface_id == 0 || allocation_bytes == 0 {
            return Err(MacosCaptureError::InvalidSurface);
        }

        let mut state = lock(&self.inner.state);
        let mut current = state.live.as_deref();
        let mut stale_generation = None;
        while let Some(surface) = current {
            if surface.iosurface_id == iosurface_id {
                if let Some(token) = surface.token.upgrade() {
                    if token.allocation_bytes != allocation_bytes {
                        return Err(MacosCaptureError::InvalidSurface);
                    }
                    return Ok(token);
                }
                stale_generation = Some(surface.generation);
                break;
            }
            current = surface.next.as_deref();
        }
        if let Some(generation) = stale_generation {
            remove_live_surface(&mut state.live, iosurface_id, generation);
        }

        let generation = state.next_generation;
        state.next_generation = state
            .next_generation
            .checked_add(1)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let admitted_bytes = allocation_bytes
            .checked_add(live_surface_tracking_bytes()?)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let lease = acquire_surface_lease(
            &self.inner.coordinator,
            &self.inner.telemetry,
            &mut state.initial_surface_reserve,
            admitted_bytes,
        )?;
        let token = Arc::new(MacosSurfaceAdmissionToken {
            pool: Mutex::new(Some(Arc::downgrade(&self.inner))),
            telemetry: Arc::clone(&self.inner.telemetry),
            iosurface_id,
            allocation_bytes,
            admitted_bytes: AtomicU64::new(admitted_bytes),
            generation,
            lease,
        });
        let next = state.live.take();
        state.live = Some(Box::new(LiveSurface {
            iosurface_id,
            generation,
            token: Arc::downgrade(&token),
            next,
        }));
        Ok(token)
    }
}

impl Drop for MacosSurfacePoolInner {
    fn drop(&mut self) {
        let mut state = lock(&self.state);
        let mut live = state.live.take();
        while let Some(mut surface) = live {
            let next = surface.next.take();
            let token = surface.token.upgrade();
            if let Some(token) = &token {
                token.detach_from_pool();
            }
            drop(surface);
            if let Some(token) = token {
                token.release_index_tracking();
            }
            live = next;
        }
        let remaining_surface_reserve = state.initial_surface_reserve.bytes();
        let pool_bytes = self
            .metadata_lease
            .bytes()
            .checked_add(remaining_surface_reserve)
            .expect("pool reservation bytes were checked during construction");
        self.telemetry
            .admitted_native_bytes
            .fetch_sub(pool_bytes, Ordering::AcqRel);
    }
}

impl Drop for MacosSurfaceAdmissionToken {
    fn drop(&mut self) {
        let pool = lock(&self.pool).take().and_then(|pool| pool.upgrade());
        if let Some(pool) = pool {
            remove_live_surface(
                &mut lock(&pool.state).live,
                self.iosurface_id,
                self.generation,
            );
        }
        self.telemetry.admitted_native_bytes.fetch_sub(
            self.admitted_bytes.load(Ordering::Acquire),
            Ordering::AcqRel,
        );
    }
}

impl MacosSurfaceAdmissionToken {
    fn detach_from_pool(&self) {
        lock(&self.pool).take();
    }

    fn release_index_tracking(&self) {
        #[cfg(test)]
        record_pool_drop_event("index_tracking_release");

        let exact_bytes = self
            .allocation_bytes
            .checked_add(surface_token_tracking_bytes().expect("token tracking quote fits"))
            .expect("surface admission bytes were checked during observation");
        self.lease
            .try_reconcile_exact(exact_bytes)
            .expect("dropping live-index tracking only reduces admission");
        let previous = self.admitted_bytes.swap(exact_bytes, Ordering::AcqRel);
        self.telemetry
            .admitted_native_bytes
            .fetch_sub(previous - exact_bytes, Ordering::AcqRel);
    }
}

fn remove_live_surface(live: &mut Option<Box<LiveSurface>>, iosurface_id: u32, generation: u64) {
    let mut cursor = live;
    while let Some(mut surface) = cursor.take() {
        if surface.iosurface_id == iosurface_id && surface.generation == generation {
            *cursor = surface.next.take();
            return;
        }
        *cursor = Some(surface);
        cursor = &mut cursor
            .as_mut()
            .expect("the current surface was restored")
            .next;
    }
}

fn pool_tracking_bytes() -> Result<u64, MacosCaptureError> {
    arc_allocation_bytes::<MacosSurfacePoolInner>()?
        .checked_add(
            screen_byte_lease_allocation_bytes()?
                .checked_mul(2)
                .ok_or(MacosCaptureError::ArithmeticOverflow)?,
        )
        .ok_or(MacosCaptureError::ArithmeticOverflow)
}

fn live_surface_tracking_bytes() -> Result<u64, MacosCaptureError> {
    u64::try_from(size_of::<LiveSurface>())
        .map_err(|_| MacosCaptureError::ArithmeticOverflow)?
        .checked_add(surface_token_tracking_bytes()?)
        .ok_or(MacosCaptureError::ArithmeticOverflow)
}

fn surface_token_tracking_bytes() -> Result<u64, MacosCaptureError> {
    arc_allocation_bytes::<MacosSurfaceAdmissionToken>()?
        .checked_add(screen_byte_lease_allocation_bytes()?)
        .ok_or(MacosCaptureError::ArithmeticOverflow)
}

fn arc_allocation_bytes<T>() -> Result<u64, MacosCaptureError> {
    arc_allocation_bytes_for_layout(Layout::new::<T>())
}

fn screen_byte_lease_allocation_bytes() -> Result<u64, MacosCaptureError> {
    let (payload, _) = Layout::new::<Arc<()>>()
        .extend(Layout::new::<AtomicU64>())
        .map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    arc_allocation_bytes_for_layout(payload.pad_to_align())
}

fn arc_allocation_bytes_for_layout(payload: Layout) -> Result<u64, MacosCaptureError> {
    let header =
        Layout::array::<AtomicUsize>(2).map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    let (allocation, _) = header
        .extend(payload)
        .map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    u64::try_from(allocation.pad_to_align().size())
        .map_err(|_| MacosCaptureError::ArithmeticOverflow)
}

fn acquire_surface_lease(
    coordinator: &ScreenByteAdmissionCoordinator,
    telemetry: &MacosScreenRuntimeTelemetry,
    initial_reserve: &mut ScreenByteReservation,
    allocation_bytes: u64,
) -> Result<ScreenByteLease, MacosCaptureError> {
    let reserved_bytes = initial_reserve.bytes().min(allocation_bytes);
    let added_bytes = allocation_bytes - reserved_bytes;
    let temporary_lease_bytes = if added_bytes == 0 {
        0
    } else {
        screen_byte_lease_allocation_bytes()?
    };
    let peak_top_up_bytes = added_bytes
        .checked_add(temporary_lease_bytes)
        .ok_or(MacosCaptureError::ArithmeticOverflow)?;
    let top_up = if added_bytes == 0 {
        None
    } else {
        if telemetry
            .admitted_native_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |admitted| {
                admitted.checked_add(peak_top_up_bytes)
            })
            .is_err()
        {
            return Err(MacosCaptureError::ArithmeticOverflow);
        }
        #[cfg(test)]
        record_top_up_event("peak_precharged");
        let top_up = match coordinator.try_acquire(peak_top_up_bytes) {
            Ok(top_up) => {
                #[cfg(test)]
                record_top_up_event("temporary_lease_admitted");
                top_up
            }
            Err(error) => {
                #[cfg(test)]
                record_top_up_event("coordinator_rejected");
                telemetry
                    .admitted_native_bytes
                    .fetch_sub(peak_top_up_bytes, Ordering::AcqRel);
                #[cfg(test)]
                record_top_up_event("peak_released");
                return Err(map_admission_error(error));
            }
        };
        #[cfg(test)]
        record_top_up_peak_snapshot("before_final_lease_split", coordinator, telemetry);
        Some(top_up)
    };
    let mut reservation = initial_reserve
        .split_off(reserved_bytes)
        .expect("surface reserve split is bounded by its current bytes");
    if let Some(top_up) = top_up {
        #[cfg(test)]
        record_top_up_event("final_lease_split");
        #[cfg(test)]
        record_top_up_peak_snapshot("after_final_lease_split", coordinator, telemetry);
        reservation
            .absorb(top_up)
            .expect("surface top-up shares the pool admission coordinator");
        #[cfg(test)]
        record_top_up_event("temporary_lease_freed");
        reservation
            .reconcile_down(allocation_bytes)
            .expect("the temporary top-up lease is freed before its admission is released");
        telemetry
            .admitted_native_bytes
            .fetch_sub(temporary_lease_bytes, Ordering::AcqRel);
        #[cfg(test)]
        record_top_up_event("temporary_peak_released");
        #[cfg(test)]
        record_top_up_peak_snapshot("steady_state", coordinator, telemetry);
    }
    Ok(reservation.freeze())
}

fn map_admission_error(error: ScreenByteAdmissionError) -> MacosCaptureError {
    let (requested_bytes, available_bytes) = match error {
        ScreenByteAdmissionError::CapacityExceeded {
            requested_bytes,
            available_bytes,
        } => (requested_bytes, available_bytes),
        ScreenByteAdmissionError::CapacityShrinkRejected {
            requested_capacity,
            reserved_bytes,
        } => (reserved_bytes, requested_capacity),
        ScreenByteAdmissionError::RevisionExhausted => (u64::MAX, 0),
    };
    MacosCaptureError::ScreenResourceExhausted {
        requested_bytes,
        available_bytes,
    }
}

#[cfg(test)]
mod tests;
