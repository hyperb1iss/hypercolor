use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacosPixelExtent {
    pub width: u32,
    pub height: u32,
}

impl MacosPixelExtent {
    pub fn new(width: u32, height: u32) -> Result<Self, MacosGeometryError> {
        if width == 0 || height == 0 {
            return Err(MacosGeometryError::EmptyExtent);
        }
        Ok(Self { width, height })
    }

    pub fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MacosScale(f64);

impl MacosScale {
    pub fn new(value: f64) -> Result<Self, MacosGeometryError> {
        if !value.is_finite() || value <= 0.0 {
            return Err(MacosGeometryError::InvalidScale(value));
        }
        Ok(Self(value))
    }

    pub fn display(value: f64) -> Result<Self, MacosGeometryError> {
        let scale = Self::new(value)?;
        if value > 4.0 {
            return Err(MacosGeometryError::InvalidDisplayScale(value));
        }
        Ok(scale)
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MacosPointRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl MacosPointRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Result<Self, MacosGeometryError> {
        if !x.is_finite() || !y.is_finite() || !width.is_finite() || !height.is_finite() {
            return Err(MacosGeometryError::NonFiniteRect);
        }
        if width <= 0.0 || height <= 0.0 {
            return Err(MacosGeometryError::EmptyRect);
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn to_pixel_rect(self, scale: MacosScale) -> Result<MacosPixelRect, MacosGeometryError> {
        let min_x = checked_floor(self.x * scale.get())?;
        let min_y = checked_floor(self.y * scale.get())?;
        let max_x = checked_ceil((self.x + self.width) * scale.get())?;
        let max_y = checked_ceil((self.y + self.height) * scale.get())?;
        let width = max_x
            .checked_sub(min_x)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(MacosGeometryError::RectOverflow)?;
        let height = max_y
            .checked_sub(min_y)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(MacosGeometryError::RectOverflow)?;
        MacosPixelRect::new(min_x, min_y, width, height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacosPixelRect {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

impl MacosPixelRect {
    pub fn new(x: i64, y: i64, width: u32, height: u32) -> Result<Self, MacosGeometryError> {
        if width == 0 || height == 0 {
            return Err(MacosGeometryError::EmptyRect);
        }
        x.checked_add(i64::from(width))
            .ok_or(MacosGeometryError::RectOverflow)?;
        y.checked_add(i64::from(height))
            .ok_or(MacosGeometryError::RectOverflow)?;
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn fits_within(self, extent: MacosPixelExtent) -> bool {
        self.x >= 0
            && self.y >= 0
            && self
                .x
                .checked_add(i64::from(self.width))
                .is_some_and(|right| right <= i64::from(extent.width))
            && self
                .y
                .checked_add(i64::from(self.height))
                .is_some_and(|bottom| bottom <= i64::from(extent.height))
    }

    pub fn clip_to(self, extent: MacosPixelExtent) -> Result<Self, MacosGeometryError> {
        let right = self
            .x
            .checked_add(i64::from(self.width))
            .ok_or(MacosGeometryError::RectOverflow)?;
        let bottom = self
            .y
            .checked_add(i64::from(self.height))
            .ok_or(MacosGeometryError::RectOverflow)?;
        let min_x = self.x.max(0);
        let min_y = self.y.max(0);
        let max_x = right.min(i64::from(extent.width));
        let max_y = bottom.min(i64::from(extent.height));
        if max_x <= min_x || max_y <= min_y {
            return Err(MacosGeometryError::RectOutsideStorage);
        }
        let width = u32::try_from(max_x - min_x).map_err(|_| MacosGeometryError::RectOverflow)?;
        let height = u32::try_from(max_y - min_y).map_err(|_| MacosGeometryError::RectOverflow)?;
        Self::new(min_x, min_y, width, height)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacosCaptureGeometry {
    pub display_scale_factor: MacosScale,
    pub content_scale: MacosScale,
    pub content_rect_points: MacosPointRect,
    pub content_rect_pixels: MacosPixelRect,
    pub screen_rect_points: Option<MacosPointRect>,
    pub bounding_rect_points: Option<MacosPointRect>,
    pub bounding_rect_pixels: Option<MacosPixelRect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum MacosGeometryError {
    #[error("pixel extent must be nonzero")]
    EmptyExtent,
    #[error("rectangle extent must be nonzero")]
    EmptyRect,
    #[error("rectangle contains a nonfinite coordinate")]
    NonFiniteRect,
    #[error("scale must be finite and positive, got {0}")]
    InvalidScale(f64),
    #[error("display scale must be within Apple's [1, 4] range, got {0}")]
    InvalidDisplayScale(f64),
    #[error("rectangle arithmetic overflowed")]
    RectOverflow,
    #[error("rectangle does not intersect pixel storage")]
    RectOutsideStorage,
}

fn checked_floor(value: f64) -> Result<i64, MacosGeometryError> {
    let value = value.floor();
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(MacosGeometryError::RectOverflow);
    }
    Ok(value as i64)
}

fn checked_ceil(value: f64) -> Result<i64, MacosGeometryError> {
    let value = value.ceil();
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(MacosGeometryError::RectOverflow);
    }
    Ok(value as i64)
}
