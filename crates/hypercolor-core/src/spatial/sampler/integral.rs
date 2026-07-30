use std::ops::Deref;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
#[cfg(feature = "spatial-workspace-test-hooks")]
use std::time::Duration;

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas};

use super::lut::decode_srgb_byte;
use crate::spatial::{PreparedAreaSample, SpatialSamplingCapacity, SpatialSamplingError};

#[derive(Debug)]
pub(crate) struct AreaWorkspacePool {
    state: Mutex<AreaWorkspacePoolState>,
    allocation_ready: Condvar,
    capacity: SpatialSamplingCapacity,
    #[cfg(feature = "spatial-workspace-test-hooks")]
    test_hook: Mutex<Option<Arc<SpatialWorkspaceAllocationTestHook>>>,
}

#[derive(Debug)]
struct AreaWorkspacePoolState {
    available: Vec<SummedAreaWorkspace>,
    resident_descriptors: Vec<WorkspaceDescriptor>,
    allocating_descriptors: Vec<WorkspaceDescriptor>,
    resident_bytes: usize,
    reserved_bytes: usize,
    leased: usize,
    reserved: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkspaceDescriptor {
    width: u32,
    height: u32,
}

#[cfg(feature = "spatial-workspace-test-hooks")]
#[derive(Debug)]
pub struct SpatialWorkspaceAllocationTestHook {
    state: Mutex<SpatialWorkspaceAllocationTestState>,
    changed: Condvar,
}

#[cfg(feature = "spatial-workspace-test-hooks")]
#[derive(Debug)]
struct SpatialWorkspaceAllocationTestState {
    attempts: usize,
    waiters: usize,
    release_first: bool,
    fail_first: bool,
}

#[cfg(feature = "spatial-workspace-test-hooks")]
impl SpatialWorkspaceAllocationTestHook {
    /// Create a hook that gates the first post-reservation allocation attempt.
    #[must_use]
    pub fn new(fail_first: bool) -> Self {
        Self {
            state: Mutex::new(SpatialWorkspaceAllocationTestState {
                attempts: 0,
                waiters: 0,
                release_first: false,
                fail_first,
            }),
            changed: Condvar::new(),
        }
    }

    /// Wait until the first allocation has reserved capacity and reached the gate.
    #[must_use]
    pub fn wait_for_first_reservation(&self, timeout: Duration) -> bool {
        self.wait_for(timeout, |state| state.attempts >= 1)
    }

    /// Wait until at least `count` peers are waiting on the in-flight descriptor.
    #[must_use]
    pub fn wait_for_waiters(&self, count: usize, timeout: Duration) -> bool {
        self.wait_for(timeout, |state| state.waiters >= count)
    }

    /// Release the first allocation attempt from its deterministic gate.
    pub fn release_first_allocation(&self) {
        let mut state = self.lock_state();
        state.release_first = true;
        drop(state);
        self.changed.notify_all();
    }

    /// Return the number of allocation attempts observed after reservation.
    #[must_use]
    pub fn allocation_attempts(&self) -> usize {
        self.lock_state().attempts
    }

    /// Return the number of descriptor waits observed by the pool.
    #[must_use]
    pub fn waiter_count(&self) -> usize {
        self.lock_state().waiters
    }

    fn before_allocation(&self) -> bool {
        let mut state = self.lock_state();
        state.attempts += 1;
        let first = state.attempts == 1;
        self.changed.notify_all();
        while first && !state.release_first {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        first && state.fail_first
    }

    fn record_waiter(&self) {
        let mut state = self.lock_state();
        state.waiters += 1;
        drop(state);
        self.changed.notify_all();
    }

    fn wait_for(
        &self,
        timeout: Duration,
        predicate: impl Fn(&SpatialWorkspaceAllocationTestState) -> bool,
    ) -> bool {
        let state = self.lock_state();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |state| !predicate(state))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        predicate(&state)
    }

