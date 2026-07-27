//! Canonical capture geometry processing.

use crate::input::screen::frame::{
    CaptureDamage, CaptureFrame, CaptureFrameError, CaptureGeometry, CapturePixelFormat,
    CapturePlanePool, CaptureRotation, CaptureStorage, CpuCaptureStorage, PhysicalOrigin,
    PixelExtent, ProcessedCaptureSurface, RawCaptureSurface,
};

/// Reusable raw-to-processed capture converter.
#[derive(Clone, Debug, Default)]
pub struct CaptureFrameProcessor {
    plane_pool: CapturePlanePool,
}

impl CaptureFrameProcessor {
    /// Convert pending crop and rotation metadata into canonical RGBA pixels.
    ///
    /// Already canonical tightly packed RGBA and GPU frames take a zero-copy
    /// transition. CPU frames requiring geometry or channel normalization use
    /// the reusable plane pool.
    ///
    /// # Errors
    ///
    /// Returns a typed frame error when geometry arithmetic overflows or a GPU
    /// frame requires a transform that cannot be performed without its native
    /// interop backend.
    pub fn process(
        &self,
        frame: CaptureFrame<RawCaptureSurface>,
    ) -> Result<CaptureFrame<ProcessedCaptureSurface>, CaptureFrameError> {
        let geometry = frame.metadata().geometry;
        let storage_extent = geometry.storage_extent();
        let identity_geometry =
            geometry.rotation() == CaptureRotation::Identity && geometry.crop().is_none();

        if identity_geometry {
            match frame.storage() {
                CaptureStorage::Cpu(storage)
                    if storage.tightly_packed_rgba8(storage_extent).is_some() =>
                {
                    let storage = CaptureStorage::Cpu(storage.clone());
                    let damage = frame.damage().clone();
                    return frame.into_processed(
                        canonical_geometry(geometry, geometry.native_extent(), storage_extent)?,
                        storage,
                        damage,
                    );
                }
                CaptureStorage::Gpu(storage) => {
                    let storage = CaptureStorage::Gpu(storage.clone());
                    let damage = frame.damage().clone();
                    return frame.into_processed(
                        canonical_geometry(geometry, geometry.native_extent(), storage_extent)?,
                        storage,
                        damage,
                    );
                }
                CaptureStorage::Cpu(_) => {}
            }
        }

        let CaptureStorage::Cpu(storage) = frame.storage() else {
            return Err(CaptureFrameError::GpuGeometryProcessingRequired);
        };
        let native_crop = geometry.crop().map_or(
            NativeCrop {
                x: 0,
                y: 0,
                extent: geometry.native_extent(),
            },
            |crop| NativeCrop {
                x: crop.x(),
                y: crop.y(),
                extent: crop.extent(),
            },
        );
        let storage_crop = storage_crop(geometry, native_crop)?;
        let output_extent = geometry.rotation().apply_to_extent(storage_crop.extent);
        let logical_extent = geometry.rotation().apply_to_extent(native_crop.extent);
        let output_len = rgba_len(output_extent)?;
        let mut plane = self.plane_pool.acquire(output_len);
        plane.resize(output_len, 0);
        transform_cpu_pixels(
            storage,
            storage_extent,
            storage_crop,
            geometry.rotation(),
            &mut plane,
        )?;
        let row_stride = i64::from(output_extent.width())
            .checked_mul(4)
            .ok_or(CaptureFrameError::StorageSizeOverflow)?;
        let processed_storage = CaptureStorage::Cpu(CpuCaptureStorage::from_owner(
            plane.freeze(),
            CapturePixelFormat::Rgba8,
            row_stride,
            0,
        ));
        let cursor = transform_cursor(&frame.metadata().cursor, native_crop, geometry.rotation())?;

        frame.into_processed_with_cursor(
            canonical_geometry(geometry, logical_extent, output_extent)?,
            processed_storage,
            CaptureDamage::default(),
            cursor,
        )
    }

    /// Number of CPU backing allocations created by this processor.
    #[must_use]
    pub fn allocation_count(&self) -> usize {
        self.plane_pool.allocation_count()
    }
}

#[derive(Clone, Copy)]
struct NativeCrop {
    x: u32,
    y: u32,
    extent: PixelExtent,
}

#[derive(Clone, Copy)]
struct StorageCrop {
    x: u32,
    y: u32,
    extent: PixelExtent,
}

