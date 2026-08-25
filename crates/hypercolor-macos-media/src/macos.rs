use std::ffi::c_void;
use std::fmt::Write as _;
use std::mem::{self, MaybeUninit};
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread as _, define_class, msg_send};
use objc2_app_kit::NSRunningApplication;
use objc2_core_foundation::{CFBoolean, CFString};
use objc2_core_services::{
    AECreateDesc, AEDesc, AEDeterminePermissionToAutomateTarget, AEDisposeDesc, AppleEvent,
    errAEEventNotPermitted, errAEEventWouldRequireUserConsent, kAECoreSuite, kAEGetData,
    typeKernelProcessID,
};
use objc2_foundation::{
    NSAppleEventDescriptor, NSBundle, NSData, NSError, NSNumber, NSObject, NSObjectProtocol,
    NSString,
};
use objc2_scripting_bridge::{SBApplication, SBApplicationDelegate, SBObject};
use objc2_security::SecTask;
use sha2::{Digest as _, Sha256};

use crate::shared::{
    AdapterFailure, Artwork, AutomationBackend, Capability, DeferredArtworkLoader,
    DeferredArtworkSource, LoadedArtwork, MediaAdapter, MediaError, MediaErrorKind,
    MediaPlayerSnapshot, MediaPoll, PlaybackStatus,
};

const MAX_ARTWORK_BYTES: usize = 8 * 1024 * 1024;
const PROCESS_NOT_FOUND: i32 = -600;
const APPLE_EVENT_TIMEOUT_TICKS: std::ffi::c_long = 24;
static AUTOMATION_CAPABILITY: OnceLock<Capability> = OnceLock::new();

pub(crate) struct NativeAutomationBackend;