    fn lock_state(&self) -> MutexGuard<'_, SpatialWorkspaceAllocationTestState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl AreaWorkspacePool {
    pub(crate) fn try_new(
        width: u32,
        height: u32,
        capacity: SpatialSamplingCapacity,
    ) -> Result<Arc<Self>, SpatialSamplingError> {
        let geometry = WorkspaceGeometry::try_new(width, height)?;
        validate_aggregate_capacity(0, 0, geometry, capacity)?;
        let workspace = SummedAreaWorkspace::try_new(geometry)?;
        let mut available = Vec::new();
        available.try_reserve_exact(1).map_err(|_| {
            SpatialSamplingError::AreaWorkspaceAllocation {
                width,
                height,
                entry_count: workspace.entry_count(),
            }
        })?;
        available.push(workspace);
        let descriptor = geometry.descriptor();
        let mut resident_descriptors = Vec::new();
        resident_descriptors.try_reserve_exact(1).map_err(|_| {
            SpatialSamplingError::AreaWorkspaceAllocation {
                width,
                height,
                entry_count: geometry.entry_count,
            }
        })?;
        resident_descriptors.push(descriptor);
        Ok(Arc::new(Self {
            state: Mutex::new(AreaWorkspacePoolState {
                available,
                resident_descriptors,
                allocating_descriptors: Vec::new(),
                resident_bytes: geometry.byte_len,
                reserved_bytes: 0,
                leased: 0,
                reserved: 0,
            }),
            allocation_ready: Condvar::new(),
            capacity,
            #[cfg(feature = "spatial-workspace-test-hooks")]
            test_hook: Mutex::new(None),
        }))
    }

    pub(crate) fn try_checkout(
        self: &Arc<Self>,
        canvas: &Canvas,
    ) -> Result<AreaWorkspaceLease, SpatialSamplingError> {
        let width = canvas.width();
        let height = canvas.height();
        let geometry = WorkspaceGeometry::try_new(width, height)?;
        let descriptor = geometry.descriptor();

        loop {
            let mut state = self.lock_state();
            if let Some(index) = state
                .available
                .iter()
                .position(|workspace| workspace.matches(width, height))
            {
                let mut workspace = state.available.swap_remove(index);
                state.leased += 1;
                drop(state);
                workspace.rebuild(canvas);
                return Ok(AreaWorkspaceLease {
                    pool: Arc::clone(self),
                    workspace: Some(workspace),
                });
            }
            if state.allocating_descriptors.contains(&descriptor) {
                drop(self.wait_for_allocation(state));
                continue;
            }
            let replacement = reserve_allocation(&mut state, geometry, self.capacity)?;
            drop(state);

            let allocation =
                allocate_workspace(replacement, geometry, self.inject_allocation_failure());
            let mut state = self.lock_state();
            finish_reservation(&mut state, descriptor, geometry);
            let mut workspace = match allocation {
                Ok(workspace) => workspace,
                Err(failure) => {
                    restore_replacement(&mut state, failure.replacement);
                    drop(state);
                    self.allocation_ready.notify_all();
                    return Err(failure.error);
                }
            };
            state.resident_bytes += geometry.byte_len;
            state.resident_descriptors.push(descriptor);
            state.leased += 1;
            drop(state);
            self.allocation_ready.notify_all();
            workspace.rebuild(canvas);
            return Ok(AreaWorkspaceLease {
                pool: Arc::clone(self),
                workspace: Some(workspace),
            });
        }
    }

    pub(crate) fn try_prepare(
        self: &Arc<Self>,
        width: u32,
        height: u32,
    ) -> Result<(), SpatialSamplingError> {
        let geometry = WorkspaceGeometry::try_new(width, height)?;
        let descriptor = geometry.descriptor();

        loop {
            let mut state = self.lock_state();
            if state.resident_descriptors.contains(&descriptor) {
                return Ok(());
            }
            if state.allocating_descriptors.contains(&descriptor) {
                drop(self.wait_for_allocation(state));
                continue;
            }
            let replacement = reserve_allocation(&mut state, geometry, self.capacity)?;
            drop(state);

            let allocation =
                allocate_workspace(replacement, geometry, self.inject_allocation_failure());
            let mut state = self.lock_state();
            finish_reservation(&mut state, descriptor, geometry);
            match allocation {
                Ok(workspace) => {
                    state.resident_bytes += geometry.byte_len;
                    state.resident_descriptors.push(descriptor);
                    state.available.push(workspace);
                    drop(state);
                    self.allocation_ready.notify_all();
                    return Ok(());
                }
                Err(failure) => {
                    restore_replacement(&mut state, failure.replacement);
                    drop(state);
                    self.allocation_ready.notify_all();
                    return Err(failure.error);
                }
            }
        }
    }