fn canonical_geometry(
    source: CaptureGeometry,
    logical_extent: PixelExtent,
    storage_extent: PixelExtent,
) -> Result<CaptureGeometry, CaptureFrameError> {
    CaptureGeometry::new(
        source.origin(),
        logical_extent,
        storage_extent,
        CaptureRotation::Identity,
        None,
        source.source_scale(),
    )
}

fn storage_crop(
    geometry: CaptureGeometry,
    crop: NativeCrop,
) -> Result<StorageCrop, CaptureFrameError> {
    let native = geometry.native_extent();
    let storage = geometry.storage_extent();
    let end_x = crop
        .x
        .checked_add(crop.extent.width())
        .ok_or(CaptureFrameError::StorageSizeOverflow)?;
    let end_y = crop
        .y
        .checked_add(crop.extent.height())
        .ok_or(CaptureFrameError::StorageSizeOverflow)?;
    let x = scale_floor(crop.x, storage.width(), native.width());
    let y = scale_floor(crop.y, storage.height(), native.height());
    let right = scale_ceil(end_x, storage.width(), native.width());
    let bottom = scale_ceil(end_y, storage.height(), native.height());
    let width = right
        .checked_sub(x)
        .ok_or(CaptureFrameError::StorageSizeOverflow)?;
    let height = bottom
        .checked_sub(y)
        .ok_or(CaptureFrameError::StorageSizeOverflow)?;
    Ok(StorageCrop {
        x,
        y,
        extent: PixelExtent::new(width, height)?,
    })
}

fn scale_floor(value: u32, stored: u32, native: u32) -> u32 {
    ((u64::from(value) * u64::from(stored)) / u64::from(native)) as u32
}

fn scale_ceil(value: u32, stored: u32, native: u32) -> u32 {
    let product = u64::from(value) * u64::from(stored);
    product.div_ceil(u64::from(native)) as u32
}