impl NativeAutomationBackend {
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl AutomationBackend for NativeAutomationBackend {
    fn capability(&self) -> Capability {
        automation_capability()
    }

    fn connect(&mut self) -> Result<(), MediaError> {
        require_automation_capability()
    }

    fn request_authorization(&mut self, adapter: MediaAdapter) -> Result<(), MediaError> {
        require_automation_capability()?;
        autoreleasepool(|_| {
            let applications = NSRunningApplication::runningApplicationsWithBundleIdentifier(
                &NSString::from_str(adapter.bundle_id()),
            );
            if applications.is_empty() {
                return Err(MediaError::new(
                    MediaErrorKind::NoRunningCapablePlayer,
                    Some(adapter),
                    format!("{} is not running", adapter.display_name()),
                ));
            }
            preflight_permission(
                adapter,
                applications.objectAtIndex(0).processIdentifier(),
                true,
            )
        })
    }

    fn poll(&mut self) -> Result<MediaPoll, MediaError> {
        require_automation_capability()?;
        autoreleasepool(|_| poll_running_players())
    }

    fn disconnect(&mut self) {}
}

fn automation_capability() -> Capability {
    AUTOMATION_CAPABILITY
        .get_or_init(detect_automation_capability)
        .clone()
}

fn detect_automation_capability() -> Capability {
    let bundle = NSBundle::mainBundle();
    if !bundle.bundlePath().to_string().ends_with(".app")
        || bundle
            .bundleIdentifier()
            .is_none_or(|identifier| identifier.to_string().trim().is_empty())
        || !responsible_code_is_valid()
        || !has_automation_entitlement()
    {
        return Capability::IneligibleResponsibleBundle;
    }
    let key = NSString::from_str("NSAppleEventsUsageDescription");
    let Some(value) = bundle.objectForInfoDictionaryKey(&key) else {
        return Capability::MissingUsageDescription;
    };
    let Ok(description) = value.downcast::<NSString>() else {
        return Capability::MissingUsageDescription;
    };
    if description.to_string().trim().is_empty() {
        Capability::MissingUsageDescription
    } else {
        Capability::Available
    }
}

fn require_automation_capability() -> Result<(), MediaError> {
    match automation_capability() {
        Capability::Available => Ok(()),
        Capability::MissingUsageDescription => Err(MediaError::new(
            MediaErrorKind::UnsupportedCapability,
            None,
            "the responsible macOS bundle lacks NSAppleEventsUsageDescription",
        )),
        Capability::IneligibleResponsibleBundle => Err(MediaError::new(
            MediaErrorKind::UnsupportedCapability,
            None,
            "the responsible process is not a valid signed macOS application bundle",
        )),
        Capability::UnsupportedPlatform => Err(MediaError::new(
            MediaErrorKind::UnsupportedCapability,
            None,
            "macOS media Automation is unavailable on this platform",
        )),
    }
}

fn poll_running_players() -> Result<MediaPoll, MediaError> {
    let mut targets = Vec::new();
    let mut players = Vec::new();
    let mut failures = Vec::new();

    for adapter in [MediaAdapter::Music, MediaAdapter::Spotify] {
        let applications = NSRunningApplication::runningApplicationsWithBundleIdentifier(
            &NSString::from_str(adapter.bundle_id()),
        );
        if applications.is_empty() {
            continue;
        }
        targets.push((adapter, applications.objectAtIndex(0).processIdentifier()));
    }
    if targets.is_empty() {
        return Ok(MediaPoll::NoRunningCapablePlayer);
    }

    std::thread::scope(|scope| {
        let polls = targets
            .into_iter()
            .map(|(adapter, process_id)| {
                (
                    adapter,
                    scope.spawn(move || autoreleasepool(|_| poll_adapter(adapter, process_id))),
                )
            })
            .collect::<Vec<_>>();
        for (adapter, poll) in polls {
            let result = poll
                .join()
                .unwrap_or_else(|_| Err(adapter_failure(adapter, "media adapter worker panicked")));
            match result {
                Ok(Some(snapshot)) => players.push(snapshot),
                Ok(None) => {}
                Err(error) => failures.push(AdapterFailure { adapter, error }),
            }
        }
    });
    Ok(MediaPoll::Players { players, failures })
}

fn poll_adapter(
    adapter: MediaAdapter,
    process_id: i32,
) -> Result<Option<MediaPlayerSnapshot>, MediaError> {
    preflight_permission(adapter, process_id, false)?;
    // SAFETY: The PID came from a live NSRunningApplication instance in this pool.
    let application = unsafe { SBApplication::applicationWithProcessIdentifier(process_id) }
        .ok_or_else(|| stale_target(adapter))?;
    let delegate = ScriptingBridgeDelegate::new();
    // SAFETY: The delegate implements SBApplicationDelegate and outlives this poll.
    unsafe {
        application.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        application.setTimeout(APPLE_EVENT_TIMEOUT_TICKS);
    }

    let status = property_value(&application, code(*b"pPlS"), adapter)
        .and_then(|value| playback_status(&value, adapter))?;
    if status == PlaybackStatus::Stopped {
        return Ok(None);
    }

    let track_value = property_value(&application, code(*b"pTrk"), adapter)?;
    let track = track_value
        .downcast::<SBObject>()
        .map_err(|_| adapter_failure(adapter, "current track was not a scriptable object"))?;
    let track_id_code = match adapter {
        MediaAdapter::Music => code(*b"pPIS"),
        MediaAdapter::Spotify => code(*b"ID  "),
    };
    let track_id = string_property(&track, track_id_code, adapter)?;
    let artwork = Some(Artwork::Deferred(DeferredArtworkSource::with_loader(
        format!("{}\u{1f}{track_id}", adapter.bundle_id()),
        Arc::new(MacosArtworkLoader {
            adapter,
            process_id,
            track_id: track_id.clone(),
        }),
    )));

    Ok(Some(MediaPlayerSnapshot {
        player_id: adapter.bundle_id().to_owned(),
        track_id,
        status,
        track: string_property(&track, code(*b"pnam"), adapter)?,
        artist: string_property(&track, code(*b"pArt"), adapter)?,
        album: string_property(&track, code(*b"pAlb"), adapter)?,
        artwork,
        position_ms: milliseconds(number_property(&application, code(*b"pPos"), adapter)?),
        duration_ms: milliseconds(number_property(&track, code(*b"pDur"), adapter)?),
    }))
}

fn preflight_permission(
    adapter: MediaAdapter,
    process_id: i32,
    ask_user_if_needed: bool,
) -> Result<(), MediaError> {
    let mut target = MaybeUninit::<AEDesc>::uninit();
    // SAFETY: Core Services copies the in-scope PID bytes into the output descriptor.
    let create_status = unsafe {
        AECreateDesc(
            typeKernelProcessID,
            (&raw const process_id).cast::<c_void>(),
            mem::size_of_val(&process_id)
                .try_into()
                .expect("process identifier size fits Core Services Size"),
            target.as_mut_ptr(),
        )
    };
    if create_status != 0 {
        return Err(adapter_failure(
            adapter,
            format!("could not create Apple Event target descriptor ({create_status})"),
        ));
    }
    // SAFETY: A zero create status guarantees Core Services initialized the descriptor.
    let mut target = unsafe { target.assume_init() };
    // SAFETY: The initialized descriptor remains live through this permission query.
    let permission_status = unsafe {
        AEDeterminePermissionToAutomateTarget(
            &raw const target,
            kAECoreSuite,
            kAEGetData,
            ask_user_if_needed,
        )
    };
    // SAFETY: The initialized descriptor is disposed exactly once after the query.
    let dispose_status = unsafe { AEDisposeDesc(&raw mut target) };
    if dispose_status != 0 {
        return Err(adapter_failure(
            adapter,
            format!("could not dispose Apple Event target descriptor ({dispose_status})"),
        ));
    }
    match permission_status {
        0 => Ok(()),
        status if status == errAEEventNotPermitted => Err(MediaError::new(
            MediaErrorKind::AuthorizationDenied,
            Some(adapter),
            format!("Automation access to {} was denied", adapter.display_name()),
        )),
        status if status == errAEEventWouldRequireUserConsent => Err(MediaError::new(
            MediaErrorKind::AuthorizationRequired,
            Some(adapter),
            format!(
                "Automation access to {} requires explicit user consent",
                adapter.display_name()
            ),
        )),
        PROCESS_NOT_FOUND => Err(stale_target(adapter)),
        status => Err(adapter_failure(
            adapter,
            format!("Automation permission preflight failed ({status})"),
        )),
    }
}

fn property_value(
    object: &SBObject,
    property_code: u32,
    adapter: MediaAdapter,
) -> Result<Retained<AnyObject>, MediaError> {
    // SAFETY: Property codes come from the installed public application dictionaries.
    let property = unsafe { object.propertyWithCode(property_code) };
    // SAFETY: The retained property proxy and its target application are live here.
    unsafe { property.get() }.ok_or_else(|| scripting_error(&property, adapter))
}

fn string_property(
    object: &SBObject,
    property_code: u32,
    adapter: MediaAdapter,
) -> Result<String, MediaError> {
    property_value(object, property_code, adapter).and_then(|value| {
        value
            .downcast::<NSString>()
            .map(|text| text.to_string())
            .map_err(|_| adapter_failure(adapter, "scripted property was not text"))
    })
}

fn number_property(
    object: &SBObject,
    property_code: u32,
    adapter: MediaAdapter,
) -> Result<f64, MediaError> {
    property_value(object, property_code, adapter).and_then(|value| {
        value
            .downcast::<NSNumber>()
            .map(|number| number.as_f64())
            .map_err(|_| adapter_failure(adapter, "scripted property was not numeric"))
    })
}

fn playback_status(value: &AnyObject, adapter: MediaAdapter) -> Result<PlaybackStatus, MediaError> {
    if let Some(number) = value.downcast_ref::<NSNumber>() {
        return match number.as_i64() {
            value if value == 0 || value == i64::from(code(*b"kPSS")) => {
                Ok(PlaybackStatus::Stopped)
            }
            value if value == 1 || value == i64::from(code(*b"kPSP")) => {
                Ok(PlaybackStatus::Playing)
            }
            value if value == 2 || value == i64::from(code(*b"kPSp")) => Ok(PlaybackStatus::Paused),
            _ => Err(adapter_failure(adapter, "unknown numeric player state")),
        };
    }
    if let Some(descriptor) = value.downcast_ref::<NSAppleEventDescriptor>() {
        return match descriptor.enumCodeValue() {
            value if value == code(*b"kPSS") => Ok(PlaybackStatus::Stopped),
            value if value == code(*b"kPSP") => Ok(PlaybackStatus::Playing),
            value if value == code(*b"kPSp") => Ok(PlaybackStatus::Paused),
            _ => Err(adapter_failure(adapter, "unknown Apple Event player state")),
        };
    }
    Err(adapter_failure(
        adapter,
        "scripted player state had an invalid type",
    ))
}

struct MacosArtworkLoader {
    adapter: MediaAdapter,
    process_id: i32,
    track_id: String,
}

impl DeferredArtworkLoader for MacosArtworkLoader {
    fn load(&self, max_bytes: usize) -> Result<Option<LoadedArtwork>, MediaError> {
        autoreleasepool(|_| load_artwork(self, max_bytes))
    }
}

fn load_artwork(
    source: &MacosArtworkLoader,
    max_bytes: usize,
) -> Result<Option<LoadedArtwork>, MediaError> {
    preflight_permission(source.adapter, source.process_id, false)?;
    // SAFETY: The PID identifies the already-running application captured with the snapshot.
    let application = unsafe { SBApplication::applicationWithProcessIdentifier(source.process_id) }
        .ok_or_else(|| stale_target(source.adapter))?;
    let delegate = ScriptingBridgeDelegate::new();
    // SAFETY: The delegate implements SBApplicationDelegate and outlives this load.
    unsafe {
        application.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        application.setTimeout(APPLE_EVENT_TIMEOUT_TICKS);
    }
    let track = property_value(&application, code(*b"pTrk"), source.adapter)?
        .downcast::<SBObject>()
        .map_err(|_| adapter_failure(source.adapter, "current track was not scriptable"))?;
    let track_id_code = match source.adapter {
        MediaAdapter::Music => code(*b"pPIS"),
        MediaAdapter::Spotify => code(*b"ID  "),
    };
    let current_track_id = string_property(&track, track_id_code, source.adapter)?;
    if current_track_id != source.track_id {
        return Err(MediaError::new(
            MediaErrorKind::StaleTarget,
            Some(source.adapter),
            format!(
                "{} changed tracks before artwork capture",
                source.adapter.display_name()
            ),
        ));
    }
    match source.adapter {
        MediaAdapter::Music => music_artwork(&track, source.adapter, max_bytes),
        MediaAdapter::Spotify => spotify_artwork(&track, source.adapter),
    }
}

fn spotify_artwork(
    track: &SBObject,
    adapter: MediaAdapter,
) -> Result<Option<LoadedArtwork>, MediaError> {
    let value = property_value(track, code(*b"aUrl"), adapter)?;
    let url = value
        .downcast::<NSString>()
        .map_err(|_| adapter_failure(adapter, "Spotify artwork URL was not text"))?
        .to_string();
    if url.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(LoadedArtwork::Url(url)))
    }
}

