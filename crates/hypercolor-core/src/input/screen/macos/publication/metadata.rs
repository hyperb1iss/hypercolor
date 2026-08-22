use super::super::{
    Arc, CaptureColorSpace, CaptureColorimetry, CaptureDynamicRange, CaptureLuminanceContext,
    CapturePixelFormat, CapturePositiveScalar, CaptureSourceId, CaptureTransferFunction,
    DEFAULT_HDR_SOURCE_CONTENT_HEADROOM, DEFAULT_HDR_SOURCE_REFERENCE_WHITE_NITS,
    MacosCaptureContentStyle, MacosCaptureDynamicRange, MacosCaptureFrame, MacosCapturePixelFormat,
    MacosCaptureSelection, MacosColorPrimaries, MacosNativeTargetManifest, MacosTransferFunction,
    ResolvedScreenPublicationDescriptor, ResourceDescriptor, ResourceState,
    ScreenPhysicalGpuDeviceIdentity, TopologyDescriptor, TopologyState, anyhow,
};

impl MacosNativeTargetManifest {
    pub(in crate::input::screen::macos) fn new(
        descriptor: &ResolvedScreenPublicationDescriptor,
    ) -> anyhow::Result<Self> {
        let resources = descriptor.physical().source().resources();
        let ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(metal_registry_id) = resources
            .physical_gpu_device()
            .ok_or_else(|| anyhow!("macOS native publication is missing Metal identity"))?
        else {
            return Err(anyhow!(
                "macOS native publication selected a non-Metal device"
            ));
        };
        if *metal_registry_id == 0
            || resources.device_generation() == 0
            || resources.resource_generation() == 0
        {
            return Err(anyhow!(
                "macOS native publication generations must be nonzero"
            ));
        }
        Ok(Self {
            capture_session_generation: resources.device_generation(),
            resource_generation: resources.resource_generation(),
            metal_registry_id: *metal_registry_id,
        })
    }

    /// Capture-session generation whose surfaces this target accepts.
    #[must_use]
    pub const fn capture_session_generation(&self) -> u64 {
        self.capture_session_generation
    }

    /// Storage-descriptor generation whose surfaces this target accepts.
    #[must_use]
    pub const fn resource_generation(&self) -> u64 {
        self.resource_generation
    }

    /// Physical Metal device registry identity.
    #[must_use]
    pub const fn metal_registry_id(&self) -> u64 {
        self.metal_registry_id
    }
}

impl TopologyState {
    pub(in crate::input::screen::macos) fn observe(
        &mut self,
        frame: &MacosCaptureFrame,
    ) -> anyhow::Result<u64> {
        let descriptor = TopologyDescriptor::from_frame(frame);
        if self.descriptor.as_ref() != Some(&descriptor) {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| anyhow!("macOS topology generation exhausted"))?;
            self.descriptor = Some(descriptor);
        }
        Ok(self.generation)
    }
}

impl ResourceState {
    pub(in crate::input::screen::macos) fn observe(
        &mut self,
        frame: &MacosCaptureFrame,
    ) -> anyhow::Result<u64> {
        let descriptor = ResourceDescriptor::from_frame(frame);
        if self.descriptor.as_ref() != Some(&descriptor) {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| anyhow!("macOS resource generation exhausted"))?;
            self.descriptor = Some(descriptor);
        }
        Ok(self.generation)
    }
}

impl ResourceDescriptor {
    pub(in crate::input::screen::macos) fn from_frame(frame: &MacosCaptureFrame) -> Self {
        Self {
            width: frame.storage_extent.width,
            height: frame.storage_extent.height,
            pixel_format: frame.pixel_format,
            planes: frame
                .planes
                .iter()
                .map(|plane| {
                    (
                        plane.index,
                        plane.extent.width,
                        plane.extent.height,
                        plane.bytes_per_row,
                        plane.length_bytes,
                    )
                })
                .collect(),
        }
    }
}

impl TopologyDescriptor {
    pub(in crate::input::screen::macos) fn from_frame(frame: &MacosCaptureFrame) -> Self {
        let content = frame.geometry.content_rect_pixels;
        Self {
            width: frame.storage_extent.width,
            height: frame.storage_extent.height,
            content: (content.x, content.y, content.width, content.height),
            scale_bits: frame.geometry.display_scale_factor.get().to_bits(),
            screen: frame.geometry.screen_rect_points.map(|rect| {
                (
                    rect.x.to_bits(),
                    rect.y.to_bits(),
                    rect.width.to_bits(),
                    rect.height.to_bits(),
                )
            }),
        }
    }
}

pub(in crate::input::screen::macos) fn capture_source_id(
    selection: MacosCaptureSelection,
) -> anyhow::Result<CaptureSourceId> {
    let source: Arc<str> = match selection {
        MacosCaptureSelection::Display { source_id } => source_id,
        MacosCaptureSelection::SessionScoped { content_style } => Arc::from(match content_style {
            MacosCaptureContentStyle::Window => "macos:session:window",
            MacosCaptureContentStyle::MultipleWindows => "macos:session:multiple-windows",
            MacosCaptureContentStyle::Application => "macos:session:application",
            MacosCaptureContentStyle::MultipleApplications => "macos:session:multiple-applications",
            MacosCaptureContentStyle::Mixed => "macos:session:mixed",
        }),
        MacosCaptureSelection::None => Arc::from("macos:session"),
    };
    Ok(CaptureSourceId::new(source)?)
}

