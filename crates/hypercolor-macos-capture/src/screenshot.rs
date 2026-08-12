use std::ffi::{CStr, OsStr, c_void};
use std::fmt;
use std::path::Path;
use std::ptr::{self, NonNull};
use std::sync::Arc;

use objc2::rc::Retained;
use objc2_core_foundation::{
    CFDictionary, CFNumber, CFNumberType, CFRetained, CFString, CFURL,
    kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks,
};
#[cfg(test)]
use objc2_core_graphics::CGBitmapContextCreateImage;
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContentToneMappingInfo, CGContext, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, CGToneMapping, kCGColorSpaceSRGB,
};

use crate::{MacosCaptureDynamicRange, MacosCaptureError, MacosPixelExtent};

pub const MAX_MACOS_SCREENSHOT_REFERENCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_COLOR_SPACE_NAME_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacosScreenshotReferenceCapability {
    PendingFirstFrame,
    SdrOnly {
        source_id: Arc<str>,
        generation: u64,
    },
    PairedSdrHdr {
        source_id: Arc<str>,
        generation: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosScreenshotPreferredDynamicRange {
    Standard,
    Constrained,
    High,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacosScreenshotReferenceMetadata {
    pub extent: MacosPixelExtent,
    pub color_space: Arc<str>,
    pub dynamic_range: MacosCaptureDynamicRange,
    pub bits_per_component: u16,
    pub bits_per_pixel: u16,
    pub bytes_per_row: u64,
    pub content_headroom: Option<f32>,
    pub content_average_light_level: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosScreenshotPixelCopy {
    pub extent: MacosPixelExtent,
    pub bytes_per_row: u64,
    pub rgba8: Vec<u8>,
}

#[derive(Clone)]
pub struct MacosScreenshotReferenceImage {
    image: Retained<CGImage>,
    metadata: MacosScreenshotReferenceMetadata,
}

impl MacosScreenshotReferenceImage {
    pub(crate) fn from_native(
        image: Retained<CGImage>,
        dynamic_range: MacosCaptureDynamicRange,
    ) -> Result<Self, MacosCaptureError> {
        let width = u32::try_from(CGImage::width(Some(&image)))
            .map_err(|_| MacosCaptureError::ScreenshotMetadataOutOfRange("width"))?;
        let height = u32::try_from(CGImage::height(Some(&image)))
            .map_err(|_| MacosCaptureError::ScreenshotMetadataOutOfRange("height"))?;
        let extent = MacosPixelExtent::new(width, height)?;
        let bits_per_component = u16::try_from(CGImage::bits_per_component(Some(&image)))
            .map_err(|_| MacosCaptureError::ScreenshotMetadataOutOfRange("bits_per_component"))?;
        let bits_per_pixel = u16::try_from(CGImage::bits_per_pixel(Some(&image)))
            .map_err(|_| MacosCaptureError::ScreenshotMetadataOutOfRange("bits_per_pixel"))?;
        let bytes_per_row = u64::try_from(CGImage::bytes_per_row(Some(&image)))
            .map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
        let allocation_bytes = bytes_per_row
            .checked_mul(u64::from(extent.height))
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        if allocation_bytes == 0 || allocation_bytes > MAX_MACOS_SCREENSHOT_REFERENCE_BYTES {
            return Err(MacosCaptureError::ScreenshotReferenceTooLarge {
                requested_bytes: allocation_bytes,
                maximum_bytes: MAX_MACOS_SCREENSHOT_REFERENCE_BYTES,
            });
        }
        let color_space = CGImage::color_space(Some(&image))
            .and_then(|color_space| CGColorSpace::name(Some(&color_space)))
            .ok_or(MacosCaptureError::MissingScreenshotColorSpace)?
            .to_string();
        if color_space.is_empty() || color_space.len() > MAX_COLOR_SPACE_NAME_BYTES {
            return Err(MacosCaptureError::ScreenshotMetadataOutOfRange(
                "color_space",
            ));
        }
        let content_headroom = positive_finite(CGImage::content_headroom(Some(&image)));
        let content_average_light_level = load_required_tahoe_symbol::<GetContentAverageLightLevel>(
            c"CGImageGetContentAverageLightLevel",
            "CGImageGetContentAverageLightLevel",
        )?;
        // SAFETY: the dynamically resolved Tahoe function has the SDK-declared
        // signature and the retained CGImage remains live for this call.
        let content_average_light_level =
            positive_finite(unsafe { content_average_light_level(Some(&image)) });
        Ok(Self {
            image,
            metadata: MacosScreenshotReferenceMetadata {
                extent,
                color_space: Arc::from(color_space),
                dynamic_range,
                bits_per_component,
                bits_per_pixel,
                bytes_per_row,
                content_headroom,
                content_average_light_level,
            },
        })
    }

    #[must_use]
    pub const fn metadata(&self) -> &MacosScreenshotReferenceMetadata {
        &self.metadata
    }

    pub fn copy_reference_rgba8(
        &self,
        preferred_dynamic_range: MacosScreenshotPreferredDynamicRange,
    ) -> Result<MacosScreenshotPixelCopy, MacosCaptureError> {
        let symbols = TahoeReferenceOutputSymbols::load()?;
        self.copy_reference_rgba8_with_symbols(preferred_dynamic_range, symbols)
    }

    fn copy_reference_rgba8_with_symbols(
        &self,
        preferred_dynamic_range: MacosScreenshotPreferredDynamicRange,
        symbols: TahoeReferenceOutputSymbols,
    ) -> Result<MacosScreenshotPixelCopy, MacosCaptureError> {
        let width = usize::try_from(self.metadata.extent.width)
            .map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
        let height = usize::try_from(self.metadata.extent.height)
            .map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
        let bytes_per_row = width
            .checked_mul(4)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let byte_len = bytes_per_row
            .checked_mul(height)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        if u64::try_from(byte_len).map_err(|_| MacosCaptureError::ArithmeticOverflow)?
            > MAX_MACOS_SCREENSHOT_REFERENCE_BYTES
        {
            return Err(MacosCaptureError::ScreenshotReferenceTooLarge {
                requested_bytes: u64::try_from(byte_len)
                    .map_err(|_| MacosCaptureError::ArithmeticOverflow)?,
                maximum_bytes: MAX_MACOS_SCREENSHOT_REFERENCE_BYTES,
            });
        }
        let mut rgba8 = vec![0_u8; byte_len];
        // SAFETY: Core Graphics exports a process-lifetime immutable CFString.
        let srgb = unsafe { kCGColorSpaceSRGB };
        let color_space = CGColorSpace::with_name(Some(srgb))
            .ok_or(MacosCaptureError::ScreenshotReferenceContextFailed)?;
        let bitmap_info =
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
        // SAFETY: the vector owns byte_len writable bytes and remains fixed
        // while the context exists. Its row and extent arithmetic is checked.
        let context = unsafe {
            CGBitmapContextCreate(
                rgba8.as_mut_ptr().cast(),
                width,
                height,
                8,
                bytes_per_row,
                Some(&color_space),
                bitmap_info,
            )
        }
        .ok_or(MacosCaptureError::ScreenshotReferenceContextFailed)?;
        let options = tone_mapping_options(
            preferred_dynamic_range,
            self.metadata.content_average_light_level,
            symbols,
        )?;
        // SAFETY: the dynamically resolved Tahoe function has the SDK-declared
        // signature. The context and retained options dictionary remain live.
        unsafe {
            (symbols.set_tone_mapping)(
                &context,
                CGContentToneMappingInfo {
                    method: CGToneMapping::ReferenceWhiteBased,
                    options: ptr::from_ref(&*options),
                },
            );
        }
        CGContext::draw_image(
            Some(&context),
            objc2_core_foundation::CGRect::new(
                objc2_core_foundation::CGPoint::new(0.0, 0.0),
                objc2_core_foundation::CGSize::new(
                    f64::from(width as u32),
                    f64::from(height as u32),
                ),
            ),
            Some(&self.image),
        );
        drop(context);
        Ok(MacosScreenshotPixelCopy {
            extent: self.metadata.extent,
            bytes_per_row: u64::try_from(bytes_per_row)
                .map_err(|_| MacosCaptureError::ArithmeticOverflow)?,
            rgba8,
        })
    }

    pub fn encode_png(&self, path: impl AsRef<Path>) -> Result<(), MacosCaptureError> {
        encode_png(&self.image, path.as_ref())
    }

    #[cfg(test)]
    pub(crate) fn new_fixture(dynamic_range: MacosCaptureDynamicRange, marker: u8) -> Self {
        let extent = MacosPixelExtent::new(1, 1).expect("fixture extent is valid");
        let mut pixel = [marker, marker, marker, u8::MAX];
        // SAFETY: Core Graphics exports a process-lifetime immutable CFString.
        let srgb = unsafe { kCGColorSpaceSRGB };
        let color_space =
            CGColorSpace::with_name(Some(srgb)).expect("fixture color space is available");
        // SAFETY: pixel is a fixed four-byte RGBA buffer retained until the
        // context creates its immutable CGImage copy.
        let context = unsafe {
            CGBitmapContextCreate(
                pixel.as_mut_ptr().cast(),
                1,
                1,
                8,
                4,
                Some(&color_space),
                CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
            )
        }
        .expect("fixture bitmap context is available");
        let image = CGBitmapContextCreateImage(Some(&context))
            .expect("fixture image should be materialized");
        Self {
            image: image.into(),
            metadata: MacosScreenshotReferenceMetadata {
                extent,
                color_space: Arc::from("kCGColorSpaceSRGB"),
                dynamic_range,
                bits_per_component: 8,
                bits_per_pixel: 32,
                bytes_per_row: 4,
                content_headroom: (dynamic_range == MacosCaptureDynamicRange::Hdr).then_some(4.0),
                content_average_light_level: Some(0.25),
            },
        }
    }
}

impl fmt::Debug for MacosScreenshotReferenceImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacosScreenshotReferenceImage")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum MacosScreenshotReferenceSet {
    Sdr {
        image: MacosScreenshotReferenceImage,
    },
    Paired {
        sdr: MacosScreenshotReferenceImage,
        hdr: MacosScreenshotReferenceImage,
    },
}

#[derive(Debug, Clone)]
pub struct MacosScreenshotReferenceCapture {
    source_id: Arc<str>,
    capture_session_generation: u64,
    references: MacosScreenshotReferenceSet,
}

impl MacosScreenshotReferenceCapture {
    pub(crate) fn new(
        source_id: Arc<str>,
        capture_session_generation: u64,
        references: MacosScreenshotReferenceSet,
    ) -> Self {
        Self {
            source_id,
            capture_session_generation,
            references,
        }
    }

    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    #[must_use]
    pub const fn capture_session_generation(&self) -> u64 {
        self.capture_session_generation
    }

    #[must_use]
    pub const fn references(&self) -> &MacosScreenshotReferenceSet {
        &self.references
    }

    #[must_use]
    pub fn into_references(self) -> MacosScreenshotReferenceSet {
        self.references
    }
}

type SetContentToneMappingInfo = unsafe extern "C-unwind" fn(&CGContext, CGContentToneMappingInfo);
type GetContentAverageLightLevel = unsafe extern "C-unwind" fn(Option<&CGImage>) -> f32;

#[derive(Clone, Copy)]
struct TahoeReferenceOutputSymbols {
    set_tone_mapping: SetContentToneMappingInfo,
    preferred_key: NonNull<CFString>,
    standard_range: NonNull<CFString>,
    constrained_range: NonNull<CFString>,
    high_range: NonNull<CFString>,
    average_light_key: NonNull<CFString>,
}

impl TahoeReferenceOutputSymbols {
    fn load() -> Result<Self, MacosCaptureError> {
        Ok(Self {
            set_tone_mapping: load_required_tahoe_symbol(
                c"CGContextSetContentToneMappingInfo",
                "CGContextSetContentToneMappingInfo",
            )?,
            preferred_key: load_required_tahoe_cf_string(
                c"kCGPreferredDynamicRange",
                "kCGPreferredDynamicRange",
            )?,
            standard_range: load_required_tahoe_cf_string(
                c"kCGDynamicRangeStandard",
                "kCGDynamicRangeStandard",
            )?,
            constrained_range: load_required_tahoe_cf_string(
                c"kCGDynamicRangeConstrained",
                "kCGDynamicRangeConstrained",
            )?,
            high_range: load_required_tahoe_cf_string(
                c"kCGDynamicRangeHigh",
                "kCGDynamicRangeHigh",
            )?,
            average_light_key: load_required_tahoe_cf_string(
                c"kCGContentAverageLightLevel",
                "kCGContentAverageLightLevel",
            )?,
        })
    }

    const fn preferred_range(
        self,
        preferred_dynamic_range: MacosScreenshotPreferredDynamicRange,
    ) -> NonNull<CFString> {
        match preferred_dynamic_range {
            MacosScreenshotPreferredDynamicRange::Standard => self.standard_range,
            MacosScreenshotPreferredDynamicRange::Constrained => self.constrained_range,
            MacosScreenshotPreferredDynamicRange::High => self.high_range,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TahoeReferenceSymbolKind {
    Function,
    CfString,
}

const REQUIRED_TAHOE_REFERENCE_OUTPUT_SYMBOLS: [(&CStr, TahoeReferenceSymbolKind); 8] = [
    (
        c"CGContextGetContentToneMappingInfo",
        TahoeReferenceSymbolKind::Function,
    ),
    (
        c"CGContextSetContentToneMappingInfo",
        TahoeReferenceSymbolKind::Function,
    ),
    (
        c"CGImageGetContentAverageLightLevel",
        TahoeReferenceSymbolKind::Function,
    ),
    (
        c"kCGPreferredDynamicRange",
        TahoeReferenceSymbolKind::CfString,
    ),
    (
        c"kCGDynamicRangeStandard",
        TahoeReferenceSymbolKind::CfString,
    ),
    (
        c"kCGDynamicRangeConstrained",
        TahoeReferenceSymbolKind::CfString,
    ),
    (c"kCGDynamicRangeHigh", TahoeReferenceSymbolKind::CfString),
    (
        c"kCGContentAverageLightLevel",
        TahoeReferenceSymbolKind::CfString,
    ),
];

fn tone_mapping_options(
    preferred_dynamic_range: MacosScreenshotPreferredDynamicRange,
    content_average_light_level: Option<f32>,
    symbols: TahoeReferenceOutputSymbols,
) -> Result<CFRetained<CFDictionary>, MacosCaptureError> {
    let preferred_key = symbols.preferred_key;
    let preferred_value = symbols.preferred_range(preferred_dynamic_range);
    let average_value = content_average_light_level
        .map(|value| {
            // SAFETY: value points to an initialized f32 for this call.
            unsafe {
                CFNumber::new(
                    None,
                    CFNumberType::Float32Type,
                    ptr::from_ref(&value).cast(),
                )
            }
            .ok_or(MacosCaptureError::ScreenshotToneMappingOptionsFailed)
        })
        .transpose()?;
    let mut keys = vec![preferred_key.as_ptr().cast::<c_void>()];
    let mut values = vec![preferred_value.as_ptr().cast::<c_void>()];
    if let Some(value) = average_value.as_ref() {
        keys.push(symbols.average_light_key.as_ptr().cast::<c_void>());
        values.push(NonNull::from(&**value).as_ptr().cast());
    }
    let count = isize::try_from(keys.len()).map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    // SAFETY: keys and values are valid CFType references for this call. The
    // standard callbacks retain them into the immutable dictionary.
    unsafe {
        CFDictionary::new(
            None,
            keys.as_mut_ptr().cast(),
            values.as_mut_ptr().cast(),
            count,
            ptr::from_ref(&kCFTypeDictionaryKeyCallBacks),
            ptr::from_ref(&kCFTypeDictionaryValueCallBacks),
        )
    }
    .ok_or(MacosCaptureError::ScreenshotToneMappingOptionsFailed)
}

fn positive_finite(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

fn load_required_tahoe_cf_string(
    symbol: &std::ffi::CStr,
    capability: &'static str,
) -> Result<NonNull<CFString>, MacosCaptureError> {
    load_tahoe_cf_string(symbol).ok_or(MacosCaptureError::TahoePlatformDefect(capability))
}

fn load_tahoe_cf_string(symbol: &CStr) -> Option<NonNull<CFString>> {
    let slot = load_raw_symbol(symbol)?.cast::<*mut CFString>();
    // SAFETY: Tahoe exports these names as CFStringRef globals. A null value
    // is treated as an absent capability rather than passed to Core Graphics.
    NonNull::new(unsafe { *slot.as_ptr() })
}

fn load_required_tahoe_symbol<T: Copy>(
    symbol: &std::ffi::CStr,
    capability: &'static str,
) -> Result<T, MacosCaptureError> {
    let raw = load_raw_symbol(symbol).ok_or(MacosCaptureError::TahoePlatformDefect(capability))?;
    // SAFETY: callers select T to match the SDK declaration for this symbol.
    Ok(unsafe { std::mem::transmute_copy::<NonNull<c_void>, T>(&raw) })
}

fn load_raw_symbol(symbol: &std::ffi::CStr) -> Option<NonNull<c_void>> {
    #[link(name = "System", kind = "dylib")]
    unsafe extern "C-unwind" {
        fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
    }
    let default_handle = ptr::without_provenance_mut::<c_void>(usize::MAX - 1);
    // SAFETY: RTLD_DEFAULT is the Darwin sentinel at address -2, and the
    // symbol is nul-terminated.
    NonNull::new(unsafe { dlsym(default_handle, symbol.as_ptr()) })
}

fn tahoe_reference_output_symbols_present_with(
    mut present: impl FnMut(&CStr, TahoeReferenceSymbolKind) -> bool,
) -> bool {
    REQUIRED_TAHOE_REFERENCE_OUTPUT_SYMBOLS
        .iter()
        .all(|(symbol, kind)| present(symbol, *kind))
}

pub(crate) fn tahoe_reference_output_symbols_present() -> bool {
    tahoe_reference_output_symbols_present_with(|symbol, kind| match kind {
        TahoeReferenceSymbolKind::Function => load_raw_symbol(symbol).is_some(),
        TahoeReferenceSymbolKind::CfString => load_tahoe_cf_string(symbol).is_some(),
    })
}

pub(crate) fn require_tahoe_reference_output_symbols() -> Result<(), MacosCaptureError> {
    if !tahoe_reference_output_symbols_present() {
        return Err(MacosCaptureError::TahoePlatformDefect(
            "Core Graphics Tahoe reference output",
        ));
    }
    TahoeReferenceOutputSymbols::load().map(drop)
}

fn encode_png(image: &CGImage, path: &Path) -> Result<(), MacosCaptureError> {
    use std::os::unix::ffi::OsStrExt as _;

    #[repr(C)]
    struct ImageDestination(c_void);

    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C-unwind" {
        fn CGImageDestinationCreateWithURL(
            url: &CFURL,
            image_type: &CFString,
            count: usize,
            options: *const CFDictionary,
        ) -> Option<NonNull<ImageDestination>>;
        fn CGImageDestinationAddImage(
            destination: NonNull<ImageDestination>,
            image: &CGImage,
            properties: *const CFDictionary,
        );
        fn CGImageDestinationFinalize(destination: NonNull<ImageDestination>) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C-unwind" {
        fn CFRelease(value: NonNull<c_void>);
    }

    let bytes = OsStr::new(path).as_bytes();
    let byte_len =
        isize::try_from(bytes.len()).map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    // SAFETY: the path byte slice remains live for the duration of this call.
    let url =
        unsafe { CFURL::from_file_system_representation(None, bytes.as_ptr(), byte_len, false) }
            .ok_or(MacosCaptureError::ScreenshotOutputUrlFailed)?;
    let png = CFString::from_static_str("public.png");
    // SAFETY: URL, UTI, and image are retained for the complete destination
    // transaction. ImageIO consumes neither borrowed value.
    let destination = unsafe {
        CGImageDestinationCreateWithURL(&url, &png, 1, ptr::null())
            .ok_or(MacosCaptureError::ScreenshotEncoderCreateFailed)?
    };
    // SAFETY: the destination is live and expects exactly one image.
    unsafe {
        CGImageDestinationAddImage(destination, image, ptr::null());
    }
    // SAFETY: finalization consumes no ownership and is called exactly once.
    let finalized = unsafe { CGImageDestinationFinalize(destination) };
    // SAFETY: the create-rule destination owns one Core Foundation retain.
    unsafe { CFRelease(destination.cast()) };
    if finalized {
        Ok(())
    } else {
        Err(MacosCaptureError::ScreenshotEncodeFailed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    static REFERENCE_TONE_MAPPING_APPLIED: AtomicBool = AtomicBool::new(false);

    unsafe extern "C-unwind" fn record_reference_tone_mapping(
        _context: &CGContext,
        info: CGContentToneMappingInfo,
    ) {
        assert_eq!(info.method, CGToneMapping::ReferenceWhiteBased);
        assert!(!info.options.is_null());
        REFERENCE_TONE_MAPPING_APPLIED.store(true, Ordering::Release);
    }

    #[test]
    fn injected_probe_requires_every_reference_output_symbol() {
        assert!(tahoe_reference_output_symbols_present_with(|_, _| true));

        for (missing_symbol, missing_kind) in REQUIRED_TAHOE_REFERENCE_OUTPUT_SYMBOLS {
            assert!(!tahoe_reference_output_symbols_present_with(
                |symbol, kind| symbol != missing_symbol || kind != missing_kind
            ));
        }
    }

    #[test]
    fn injected_reference_output_applies_reference_white_tone_mapping() {
        let preferred_key = CFString::from_static_str("preferred");
        let standard_range = CFString::from_static_str("standard");
        let constrained_range = CFString::from_static_str("constrained");
        let high_range = CFString::from_static_str("high");
        let average_light_key = CFString::from_static_str("average-light");
        let symbols = TahoeReferenceOutputSymbols {
            set_tone_mapping: record_reference_tone_mapping,
            preferred_key: NonNull::from(&*preferred_key),
            standard_range: NonNull::from(&*standard_range),
            constrained_range: NonNull::from(&*constrained_range),
            high_range: NonNull::from(&*high_range),
            average_light_key: NonNull::from(&*average_light_key),
        };
        let image = MacosScreenshotReferenceImage::new_fixture(MacosCaptureDynamicRange::Sdr, 0x40);
        REFERENCE_TONE_MAPPING_APPLIED.store(false, Ordering::Release);

        let output = image
            .copy_reference_rgba8_with_symbols(
                MacosScreenshotPreferredDynamicRange::Standard,
                symbols,
            )
            .expect("injected reference output should render");

        assert!(REFERENCE_TONE_MAPPING_APPLIED.load(Ordering::Acquire));
        assert_eq!(output.extent, MacosPixelExtent::new(1, 1).expect("extent"));
        assert_eq!(output.bytes_per_row, 4);
        assert_eq!(output.rgba8.len(), 4);
    }

    #[test]
    fn reference_capture_carries_the_fenced_source_identity() {
        let capture = MacosScreenshotReferenceCapture::new(
            Arc::from("display:main"),
            17,
            MacosScreenshotReferenceSet::Sdr {
                image: MacosScreenshotReferenceImage::new_fixture(
                    MacosCaptureDynamicRange::Sdr,
                    0x80,
                ),
            },
        );

        assert_eq!(capture.source_id(), "display:main");
        assert_eq!(capture.capture_session_generation(), 17);
        assert!(matches!(
            capture.references(),
            MacosScreenshotReferenceSet::Sdr { .. }
        ));
        assert!(matches!(
            capture.into_references(),
            MacosScreenshotReferenceSet::Sdr { .. }
        ));
    }
}