fn music_artwork(
    track: &SBObject,
    adapter: MediaAdapter,
    max_bytes: usize,
) -> Result<Option<LoadedArtwork>, MediaError> {
    // SAFETY: cArt is Music's public artwork element code for a live track proxy.
    let artworks = unsafe { track.elementArrayWithCode(code(*b"cArt")) };
    if artworks.is_empty() {
        return Ok(None);
    }
    let artwork = artworks.objectAtIndex(0);
    let artwork = artwork
        .downcast::<SBObject>()
        .map_err(|_| adapter_failure(adapter, "Music artwork was not scriptable"))?;
    let value = property_value(&artwork, code(*b"pRaw"), adapter)?;
    let bytes = if let Some(data) = value.downcast_ref::<NSData>() {
        bounded_artwork_bytes(data, adapter, max_bytes)?
    } else if let Some(descriptor) = value.downcast_ref::<NSAppleEventDescriptor>() {
        bounded_artwork_bytes(&descriptor.data(), adapter, max_bytes)?
    } else {
        return Err(adapter_failure(
            adapter,
            "Music artwork was not binary data",
        ));
    };
    Ok(Some(LoadedArtwork::Bytes {
        identity: artwork_identity(&string_property(track, code(*b"pPIS"), adapter)?, &bytes),
        data: Arc::from(bytes),
    }))
}