pub(in crate::input::screen::macos) fn capture_colorimetry(
    frame: &MacosCaptureFrame,
) -> anyhow::Result<CaptureColorimetry> {
    let color = frame.color;
    let color_space = match color.primaries {
        MacosColorPrimaries::Srgb => CaptureColorSpace::Srgb,
        MacosColorPrimaries::DisplayP3 => CaptureColorSpace::DisplayP3,
        MacosColorPrimaries::Rec2020 => CaptureColorSpace::Rec2020,
    };
    let transfer_function = match color.transfer {
        MacosTransferFunction::Srgb => CaptureTransferFunction::Srgb,
        MacosTransferFunction::Rec709 => CaptureTransferFunction::Rec709,
        MacosTransferFunction::Rec2020 => CaptureTransferFunction::Rec2020,
        MacosTransferFunction::Linear => CaptureTransferFunction::Linear,
        MacosTransferFunction::Pq => CaptureTransferFunction::Pq,
        MacosTransferFunction::Hlg => CaptureTransferFunction::Hlg,
    };
    let delivered = frame.delivered_metadata();
    let dynamic_range = if matches!(
        color.transfer,
        MacosTransferFunction::Pq | MacosTransferFunction::Hlg
    ) || delivered
        .is_some_and(|metadata| metadata.dynamic_range == MacosCaptureDynamicRange::Hdr)
        || matches!(
            frame.pixel_format,
            MacosCapturePixelFormat::Argb2101010 | MacosCapturePixelFormat::Rgba16Float
        ) {
        CaptureDynamicRange::High
    } else {
        CaptureDynamicRange::Standard
    };
    let luminance = if dynamic_range == CaptureDynamicRange::High {
        let delivered = delivered
            .ok_or_else(|| anyhow!("macOS HDR capture is missing delivered luminance metadata"))?;
        if delivered.pixel_format != frame.pixel_format
            || delivered.color != frame.color
            || delivered.dynamic_range != MacosCaptureDynamicRange::Hdr
        {
            return Err(anyhow!(
                "macOS HDR delivered metadata contradicts the capture frame"
            ));
        }
        // ScreenCaptureKit only sometimes attaches ContentLightLevelInfo and
        // IOSurfaceContentHeadroom; spec 76 requires a source reference white
        // regardless and treats headroom as best-effort. When the OS stays
        // silent, anchor reference white at BT.2408 diffuse white (203 nits,
        // the same default as the target LED calibration, so unsignalled
        // content maps through at unity) and assume one stop of highlight
        // headroom, matching the output headroom the tone map reserves.
        let reference_white = delivered
            .source_reference_white_nits
            .unwrap_or(DEFAULT_HDR_SOURCE_REFERENCE_WHITE_NITS);
        // EDR headroom is dynamic: macOS reports it relative to the current
        // display brightness, and at high brightness it legitimately reaches
        // 1.0 (no room above SDR white right now). No real pixel can exceed
        // reference white in that state, so giving the tone map its default
        // rolloff space is visually identity while keeping the luminance
        // contract well-formed.
        let headroom = delivered
            .content_headroom
            .filter(|headroom| *headroom > 1.0)
            .unwrap_or(DEFAULT_HDR_SOURCE_CONTENT_HEADROOM);
        let reference_white = CapturePositiveScalar::try_new(reference_white)?;
        let peak = CapturePositiveScalar::try_new(reference_white.value() * headroom)?;
        Some(CaptureLuminanceContext::new(reference_white, peak)?)
    } else {
        None
    };
    Ok(CaptureColorimetry::new(
        color_space,
        transfer_function,
        Some(dynamic_range),
        luminance,
    )?)
}

pub(in crate::input::screen::macos) const fn capture_pixel_format(
    format: MacosCapturePixelFormat,
) -> CapturePixelFormat {
    match format {
        MacosCapturePixelFormat::Bgra8 => CapturePixelFormat::Bgra8,
        MacosCapturePixelFormat::Argb2101010 => CapturePixelFormat::Argb2101010,
        MacosCapturePixelFormat::Rgba16Float => CapturePixelFormat::Rgba16Float,
        MacosCapturePixelFormat::Yuv420VideoRange => CapturePixelFormat::Yuv420VideoRange,
        MacosCapturePixelFormat::Yuv420FullRange => CapturePixelFormat::Yuv420FullRange,
        MacosCapturePixelFormat::Yuv44410BiPlanar => CapturePixelFormat::Yuv44410BiPlanar,
    }
}

pub(in crate::input::screen::macos) fn capture_origin(
    frame: &MacosCaptureFrame,
) -> anyhow::Result<super::super::super::PhysicalOrigin> {
    let rect = frame
        .geometry
        .screen_rect_points
        .unwrap_or(frame.geometry.content_rect_points);
    let scale = frame.geometry.display_scale_factor.get();
    Ok(super::super::super::PhysicalOrigin {
        x: scaled_coordinate(rect.x, scale)?,
        y: scaled_coordinate(rect.y, scale)?,
    })
}

pub(in crate::input::screen::macos) fn scaled_coordinate(
    value: f64,
    scale: f64,
) -> anyhow::Result<i32> {
    let value = (value * scale).floor();
    if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(anyhow!("macOS capture origin exceeds i32"));
    }
    Ok(value as i32)
}
