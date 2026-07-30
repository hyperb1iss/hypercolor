use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard};

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas};

use super::lut::decode_srgb_byte;
use crate::spatial::{PreparedAreaSample, SpatialSamplingCapacity, SpatialSamplingError};

#[derive(Debug)]
pub(crate) struct AreaWorkspacePool {
    state: Mutex<AreaWorkspacePoolState>,
    capacity: SpatialSamplingCapacity,
}

#[derive(Debug)]
struct AreaWorkspacePoolState {
    available: Vec<SummedAreaWorkspace>,
    leased: usize,
    reserved: usize,
}

impl AreaWorkspacePool {
    pub(crate) fn try_new(
        width: u32,
        height: u32,
        capacity: SpatialSamplingCapacity,
    ) -> Result<Arc<Self>, SpatialSamplingError> {
        let workspace = SummedAreaWorkspace::try_new(width, height, capacity)?;
        let mut available = Vec::new();
        available.try_reserve_exact(1).map_err(|_| {
            SpatialSamplingError::AreaWorkspaceAllocation {
                width,
                height,
                entry_count: workspace.entry_count(),
            }
        })?;
        available.push(workspace);
        Ok(Arc::new(Self {
            state: Mutex::new(AreaWorkspacePoolState {
                available,
                leased: 0,
                reserved: 0,
            }),
            capacity,
        }))
    }

    pub(crate) fn try_checkout(
        self: &Arc<Self>,
        canvas: &Canvas,
    ) -> Result<AreaWorkspaceLease, SpatialSamplingError> {
        let width = canvas.width();
        let height = canvas.height();
        let mut state = self.lock_state();
        let matching = state
            .available
            .iter()
            .position(|workspace| workspace.matches(width, height));
        let workspace = match matching {
            Some(index) => Some(state.available.swap_remove(index)),
            None => state.available.pop(),
        };
        if workspace.is_none() {
            reserve_pool_slot(&mut state, width, height)?;
        }
        state.leased += 1;
        drop(state);

        let mut workspace = match workspace {
            Some(workspace) => workspace,
            None => match SummedAreaWorkspace::try_new(width, height, self.capacity) {
                Ok(workspace) => workspace,
                Err(error) => {
                    self.cancel_lease();
                    return Err(error);
                }
            },
        };
        if let Err(error) = workspace.try_resize(width, height, self.capacity) {
            self.checkin(workspace);
            return Err(error);
        }
        workspace.rebuild(canvas);
        Ok(AreaWorkspaceLease {
            pool: Arc::clone(self),
            workspace: Some(workspace),
        })
    }

    pub(crate) fn try_prepare(
        self: &Arc<Self>,
        width: u32,
        height: u32,
    ) -> Result<(), SpatialSamplingError> {
        let mut state = self.lock_state();
        if state
            .available
            .iter()
            .any(|workspace| workspace.matches(width, height))
        {
            return Ok(());
        }
        reserve_pool_slot(&mut state, width, height)?;
        state.reserved += 1;
        drop(state);

        let workspace = match SummedAreaWorkspace::try_new(width, height, self.capacity) {
            Ok(workspace) => workspace,
            Err(error) => {
                self.cancel_reservation();
                return Err(error);
            }
        };
        let mut state = self.lock_state();
        state.reserved -= 1;
        state.available.push(workspace);
        Ok(())
    }

    fn checkin(&self, workspace: SummedAreaWorkspace) {
        let mut state = self.lock_state();
        state.leased -= 1;
        state.available.push(workspace);
    }

    fn cancel_lease(&self) {
        self.lock_state().leased -= 1;
    }

    fn cancel_reservation(&self) {
        self.lock_state().reserved -= 1;
    }

    fn lock_state(&self) -> MutexGuard<'_, AreaWorkspacePoolState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn reserve_pool_slot(
    state: &mut AreaWorkspacePoolState,
    width: u32,
    height: u32,
) -> Result<(), SpatialSamplingError> {
    let required_slots = state
        .available
        .len()
        .saturating_add(state.leased)
        .saturating_add(state.reserved)
        .saturating_add(1);
    if state.available.capacity() >= required_slots {
        return Ok(());
    }
    let geometry = WorkspaceGeometry::try_new(width, height)?;
    state
        .available
        .try_reserve_exact(required_slots - state.available.len())
        .map_err(|_| SpatialSamplingError::AreaWorkspaceAllocation {
            width,
            height,
            entry_count: geometry.entry_count,
        })
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
    fn try_new(
        width: u32,
        height: u32,
        capacity: SpatialSamplingCapacity,
    ) -> Result<Self, SpatialSamplingError> {
        let geometry = WorkspaceGeometry::try_new(width, height)?;
        geometry.validate_capacity(capacity)?;
        let sums = allocate_sums(geometry)?;
        Ok(Self {
            width,
            height,
            stride: geometry.stride,
            sums,
        })
    }

    fn try_resize(
        &mut self,
        width: u32,
        height: u32,
        capacity: SpatialSamplingCapacity,
    ) -> Result<(), SpatialSamplingError> {
        if self.matches(width, height) {
            return Ok(());
        }
        let geometry = WorkspaceGeometry::try_new(width, height)?;
        geometry.validate_capacity(capacity)?;
        let sums = allocate_sums(geometry)?;
        self.width = width;
        self.height = height;
        self.stride = geometry.stride;
        self.sums = sums;
        Ok(())
    }

    fn matches(&self, width: u32, height: u32) -> bool {
        self.width == width && self.height == height
    }

    fn entry_count(&self) -> usize {
        self.sums.len()
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

    fn validate_capacity(
        self,
        capacity: SpatialSamplingCapacity,
    ) -> Result<(), SpatialSamplingError> {
        let capacity_bytes = capacity.max_area_workspace_bytes();
        if self.byte_len > capacity_bytes {
            return Err(SpatialSamplingError::AreaWorkspaceCapacityExceeded {
                width: self.width,
                height: self.height,
                required_bytes: self.byte_len,
                capacity_bytes,
            });
        }
        Ok(())
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