fn rgba_len(extent: PixelExtent) -> Result<usize, CaptureFrameError> {
    usize::try_from(extent.width())
        .ok()
        .and_then(|width| {
            usize::try_from(extent.height())
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureFrameError::StorageSizeOverflow)
}

fn transform_cpu_pixels(
    storage: &CpuCaptureStorage,
    storage_extent: PixelExtent,
    crop: StorageCrop,
    rotation: CaptureRotation,
    output: &mut [u8],
) -> Result<(), CaptureFrameError> {
    let output_extent = rotation.apply_to_extent(crop.extent);
    let output_width = usize::try_from(output_extent.width())
        .map_err(|_| CaptureFrameError::StorageSizeOverflow)?;
    for y in 0..output_extent.height() {
        for x in 0..output_extent.width() {
            let (crop_x, crop_y) = inverse_rotation(x, y, crop.extent, rotation);
            let source_x = crop
                .x
                .checked_add(crop_x)
                .ok_or(CaptureFrameError::StorageSizeOverflow)?;
            let source_y = crop
                .y
                .checked_add(crop_y)
                .ok_or(CaptureFrameError::StorageSizeOverflow)?;
            let pixel = read_pixel(storage, storage_extent, source_x, source_y)?;
            let destination = usize::try_from(y)
                .ok()
                .and_then(|row| row.checked_mul(output_width))
                .and_then(|row| row.checked_add(usize::try_from(x).ok()?))
                .and_then(|index| index.checked_mul(4))
                .ok_or(CaptureFrameError::StorageSizeOverflow)?;
            output[destination..destination + 4].copy_from_slice(&pixel);
        }
    }
    Ok(())
}

const fn inverse_rotation(
    x: u32,
    y: u32,
    extent: PixelExtent,
    rotation: CaptureRotation,
) -> (u32, u32) {
    match rotation {
        CaptureRotation::Identity => (x, y),
        CaptureRotation::Clockwise90 => (y, extent.height() - 1 - x),
        CaptureRotation::Clockwise180 => (extent.width() - 1 - x, extent.height() - 1 - y),
        CaptureRotation::Clockwise270 => (extent.width() - 1 - y, x),
    }
}

fn read_pixel(
    storage: &CpuCaptureStorage,
    extent: PixelExtent,
    x: u32,
    y: u32,
) -> Result<[u8; 4], CaptureFrameError> {
    if x >= extent.width() || y >= extent.height() {
        return Err(CaptureFrameError::StorageSizeOverflow);
    }
    let offset = i128::try_from(storage.row0_offset())
        .ok()
        .and_then(|row0| {
            i128::from(storage.row_stride())
                .checked_mul(i128::from(y))
                .and_then(|row| row0.checked_add(row))
        })
        .and_then(|row| row.checked_add(i128::from(x) * 4))
        .and_then(|offset| usize::try_from(offset).ok())
        .ok_or(CaptureFrameError::StorageSizeOverflow)?;
    let end = offset
        .checked_add(4)
        .ok_or(CaptureFrameError::StorageSizeOverflow)?;
    let bytes = storage
        .bytes()
        .get(offset..end)
        .ok_or(CaptureFrameError::StorageSizeOverflow)?;
    Ok(match storage.format() {
        CapturePixelFormat::Rgba8 => [bytes[0], bytes[1], bytes[2], bytes[3]],
        CapturePixelFormat::Bgra8 => [bytes[2], bytes[1], bytes[0], bytes[3]],
    })
}

fn transform_cursor(
    cursor: &crate::input::screen::CaptureCursor,
    crop: NativeCrop,
    rotation: CaptureRotation,
) -> Result<crate::input::screen::CaptureCursor, CaptureFrameError> {
    let shape = cursor.shape_extent.unwrap_or(PixelExtent::new(1, 1)?);
    let position = cursor
        .position
        .map(|position| transform_cursor_origin(position, shape, crop, rotation))
        .transpose()?;
    let visible = cursor.visible
        && cursor
            .position
            .is_some_and(|position| cursor_intersects_crop(position, shape, crop));
    let hotspot = match (cursor.hotspot, cursor.shape_extent) {
        (Some(hotspot), Some(shape)) => Some(transform_hotspot(hotspot, shape, rotation)?),
        (hotspot, _) if rotation == CaptureRotation::Identity => hotspot,
        _ => None,
    };

    Ok(crate::input::screen::CaptureCursor {
        visible,
        position,
        hotspot,
        shape_extent: cursor
            .shape_extent
            .map(|extent| rotation.apply_to_extent(extent)),
        shape_generation: cursor.shape_generation,
        composed: cursor.composed,
    })
}

fn transform_cursor_origin(
    position: PhysicalOrigin,
    shape: PixelExtent,
    crop: NativeCrop,
    rotation: CaptureRotation,
) -> Result<PhysicalOrigin, CaptureFrameError> {
    let x = i64::from(position.x) - i64::from(crop.x);
    let y = i64::from(position.y) - i64::from(crop.y);
    let width = i64::from(crop.extent.width());
    let height = i64::from(crop.extent.height());
    let shape_width = i64::from(shape.width());
    let shape_height = i64::from(shape.height());
    let (x, y) = match rotation {
        CaptureRotation::Identity => (x, y),
        CaptureRotation::Clockwise90 => (height - y - shape_height, x),
        CaptureRotation::Clockwise180 => (width - x - shape_width, height - y - shape_height),
        CaptureRotation::Clockwise270 => (y, width - x - shape_width),
    };
    Ok(PhysicalOrigin {
        x: i32::try_from(x).map_err(|_| CaptureFrameError::CursorCoordinateOverflow)?,
        y: i32::try_from(y).map_err(|_| CaptureFrameError::CursorCoordinateOverflow)?,
    })
}

fn transform_hotspot(
    hotspot: PhysicalOrigin,
    shape: PixelExtent,
    rotation: CaptureRotation,
) -> Result<PhysicalOrigin, CaptureFrameError> {
    let x = i64::from(hotspot.x);
    let y = i64::from(hotspot.y);
    let width = i64::from(shape.width());
    let height = i64::from(shape.height());
    let (x, y) = match rotation {
        CaptureRotation::Identity => (x, y),
        CaptureRotation::Clockwise90 => (height - 1 - y, x),
        CaptureRotation::Clockwise180 => (width - 1 - x, height - 1 - y),
        CaptureRotation::Clockwise270 => (y, width - 1 - x),
    };
    Ok(PhysicalOrigin {
        x: i32::try_from(x).map_err(|_| CaptureFrameError::CursorCoordinateOverflow)?,
        y: i32::try_from(y).map_err(|_| CaptureFrameError::CursorCoordinateOverflow)?,
    })
}

fn cursor_intersects_crop(position: PhysicalOrigin, shape: PixelExtent, crop: NativeCrop) -> bool {
    let left = i64::from(position.x);
    let top = i64::from(position.y);
    let right = left + i64::from(shape.width());
    let bottom = top + i64::from(shape.height());
    let crop_left = i64::from(crop.x);
    let crop_top = i64::from(crop.y);
    let crop_right = crop_left + i64::from(crop.extent.width());
    let crop_bottom = crop_top + i64::from(crop.extent.height());
    left < crop_right && right > crop_left && top < crop_bottom && bottom > crop_top
}