    pub(crate) fn usage(&self) -> (usize, usize, usize, usize) {
        let state = self.lock_state();
        (
            state.resident_descriptors.len(),
            state.resident_bytes,
            state.reserved,
            state.reserved_bytes,
        )
    }

    #[cfg(feature = "spatial-workspace-test-hooks")]
    pub(crate) fn install_test_hook(&self, hook: Arc<SpatialWorkspaceAllocationTestHook>) {
        *self
            .test_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    fn checkin(&self, workspace: SummedAreaWorkspace) {
        let mut state = self.lock_state();
        state.leased -= 1;
        state.available.push(workspace);
    }

    fn wait_for_allocation<'a>(
        &self,
        state: MutexGuard<'a, AreaWorkspacePoolState>,
    ) -> MutexGuard<'a, AreaWorkspacePoolState> {
        #[cfg(feature = "spatial-workspace-test-hooks")]
        if let Some(hook) = self.test_hook() {
            hook.record_waiter();
        }
        self.allocation_ready
            .wait(state)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_state(&self) -> MutexGuard<'_, AreaWorkspacePoolState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn inject_allocation_failure(&self) -> bool {
        #[cfg(feature = "spatial-workspace-test-hooks")]
        if let Some(hook) = self.test_hook() {
            return hook.before_allocation();
        }
        false
    }