fn bounded_artwork_bytes(
    data: &NSData,
    adapter: MediaAdapter,
    max_bytes: usize,
) -> Result<Vec<u8>, MediaError> {
    validate_artwork_size(data.length(), adapter, max_bytes)?;
    Ok(data.to_vec())
}

fn validate_artwork_size(
    length: usize,
    adapter: MediaAdapter,
    max_bytes: usize,
) -> Result<(), MediaError> {
    let limit = max_bytes.min(MAX_ARTWORK_BYTES);
    if length > limit {
        return Err(adapter_failure(
            adapter,
            format!("Music artwork exceeded {limit} bytes"),
        ));
    }
    Ok(())
}

fn scripting_error(object: &SBObject, adapter: MediaAdapter) -> MediaError {
    // SAFETY: The object proxy remains retained after the failed property evaluation.
    let error = unsafe { object.lastError() };
    let Some(error) = error else {
        return adapter_failure(adapter, "scripted property returned no value");
    };
    classify_error(
        adapter,
        error.code(),
        error.localizedDescription().to_string(),
    )
}

fn classify_error(adapter: MediaAdapter, code: isize, message: String) -> MediaError {
    let kind = match code {
        -1743 => MediaErrorKind::AuthorizationDenied,
        -1744 => MediaErrorKind::AuthorizationRequired,
        -600 => MediaErrorKind::StaleTarget,
        -1712 => MediaErrorKind::TimedOut,
        _ => MediaErrorKind::AdapterFailure,
    };
    MediaError::new(kind, Some(adapter), message)
}

