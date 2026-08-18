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
mod tests {
    use std::alloc::Layout;
    use std::mem::size_of;
    use std::sync::{Arc, Barrier};

    use super::*;
    use crate::input::screen::ScreenAdmissionCapacity;

    fn pool(
        coordinator: &ScreenByteAdmissionCoordinator,
        telemetry: Arc<MacosScreenRuntimeTelemetry>,
    ) -> MacosSurfacePool {
        MacosSurfacePool::reserve(coordinator, telemetry, 100, 32)
            .expect("initial queue reserve should fit")
    }

    fn metadata_bytes(pool: &MacosSurfacePool) -> u64 {
        pool.inner.metadata_lease.bytes()
    }

    fn initial_reserve_bytes(pool: &MacosSurfacePool) -> u64 {
        lock(&pool.inner.state).initial_surface_reserve.bytes()
    }

    fn live_surface_count(pool: &MacosSurfacePool) -> usize {
        let state = lock(&pool.inner.state);
        let mut count = 0;
        let mut current = state.live.as_deref();
        while let Some(surface) = current {
            count += 1;
            current = surface.next.as_deref();
        }
        count
    }

    fn take_pool_drop_events() -> Vec<&'static str> {
        POOL_DROP_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
    }

    fn take_top_up_events() -> Vec<&'static str> {
        TOP_UP_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
    }

    fn take_top_up_peak_snapshots() -> Vec<(&'static str, u64, u64)> {
        TOP_UP_PEAK_SNAPSHOTS.with(|snapshots| std::mem::take(&mut *snapshots.borrow_mut()))
    }

    fn arc_allocation_bytes_oracle<T>() -> u64 {
        arc_allocation_bytes_for_layout_oracle(Layout::new::<T>())
    }

    fn lease_allocation_bytes_oracle() -> u64 {
        let (payload, _) = Layout::new::<Arc<()>>()
            .extend(Layout::new::<AtomicU64>())
            .expect("lease payload layout should build");
        arc_allocation_bytes_for_layout_oracle(payload.pad_to_align())
    }

    fn arc_allocation_bytes_for_layout_oracle(payload: Layout) -> u64 {
        let header = Layout::array::<AtomicUsize>(2).expect("Arc header layout should build");
        let (allocation, _) = header
            .extend(payload)
            .expect("Arc allocation layout should build");
        u64::try_from(allocation.pad_to_align().size()).expect("Arc allocation size fits u64")
    }

    fn pool_tracking_bytes_oracle() -> u64 {
        arc_allocation_bytes_oracle::<MacosSurfacePoolInner>() + 2 * lease_allocation_bytes_oracle()
    }

    fn surface_token_tracking_bytes_oracle() -> u64 {
        arc_allocation_bytes_oracle::<MacosSurfaceAdmissionToken>()
            + lease_allocation_bytes_oracle()
    }

    fn live_surface_tracking_bytes_oracle() -> u64 {
        u64::try_from(size_of::<LiveSurface>()).expect("live index allocation size fits u64")
            + surface_token_tracking_bytes_oracle()
    }

    #[test]
    fn byte_quotes_enumerate_every_pool_and_live_heap_allocation() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
        let pool = pool(&coordinator, Arc::clone(&telemetry));
        let pool_bytes = 32 + pool_tracking_bytes_oracle();
        let live_tracking_bytes = live_surface_tracking_bytes_oracle();

        assert_eq!(metadata_bytes(&pool), pool_bytes);
        assert_eq!(
            initial_reserve_bytes(&pool),
            u64::try_from(MACOS_STREAM_QUEUE_DEPTH).expect("queue depth fits u64")
                * (100 + live_tracking_bytes)
        );

        let token = pool.observe(29, 120).expect("surface should fit");
        assert_eq!(
            token.admitted_bytes.load(Ordering::Acquire),
            120 + live_tracking_bytes
        );
        assert_eq!(token.lease.bytes(), 120 + live_tracking_bytes);
        assert!(lock(&token.pool).is_some());

        drop(pool);
        let pinned_bytes = 120 + surface_token_tracking_bytes_oracle();
        assert!(lock(&token.pool).is_none());
        assert_eq!(coordinator.snapshot().reserved_bytes(), pinned_bytes);
        assert_eq!(
            telemetry.admitted_native_bytes.load(Ordering::Acquire),
            pinned_bytes
        );

        drop(token);
        assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
        assert_eq!(telemetry.admitted_native_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn pinned_generations_retain_tokens_without_retaining_pool_allocations() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
        let pinned_surface_bytes = 120 + surface_token_tracking_bytes_oracle();
        let mut tokens = Vec::new();

        for generation in 1..=9 {
            let generation_pool = pool(&coordinator, Arc::clone(&telemetry));
            let token = generation_pool
                .observe(31, 120)
                .expect("generation surface should fit");
            drop(generation_pool);

            assert!(lock(&token.pool).is_none());
            tokens.push(token);
            assert_eq!(
                coordinator.snapshot().reserved_bytes(),
                generation * pinned_surface_bytes
            );
            assert_eq!(
                telemetry.admitted_native_bytes.load(Ordering::Acquire),
                generation * pinned_surface_bytes
            );
        }

        drop(tokens);
        assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
        assert_eq!(telemetry.admitted_native_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn pool_drop_frees_live_index_before_releasing_its_tracking() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let pool = pool(
            &coordinator,
            Arc::new(MacosScreenRuntimeTelemetry::default()),
        );
        let token = pool.observe(37, 120).expect("surface should fit");
        drop(take_pool_drop_events());

        drop(pool);

        assert_eq!(
            take_pool_drop_events(),
            vec!["live_surface_drop", "index_tracking_release"]
        );
        drop(token);
    }

    #[test]
    fn ninth_historical_surface_is_admitted_after_prior_tokens_drop() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
        let pool = pool(&coordinator, Arc::clone(&telemetry));
        let metadata_bytes = metadata_bytes(&pool);

        for iosurface_id in 1..=9 {
            let token = pool
                .observe(iosurface_id, 100)
                .expect("historical identity must not consume a live slot");
            assert_eq!(live_surface_count(&pool), 1);
            drop(token);
            assert_eq!(live_surface_count(&pool), 0);
        }

        assert_eq!(initial_reserve_bytes(&pool), 0);
        assert_eq!(coordinator.snapshot().reserved_bytes(), metadata_bytes);
        assert_eq!(
            telemetry.admitted_native_bytes.load(Ordering::Acquire),
            metadata_bytes
        );
    }

    #[test]
    fn ninth_simultaneous_surface_depends_only_on_byte_capacity() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
        let pool = pool(&coordinator, Arc::clone(&telemetry));
        let metadata_bytes = metadata_bytes(&pool);
        let per_surface_bytes = 100 + live_surface_tracking_bytes_oracle();
        let tokens = (1..=9)
            .map(|iosurface_id| {
                pool.observe(iosurface_id, 100)
                    .expect("real byte capacity admits more than queue depth")
            })
            .collect::<Vec<_>>();

        assert_eq!(live_surface_count(&pool), 9);
        assert_eq!(
            coordinator.snapshot().reserved_bytes(),
            metadata_bytes + 9 * per_surface_bytes
        );
        assert_eq!(
            telemetry.admitted_native_bytes.load(Ordering::Acquire),
            metadata_bytes + 9 * per_surface_bytes
        );
        drop(tokens);
        assert_eq!(coordinator.snapshot().reserved_bytes(), metadata_bytes);
    }

    #[test]
    fn ninth_simultaneous_surface_is_rejected_when_byte_capacity_is_full() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let pool = pool(
            &coordinator,
            Arc::new(MacosScreenRuntimeTelemetry::default()),
        );
        let initial_bytes = coordinator.snapshot().reserved_bytes();
        coordinator
            .try_set_capacity(ScreenAdmissionCapacity::new(initial_bytes, initial_bytes))
            .expect("exact current capacity installs");
        let tokens = (1..=8)
            .map(|iosurface_id| {
                pool.observe(iosurface_id, 100)
                    .expect("initial queue reserve covers eight live surfaces")
            })
            .collect::<Vec<_>>();

        assert!(matches!(
            pool.observe(9, 100),
            Err(MacosCaptureError::ScreenResourceExhausted {
                requested_bytes,
                available_bytes: 0,
            }) if requested_bytes
                == 100
                    + live_surface_tracking_bytes_oracle()
                    + lease_allocation_bytes_oracle()
        ));
        assert_eq!(live_surface_count(&pool), 8);
        assert_eq!(coordinator.snapshot().reserved_bytes(), initial_bytes);
        drop(tokens);
    }

    #[test]
    fn top_up_peak_admits_the_temporary_lease_allocation() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let pool = pool(
            &coordinator,
            Arc::new(MacosScreenRuntimeTelemetry::default()),
        );
        let tokens = (1..=8)
            .map(|iosurface_id| {
                pool.observe(iosurface_id, 100)
                    .expect("initial queue reserve covers eight live surfaces")
            })
            .collect::<Vec<_>>();
        let reserved_before = coordinator.snapshot().reserved_bytes();
        let final_surface_bytes = 100 + live_surface_tracking_bytes_oracle();
        let temporary_lease_bytes = lease_allocation_bytes_oracle();
        coordinator
            .try_set_capacity(ScreenAdmissionCapacity::new(
                reserved_before + final_surface_bytes,
                reserved_before + final_surface_bytes,
            ))
            .expect("capacity for only the final surface installs");
        drop(take_top_up_events());
        drop(take_top_up_peak_snapshots());

        assert!(matches!(
            pool.observe(9, 100),
            Err(MacosCaptureError::ScreenResourceExhausted {
                requested_bytes,
                available_bytes,
            }) if requested_bytes == final_surface_bytes + temporary_lease_bytes
                && available_bytes == final_surface_bytes
        ));
        assert_eq!(coordinator.snapshot().reserved_bytes(), reserved_before);
        assert_eq!(
            pool.inner
                .telemetry
                .admitted_native_bytes
                .load(Ordering::Acquire),
            reserved_before
        );
        assert_eq!(
            take_top_up_events(),
            vec!["peak_precharged", "coordinator_rejected", "peak_released"]
        );
        assert!(take_top_up_peak_snapshots().is_empty());

        coordinator
            .try_set_capacity(ScreenAdmissionCapacity::new(
                reserved_before + final_surface_bytes + temporary_lease_bytes,
                reserved_before + final_surface_bytes + temporary_lease_bytes,
            ))
            .expect("capacity for the exact top-up peak installs");
        let ninth = pool
            .observe(9, 100)
            .expect("exact temporary peak capacity admits the surface");
        assert_eq!(
            coordinator.snapshot().reserved_bytes(),
            reserved_before + final_surface_bytes
        );
        assert_eq!(
            take_top_up_peak_snapshots(),
            vec![
                (
                    "before_final_lease_split",
                    reserved_before + final_surface_bytes + temporary_lease_bytes,
                    reserved_before + final_surface_bytes + temporary_lease_bytes,
                ),
                (
                    "after_final_lease_split",
                    reserved_before + final_surface_bytes + temporary_lease_bytes,
                    reserved_before + final_surface_bytes + temporary_lease_bytes,
                ),
                (
                    "steady_state",
                    reserved_before + final_surface_bytes,
                    reserved_before + final_surface_bytes,
                ),
            ]
        );
        assert_eq!(
            take_top_up_events(),
            vec![
                "peak_precharged",
                "temporary_lease_admitted",
                "final_lease_split",
                "temporary_lease_freed",
                "temporary_peak_released",
            ]
        );
        assert_eq!(
            pool.inner
                .telemetry
                .admitted_native_bytes
                .load(Ordering::Acquire),
            reserved_before + final_surface_bytes
        );

        drop(ninth);
        drop(tokens);
    }

    #[test]
    fn rejected_top_up_restores_the_unconsumed_initial_reserve() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
        let pool = pool(&coordinator, Arc::clone(&telemetry));
        let initial_bytes = coordinator.snapshot().reserved_bytes();
        let initial_surface_reserve = initial_reserve_bytes(&pool);
        coordinator
            .try_set_capacity(ScreenAdmissionCapacity::new(initial_bytes, initial_bytes))
            .expect("exact current capacity installs");

        assert!(matches!(
            pool.observe(1, initial_surface_reserve),
            Err(MacosCaptureError::ScreenResourceExhausted { .. })
        ));
        assert_eq!(live_surface_count(&pool), 0);
        assert_eq!(initial_reserve_bytes(&pool), initial_surface_reserve);
        assert_eq!(coordinator.snapshot().reserved_bytes(), initial_bytes);
        assert_eq!(
            telemetry.admitted_native_bytes.load(Ordering::Acquire),
            initial_bytes
        );
    }

    #[test]
    fn repeated_live_observations_share_one_token_and_release_exactly_once() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
        let pool = pool(&coordinator, Arc::clone(&telemetry));
        let reserved_before = coordinator.snapshot().reserved_bytes();
        let tracking_bytes = live_surface_tracking_bytes_oracle();
        let first = pool.observe(7, 120).expect("first observation fits");
        let repeated = pool.observe(7, 120).expect("live reuse fits");

        assert!(Arc::ptr_eq(&first, &repeated));
        assert_eq!(live_surface_count(&pool), 1);
        assert_eq!(
            initial_reserve_bytes(&pool),
            8 * (100 + tracking_bytes) - (120 + tracking_bytes)
        );
        assert_eq!(coordinator.snapshot().reserved_bytes(), reserved_before);
        drop(first);
        assert_eq!(coordinator.snapshot().reserved_bytes(), reserved_before);
        drop(repeated);
        assert_eq!(
            coordinator.snapshot().reserved_bytes(),
            reserved_before - (120 + tracking_bytes)
        );
        assert_eq!(live_surface_count(&pool), 0);
    }

    #[test]
    fn concurrent_live_observations_share_one_token() {
        const OBSERVERS: usize = 16;

        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
        let pool = pool(&coordinator, telemetry);
        let tracking_bytes = live_surface_tracking_bytes_oracle();
        let barrier = Arc::new(Barrier::new(OBSERVERS));
        let tokens = std::thread::scope(|scope| {
            let handles = (0..OBSERVERS)
                .map(|_| {
                    let pool = pool.clone();
                    let barrier = Arc::clone(&barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        pool.observe(11, 144).expect("shared observation fits")
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("observer thread succeeds"))
                .collect::<Vec<_>>()
        });

        assert!(tokens.iter().all(|token| Arc::ptr_eq(&tokens[0], token)));
        assert_eq!(live_surface_count(&pool), 1);
        assert_eq!(
            initial_reserve_bytes(&pool),
            8 * (100 + tracking_bytes) - (144 + tracking_bytes)
        );
    }

    #[test]
    fn live_allocation_conflicts_fail_closed_and_recycled_ids_admit_fresh() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
        let pool = pool(&coordinator, telemetry);
        let first = pool.observe(19, 120).expect("first observation fits");
        let reserved = coordinator.snapshot().reserved_bytes();

        assert!(matches!(
            pool.observe(19, 121),
            Err(MacosCaptureError::InvalidSurface)
        ));
        assert_eq!(coordinator.snapshot().reserved_bytes(), reserved);
        drop(first);

        let recycled = pool
            .observe(19, 121)
            .expect("fully released identity is admitted fresh");
        assert_eq!(recycled.allocation_bytes, 121);
        assert_eq!(live_surface_count(&pool), 1);
    }

    #[test]
    fn pinned_old_generation_retains_only_its_live_surface_bytes() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(10_000, 10_000));
        let pinned_bytes = 120 + surface_token_tracking_bytes_oracle();
        let old_telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
        let old_pool = pool(&coordinator, Arc::clone(&old_telemetry));
        let pinned = old_pool
            .observe(23, 120)
            .expect("old generation surface fits");
        drop(old_pool);

        assert_eq!(coordinator.snapshot().reserved_bytes(), pinned_bytes);
        assert_eq!(
            old_telemetry.admitted_native_bytes.load(Ordering::Acquire),
            pinned_bytes
        );

        let candidate_telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
        let candidate = pool(&coordinator, Arc::clone(&candidate_telemetry));
        let candidate_bytes = coordinator.snapshot().reserved_bytes() - pinned_bytes;
        drop(pinned);
        assert_eq!(coordinator.snapshot().reserved_bytes(), candidate_bytes);
        assert_eq!(
            old_telemetry.admitted_native_bytes.load(Ordering::Acquire),
            0
        );
        drop(candidate);
        assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
    }
}