    #[cfg(feature = "spatial-workspace-test-hooks")]
    fn test_hook(&self) -> Option<Arc<SpatialWorkspaceAllocationTestHook>> {
        self.test_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

fn reserve_allocation(
    state: &mut AreaWorkspacePoolState,
    geometry: WorkspaceGeometry,
    capacity: SpatialSamplingCapacity,
) -> Result<Option<EvictedWorkspace>, SpatialSamplingError> {
    let replacement_geometry = state.available.last().map(SummedAreaWorkspace::geometry);
    let replaced_bytes = replacement_geometry.map_or(0, |candidate| candidate.byte_len);
    validate_aggregate_capacity(
        state.resident_bytes - replaced_bytes,
        state.reserved_bytes,
        geometry,
        capacity,
    )?;
    let adds_workspace = usize::from(replacement_geometry.is_none());
    let required_slots = state
        .available
        .len()
        .saturating_add(state.leased)
        .saturating_add(state.reserved)
        .saturating_add(adds_workspace);
    if state.available.capacity() < required_slots {
        state
            .available
            .try_reserve_exact(required_slots - state.available.len())
            .map_err(|_| allocation_error(geometry))?;
    }
    let required_descriptors = state
        .resident_descriptors
        .len()
        .saturating_add(state.reserved)
        .saturating_add(adds_workspace);
    if state.resident_descriptors.capacity() < required_descriptors {
        state
            .resident_descriptors
            .try_reserve_exact(required_descriptors - state.resident_descriptors.len())
            .map_err(|_| allocation_error(geometry))?;
    }
    state
        .allocating_descriptors
        .try_reserve_exact(1)
        .map_err(|_| allocation_error(geometry))?;

    let replacement = state.available.pop().map(|workspace| {
        let old_geometry = workspace.geometry();
        let descriptor_index = state
            .resident_descriptors
            .iter()
            .position(|candidate| *candidate == old_geometry.descriptor())
            .expect("idle workspace must retain a resident descriptor");
        state.resident_descriptors.swap_remove(descriptor_index);
        state.resident_bytes -= old_geometry.byte_len;
        EvictedWorkspace {
            workspace,
            geometry: old_geometry,
        }
    });
    state.reserved += 1;
    state.reserved_bytes += geometry.byte_len;
    state.allocating_descriptors.push(geometry.descriptor());
    Ok(replacement)
}

#[derive(Debug)]
struct EvictedWorkspace {
    workspace: SummedAreaWorkspace,
    geometry: WorkspaceGeometry,
}

#[derive(Debug)]
struct WorkspaceAllocationFailure {
    error: SpatialSamplingError,
    replacement: Option<EvictedWorkspace>,
}

fn allocate_workspace(
    mut replacement: Option<EvictedWorkspace>,
    geometry: WorkspaceGeometry,
    inject_failure: bool,
) -> Result<SummedAreaWorkspace, WorkspaceAllocationFailure> {
    if inject_failure {
        return Err(WorkspaceAllocationFailure {
            error: allocation_error(geometry),
            replacement,
        });
    }
    if let Some(candidate) = replacement.as_mut() {
        if let Err(error) = candidate.workspace.try_resize(geometry) {
            return Err(WorkspaceAllocationFailure { error, replacement });
        }
        return Ok(replacement
            .take()
            .expect("replacement must remain present after resize")
            .workspace);
    }
    SummedAreaWorkspace::try_new(geometry).map_err(|error| WorkspaceAllocationFailure {
        error,
        replacement: None,
    })
}

fn restore_replacement(state: &mut AreaWorkspacePoolState, replacement: Option<EvictedWorkspace>) {
    if let Some(replacement) = replacement {
        state.resident_bytes += replacement.geometry.byte_len;
        state
            .resident_descriptors
            .push(replacement.geometry.descriptor());
        state.available.push(replacement.workspace);
    }
}

fn finish_reservation(
    state: &mut AreaWorkspacePoolState,
    descriptor: WorkspaceDescriptor,
    geometry: WorkspaceGeometry,
) {
    let index = state
        .allocating_descriptors
        .iter()
        .position(|candidate| *candidate == descriptor)
        .expect("active allocation must retain its descriptor reservation");
    state.allocating_descriptors.swap_remove(index);
    state.reserved -= 1;
    state.reserved_bytes -= geometry.byte_len;
}

fn validate_aggregate_capacity(
    resident_bytes: usize,
    reserved_bytes: usize,
    geometry: WorkspaceGeometry,
    capacity: SpatialSamplingCapacity,
) -> Result<(), SpatialSamplingError> {
    let capacity_bytes = capacity.max_area_workspace_bytes();
    let required_bytes = resident_bytes
        .checked_add(reserved_bytes)
        .and_then(|bytes| bytes.checked_add(geometry.byte_len))
        .ok_or(SpatialSamplingError::AreaWorkspaceCapacityExceeded {
            width: geometry.width,
            height: geometry.height,
            required_bytes: usize::MAX,
            capacity_bytes,
        })?;
    if required_bytes > capacity_bytes {
        return Err(SpatialSamplingError::AreaWorkspaceCapacityExceeded {
            width: geometry.width,
            height: geometry.height,
            required_bytes,
            capacity_bytes,
        });
    }
    Ok(())
}

fn allocation_error(geometry: WorkspaceGeometry) -> SpatialSamplingError {
    SpatialSamplingError::AreaWorkspaceAllocation {
        width: geometry.width,
        height: geometry.height,
        entry_count: geometry.entry_count,
    }
}

#[derive(Debug)]
pub(crate) struct AreaWorkspaceLease {
    pool: Arc<AreaWorkspacePool>,
    workspace: Option<SummedAreaWorkspace>,
}

impl Deref for AreaWorkspaceLease {
    type Target = SummedAreaWorkspace;

    fn deref(&self) -> &Self::Target {
        self.workspace
            .as_ref()
            .expect("area workspace lease must own its workspace")
    }
}

impl Drop for AreaWorkspaceLease {
    fn drop(&mut self) {
        if let Some(workspace) = self.workspace.take() {
            self.pool.checkin(workspace);
        }
    }
}

#[derive(Debug)]
pub(crate) struct SummedAreaWorkspace {
    width: u32,
    height: u32,
    stride: usize,
    sums: Vec<[u64; 3]>,
}

impl SummedAreaWorkspace {
    fn try_new(geometry: WorkspaceGeometry) -> Result<Self, SpatialSamplingError> {
        let sums = allocate_sums(geometry)?;
        Ok(Self {
            width: geometry.width,
            height: geometry.height,
            stride: geometry.stride,
            sums,
        })
    }

    fn try_resize(&mut self, geometry: WorkspaceGeometry) -> Result<(), SpatialSamplingError> {
        let additional = geometry.entry_count.saturating_sub(self.sums.len());
        self.sums
            .try_reserve_exact(additional)
            .map_err(|_| allocation_error(geometry))?;
        self.sums.resize(geometry.entry_count, [0; 3]);
        self.width = geometry.width;
        self.height = geometry.height;
        self.stride = geometry.stride;
        Ok(())
    }

    fn matches(&self, width: u32, height: u32) -> bool {
        self.width == width && self.height == height
    }

    fn entry_count(&self) -> usize {
        self.sums.len()
    }

    fn geometry(&self) -> WorkspaceGeometry {
        WorkspaceGeometry {
            stride: self.stride,
            entry_count: self.sums.len(),
            width: self.width,
            height: self.height,
            byte_len: self
                .sums
                .len()
                .checked_mul(std::mem::size_of::<[u64; 3]>())
                .expect("admitted workspace byte length must remain addressable"),
        }
    }

    fn rebuild(&mut self, canvas: &Canvas) {
        debug_assert!(self.matches(canvas.width(), canvas.height()));
        self.sums[..self.stride].fill([0; 3]);

        let bytes = canvas.as_rgba_bytes();
        let source_stride = usize::try_from(self.width)
            .expect("admitted canvas width must fit usize")
            * BYTES_PER_PIXEL;
        let width = usize::try_from(self.width).expect("admitted canvas width must fit usize");
        let height = usize::try_from(self.height).expect("admitted canvas height must fit usize");

        for y in 0..height {
            let source_row = y * source_stride;
            let table_row = (y + 1) * self.stride;
            let table_above = y * self.stride;
            self.sums[table_row] = [0; 3];
            let mut row_sum = [0_u64; 3];
            for x in 0..width {
                let source = source_row + x * BYTES_PER_PIXEL;
                row_sum[0] += u64::from(decode_srgb_byte(bytes[source]));
                row_sum[1] += u64::from(decode_srgb_byte(bytes[source + 1]));
                row_sum[2] += u64::from(decode_srgb_byte(bytes[source + 2]));
                let above = self.sums[table_above + x + 1];
                self.sums[table_row + x + 1] = [
                    row_sum[0] + above[0],
                    row_sum[1] + above[1],
                    row_sum[2] + above[2],
                ];
            }
        }
    }

    pub(super) fn sample(&self, sample: &PreparedAreaSample) -> [u16; 3] {
        debug_assert!(self.matches(sample.canvas_width, sample.canvas_height));
        let x = ClampedAxis::new(sample.center_x, sample.radius_x, self.width - 1);
        let y = ClampedAxis::new(sample.center_y, sample.radius_y, self.height - 1);
        let mut sum = [0_u128; 3];

        add_weighted(
            &mut sum,
            self.rectangle_sum(x.start, y.start, x.end, y.end),
            1,
        );
        add_weighted(
            &mut sum,
            self.rectangle_sum(0, y.start, 0, y.end),
            u128::from(x.before),
        );
        add_weighted(
            &mut sum,
            self.rectangle_sum(self.width - 1, y.start, self.width - 1, y.end),
            u128::from(x.after),
        );
        add_weighted(
            &mut sum,
            self.rectangle_sum(x.start, 0, x.end, 0),
            u128::from(y.before),
        );
        add_weighted(
            &mut sum,
            self.rectangle_sum(x.start, self.height - 1, x.end, self.height - 1),
            u128::from(y.after),
        );

        for (pixel_x, x_weight) in [(0, x.before), (self.width - 1, x.after)] {
            for (pixel_y, y_weight) in [(0, y.before), (self.height - 1, y.after)] {
                add_weighted(
                    &mut sum,
                    self.rectangle_sum(pixel_x, pixel_y, pixel_x, pixel_y),
                    u128::from(x_weight) * u128::from(y_weight),
                );
            }
        }

        let count = u128::from(x.sample_count) * u128::from(y.sample_count);
        [
            u16::try_from(sum[0] / count).expect("area average must fit in u16"),
            u16::try_from(sum[1] / count).expect("area average must fit in u16"),
            u16::try_from(sum[2] / count).expect("area average must fit in u16"),
        ]
    }

    fn rectangle_sum(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> [u64; 3] {
        let x0 = usize::try_from(x0).expect("admitted x coordinate must fit usize");
        let y0 = usize::try_from(y0).expect("admitted y coordinate must fit usize");
        let x1 = usize::try_from(x1).expect("admitted x coordinate must fit usize") + 1;
        let y1 = usize::try_from(y1).expect("admitted y coordinate must fit usize") + 1;
        let bottom_right = self.sums[y1 * self.stride + x1];
        let bottom_left = self.sums[y1 * self.stride + x0];
        let top_right = self.sums[y0 * self.stride + x1];
        let top_left = self.sums[y0 * self.stride + x0];
        [
            (bottom_right[0] - bottom_left[0]) - (top_right[0] - top_left[0]),
            (bottom_right[1] - bottom_left[1]) - (top_right[1] - top_left[1]),
            (bottom_right[2] - bottom_left[2]) - (top_right[2] - top_left[2]),
        ]
    }
}

#[derive(Debug, Clone, Copy)]
struct WorkspaceGeometry {
    stride: usize,
    entry_count: usize,
    width: u32,
    height: u32,
    byte_len: usize,
}

impl WorkspaceGeometry {
    fn try_new(width: u32, height: u32) -> Result<Self, SpatialSamplingError> {
        let error = || SpatialSamplingError::AreaWorkspaceUnaddressable { width, height };
        if width == 0 || height == 0 {
            return Err(error());
        }
        let stride = usize::try_from(u64::from(width) + 1).map_err(|_| error())?;
        let rows = usize::try_from(u64::from(height) + 1).map_err(|_| error())?;
        let entry_count = stride.checked_mul(rows).ok_or_else(error)?;
        let byte_len = entry_count
            .checked_mul(std::mem::size_of::<[u64; 3]>())
            .ok_or_else(error)?;
        u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(u64::from(u16::MAX)))
            .ok_or_else(error)?;
        Ok(Self {
            stride,
            entry_count,
            width,
            height,
            byte_len,
        })
    }

    fn descriptor(self) -> WorkspaceDescriptor {
        WorkspaceDescriptor {
            width: self.width,
            height: self.height,
        }
    }
}

fn allocate_sums(geometry: WorkspaceGeometry) -> Result<Vec<[u64; 3]>, SpatialSamplingError> {
    let mut sums = Vec::new();
    sums.try_reserve_exact(geometry.entry_count).map_err(|_| {
        SpatialSamplingError::AreaWorkspaceAllocation {
            width: geometry.width,
            height: geometry.height,
            entry_count: geometry.entry_count,
        }
    })?;
    sums.resize(geometry.entry_count, [0; 3]);
    Ok(sums)
}

#[derive(Debug, Clone, Copy)]
struct ClampedAxis {
    start: u32,
    end: u32,
    before: u64,
    after: u64,
    sample_count: u64,
}

impl ClampedAxis {
    fn new(center: u32, radius: u32, maximum: u32) -> Self {
        let center = u64::from(center);
        let radius = u64::from(radius);
        let maximum = u64::from(maximum);
        let upper = center + radius;
        Self {
            start: u32::try_from(center.saturating_sub(radius))
                .expect("clamped axis start must fit u32"),
            end: u32::try_from(upper.min(maximum)).expect("clamped axis end must fit u32"),
            before: radius.saturating_sub(center),
            after: upper.saturating_sub(maximum),
            sample_count: radius * 2 + 1,
        }
    }
}

fn add_weighted(sum: &mut [u128; 3], value: [u64; 3], weight: u128) {
    sum[0] += u128::from(value[0]) * weight;
    sum[1] += u128::from(value[1]) * weight;
    sum[2] += u128::from(value[2]) * weight;
}