fn milliseconds(seconds: f64) -> u64 {
    Duration::try_from_secs_f64(seconds.max(0.0)).map_or(0, |duration| {
        duration.as_millis().try_into().unwrap_or(u64::MAX)
    })
}

const fn code(value: [u8; 4]) -> u32 {
    u32::from_be_bytes(value)
}

fn stale_target(adapter: MediaAdapter) -> MediaError {
    MediaError::new(
        MediaErrorKind::StaleTarget,
        Some(adapter),
        format!(
            "{} terminated before its media snapshot completed",
            adapter.display_name()
        ),
    )
}

fn adapter_failure(adapter: MediaAdapter, message: impl Into<String>) -> MediaError {
    MediaError::new(MediaErrorKind::AdapterFailure, Some(adapter), message)
}

fn artwork_identity(track_id: &str, data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut identity = String::with_capacity(track_id.len() + 65);
    identity.push_str(track_id);
    identity.push('\u{1f}');
    for byte in digest {
        write!(&mut identity, "{byte:02x}").expect("writing to String cannot fail");
    }
    identity
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "HypercolorScriptingBridgeDelegate"]
    struct ScriptingBridgeDelegate;

    // SAFETY: NSObjectProtocol adds no requirements beyond the NSObject superclass.
    unsafe impl NSObjectProtocol for ScriptingBridgeDelegate {}

    // SAFETY: The implemented selector exactly matches SBApplicationDelegate.
    unsafe impl SBApplicationDelegate for ScriptingBridgeDelegate {
        #[unsafe(method_id(eventDidFail:withError:))]
        #[allow(non_snake_case)]
        unsafe fn eventDidFail_withError(
            &self,
            _event: NonNull<AppleEvent>,
            _error: &NSError,
        ) -> Option<Retained<AnyObject>> {
            None
        }
    }
);

impl ScriptingBridgeDelegate {
    fn new() -> Retained<Self> {
        // SAFETY: NSObject has no additional initialization requirements.
        unsafe { msg_send![super(Self::alloc().set_ivars(())), init] }
    }
}

fn responsible_code_is_valid() -> bool {
    let mut code = std::ptr::null();
    // SAFETY: Security writes one retained code reference into the valid out pointer.
    let copy_status = unsafe { SecCodeCopySelf(0, &raw mut code) };
    if copy_status != 0 || code.is_null() {
        return false;
    }
    // SAFETY: A successful copy returned a non-null live SecCode reference.
    let validity_status = unsafe { SecCodeCheckValidity(code, 0, std::ptr::null()) };
    // SAFETY: SecCodeCopySelf returned this retained Core Foundation object.
    unsafe { CFRelease(code) };
    validity_status == 0
}

fn has_automation_entitlement() -> bool {
    // SAFETY: the Security framework creates a retained task for this process.
    let Some(task) = (unsafe { SecTask::from_self(None) }) else {
        return false;
    };
    let entitlement = CFString::from_static_str("com.apple.security.automation.apple-events");
    // SAFETY: the task and entitlement are live, and a null error pointer opts
    // out of receiving an owned CFError.
    let Some(value) = (unsafe { task.value_for_entitlement(&entitlement, std::ptr::null_mut()) })
    else {
        return false;
    };
    value
        .downcast_ref::<CFBoolean>()
        .is_some_and(CFBoolean::as_bool)
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecCodeCopySelf(flags: u32, code: *mut *const c_void) -> i32;
    fn SecCodeCheckValidity(code: *const c_void, flags: u32, requirement: *const c_void) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *const c_void);
}

#[cfg(test)]
mod tests {
    use objc2_foundation::NSNumber;

    use super::{
        MAX_ARTWORK_BYTES, artwork_identity, code, playback_status, validate_artwork_size,
    };
    use crate::{MediaAdapter, MediaErrorKind, PlaybackStatus};

    #[test]
    fn player_state_accepts_cocoa_ordinals_and_apple_event_codes() {
        for (raw, expected) in [
            (0, PlaybackStatus::Stopped),
            (1, PlaybackStatus::Playing),
            (2, PlaybackStatus::Paused),
            (i64::from(code(*b"kPSS")), PlaybackStatus::Stopped),
            (i64::from(code(*b"kPSP")), PlaybackStatus::Playing),
            (i64::from(code(*b"kPSp")), PlaybackStatus::Paused),
        ] {
            let value = NSNumber::new_i64(raw);
            assert_eq!(
                playback_status(&value, MediaAdapter::Music).expect("known state decodes"),
                expected
            );
        }
    }

    #[test]
    fn embedded_artwork_identity_is_stable_and_content_sensitive() {
        assert_eq!(
            artwork_identity("track", b"same"),
            artwork_identity("track", b"same")
        );
        assert_ne!(
            artwork_identity("track", b"first"),
            artwork_identity("track", b"second")
        );
    }

    #[test]
    fn native_artwork_allocation_is_bounded_before_copying() {
        assert!(
            validate_artwork_size(MAX_ARTWORK_BYTES, MediaAdapter::Music, MAX_ARTWORK_BYTES)
                .is_ok()
        );
        assert_eq!(
            validate_artwork_size(
                MAX_ARTWORK_BYTES + 1,
                MediaAdapter::Music,
                MAX_ARTWORK_BYTES,
            )
            .expect_err("oversized native data is rejected before copying")
            .kind(),
            MediaErrorKind::AdapterFailure
        );
    }
}
