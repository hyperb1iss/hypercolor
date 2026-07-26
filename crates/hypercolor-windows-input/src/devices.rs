//! Device identity, classification, and the handle cache.
//!
//! `RAWINPUTHEADER.hDevice` is opaque and recycled, so it cannot be an
//! identity on its own. Each handle resolves once to its device interface
//! path, which becomes the `source_id` core unions held state by:
//!
//! ```text
//! \\?\HID#VID_046D&PID_C52B&MI_01&Col01#7&1f2a3b4c&0&0000#{884b96c3-...}
//! ```
//!
//! That path is a **session-local key, not a durable identity**. It usually
//! survives a replug into the same port and it embeds VID/PID, but Windows
//! guarantees nothing across port changes, driver re-enumeration, or a device
//! exposing different collections. Good enough for unioning held state within
//! one capture session; not good enough for durable device fingerprinting,
//! which needs SetupAPI and belongs to a later wave. The raw path is preserved
//! so that work has something to join on.

use std::collections::HashMap;
use std::sync::Arc;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::UI::Input::{
    GetRawInputDeviceInfoW, GetRawInputDeviceList, RAWINPUTDEVICELIST, RID_DEVICE_INFO,
    RIDI_DEVICEINFO, RIDI_DEVICENAME, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
};

use crate::shared::RawDeviceKind;

/// Devices with a null handle share this bucket.
///
/// `hDevice` can legitimately be zero: precision touchpads and some injected
/// input arrive that way, so the cache cannot cover every event and zero is
/// *not* a reliable "this was synthetic" marker. Null handles get one stable
/// source and skip the device-info query entirely.
const UNKNOWN_SOURCE_ID: &str = "windows:unknown";

/// One resolved device.
#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    pub source_id: Arc<str>,
    pub label: String,
    pub kind: RawDeviceKind,
}

/// Handle-to-identity cache, generation-tagged against handle recycling.
///
/// Windows reuses handles after removal. A record still sitting in a buffered
/// drain that references a handle already removed and reissued would otherwise
/// resolve to the *new* device and pour its keys into the old device's held
/// set, so removal bumps a generation and stale lookups are dropped instead.
#[derive(Debug, Default)]
pub struct DeviceCache {
    entries: HashMap<isize, DeviceIdentity>,
    unknown: Option<Arc<str>>,
}

impl DeviceCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of devices currently resolved and streaming.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The bucket every null-handle report shares.
    pub fn unknown_source(&mut self) -> Arc<str> {
        Arc::clone(
            self.unknown
                .get_or_insert_with(|| Arc::from(UNKNOWN_SOURCE_ID)),
        )
    }

    /// Look up a handle, resolving it on first sight.
    ///
    /// Lazy resolution is needed as well as `GIDC_ARRIVAL` handling because a
    /// device can appear in a report before its arrival notification is
    /// processed, and because `RIDEV_DEVNOTIFY` only fires on *change* —
    /// devices already attached at registration produce no arrival at all.
    pub fn resolve(&mut self, handle: HANDLE) -> Option<DeviceIdentity> {
        if handle.is_invalid() {
            return None;
        }
        if let Some(entry) = self.entries.get(&handle.0.addr().cast_signed()) {
            return Some(entry.clone());
        }
        let identity = query_identity(handle)?;
        self.entries
            .insert(handle.0.addr().cast_signed(), identity.clone());
        Some(identity)
    }

    /// Drop a removed handle, returning the identity it held so the caller can
    /// synthesize release edges for it.
    pub fn remove(&mut self, handle: HANDLE) -> Option<DeviceIdentity> {
        self.entries.remove(&handle.0.addr().cast_signed())
    }

    /// Every device currently known, for readiness reporting.
    pub fn identities(&self) -> impl Iterator<Item = &DeviceIdentity> {
        self.entries.values()
    }
}

/// Resolve one handle to its interface path, kind, and synthesized label.
fn query_identity(handle: HANDLE) -> Option<DeviceIdentity> {
    let kind = query_kind(handle)?;
    let path = query_device_path(handle)?;
    let label = synthesize_label(&path, kind);
    Some(DeviceIdentity {
        source_id: Arc::from(path.as_str()),
        label,
        kind,
    })
}

/// Classify via `RIDI_DEVICEINFO`, which reports the type outright — genuinely
/// simpler than evdev's capability sniffing.
fn query_kind(handle: HANDLE) -> Option<RawDeviceKind> {
    let mut info = RID_DEVICE_INFO {
        cbSize: u32::try_from(size_of::<RID_DEVICE_INFO>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut size = info.cbSize;
    // SAFETY: `info` is a live, correctly sized `RID_DEVICE_INFO` with its
    // `cbSize` set as the API requires, and `size` describes that same
    // allocation. The call only writes within it.
    let written = unsafe {
        GetRawInputDeviceInfoW(
            Some(handle),
            RIDI_DEVICEINFO,
            Some(std::ptr::from_mut(&mut info).cast()),
            &raw mut size,
        )
    };
    if written == u32::MAX || written == 0 {
        return None;
    }
    match info.dwType {
        RIM_TYPEKEYBOARD => Some(RawDeviceKind::Keyboard),
        RIM_TYPEMOUSE => Some(RawDeviceKind::Mouse),
        // RIM_TYPEHID and anything else: we never registered for those usages
        // and must never decode one as a keyboard record.
        _ => None,
    }
}

/// Read the device interface path.
///
/// Sizing is in **UTF-16 characters, not bytes** — the query form returns a
/// character count, so the buffer is allocated in `u16`s and the byte-sized
/// intuition would over-allocate by 2× and mis-slice the result.
fn query_device_path(handle: HANDLE) -> Option<String> {
    let mut chars: u32 = 0;
    // SAFETY: the query form takes a null data pointer and only writes the
    // required character count into `chars`.
    let probe =
        unsafe { GetRawInputDeviceInfoW(Some(handle), RIDI_DEVICENAME, None, &raw mut chars) };
    if probe == u32::MAX || chars == 0 {
        return None;
    }

    // The two-call pattern has a real gap: the path can grow between the
    // sizing call and the fetch, and the API then fails rather than
    // truncating. Giving up there would drop the device entirely — it would go
    // on producing input that never resolves to a source id — so a size change
    // is retried with the count the failed call reported. The attempt bound
    // only stops a pathologically racing driver from spinning us forever.
    const MAX_ATTEMPTS: usize = 4;

    for _ in 0..MAX_ATTEMPTS {
        let mut buffer = vec![0u16; chars as usize];
        let mut capacity = chars;
        // SAFETY: `buffer` holds `capacity` UTF-16 units and outlives the call;
        // `capacity` describes exactly that allocation in characters, which is
        // the unit this API sizes in.
        let written = unsafe {
            GetRawInputDeviceInfoW(
                Some(handle),
                RIDI_DEVICENAME,
                Some(buffer.as_mut_ptr().cast()),
                &raw mut capacity,
            )
        };

        if written == u32::MAX {
            // A sizing failure writes the required count back into `capacity`.
            // Anything else, or no growth, is terminal.
            if capacity > chars {
                chars = capacity;
                continue;
            }
            return None;
        }
        if written == 0 {
            return None;
        }

        let len = (written as usize).min(buffer.len());
        let text: String = String::from_utf16_lossy(&buffer[..len]);
        let trimmed = text.trim_end_matches('\0').to_owned();
        return (!trimmed.is_empty()).then_some(trimmed);
    }

    tracing::debug!("raw input device path kept changing size; giving up on this device");
    None
}

/// Build a diagnostic label from the interface path.
///
/// `RIDI_DEVICEINFO` carries no human-readable name — `RID_DEVICE_INFO_KEYBOARD`
/// is type, subtype, mode, and three key counts, and `RID_DEVICE_INFO_MOUSE` is
/// id, button count, sample rate, and a horizontal-wheel flag. A friendly
/// product name would mean a SetupAPI dependency this wave does not need, so
/// the label is synthesized from VID/PID and presented as exactly that.
///
/// Terminal Services virtual devices are labeled rather than filtered: an RDP
/// session is a legitimate way to use the machine, and the session probe
/// already refuses to condemn RDP users. Labeling keeps them from looking like
/// phantom hardware in diagnostics.
fn synthesize_label(path: &str, kind: RawDeviceKind) -> String {
    let kind_name = match kind {
        RawDeviceKind::Keyboard => "keyboard",
        RawDeviceKind::Mouse => "mouse",
    };
    let upper = path.to_ascii_uppercase();
    if upper.contains("RDP_MOU") || upper.contains("RDP_KBD") {
        return format!("Remote Desktop virtual {kind_name}");
    }
    match (extract_field(&upper, "VID_"), extract_field(&upper, "PID_")) {
        (Some(vid), Some(pid)) => format!("HID {kind_name} {vid}:{pid}"),
        _ => format!("HID {kind_name}"),
    }
}

/// Pull the four hex digits following a `VID_`/`PID_` marker.
fn extract_field(upper_path: &str, marker: &str) -> Option<String> {
    let start = upper_path.find(marker)? + marker.len();
    let value: String = upper_path[start..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .take(4)
        .collect();
    (value.len() == 4).then_some(value)
}

/// Enumerate devices already attached, so their arrival is not missed.
///
/// `RIDEV_DEVNOTIFY` fires only on *change*, so an already-attached mouse
/// produces no `GIDC_ARRIVAL`. Resolving lazily on first input would leave
/// core with no pointer at all until the user moved it, whereas the Linux
/// backend establishes pointer presence from the device list at scan time.
pub fn enumerate_devices(keyboard: bool, mouse: bool) -> Vec<(HANDLE, DeviceIdentity)> {
    let mut count: u32 = 0;
    let entry_size = u32::try_from(size_of::<RAWINPUTDEVICELIST>()).unwrap_or(u32::MAX);
    // SAFETY: the query form takes a null list pointer and only writes the
    // device count into `count`.
    let probe = unsafe { GetRawInputDeviceList(None, &raw mut count, entry_size) };
    if probe == u32::MAX || count == 0 {
        return Vec::new();
    }

    let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
    // SAFETY: `list` holds `count` entries and outlives the call; `count`
    // describes exactly that allocation and `entry_size` the element stride.
    let written =
        unsafe { GetRawInputDeviceList(Some(list.as_mut_ptr()), &raw mut count, entry_size) };
    if written == u32::MAX {
        return Vec::new();
    }

    list.truncate((written as usize).min(list.len()));
    list.into_iter()
        .filter_map(|entry| {
            let identity = query_identity(entry.hDevice)?;
            let wanted = match identity.kind {
                RawDeviceKind::Keyboard => keyboard,
                RawDeviceKind::Mouse => mouse,
            };
            wanted.then_some((entry.hDevice, identity))
        })
        .collect()
}

/// Seed a cache from an enumeration result.
pub fn seed_cache(cache: &mut DeviceCache, devices: Vec<(HANDLE, DeviceIdentity)>) {
    for (handle, identity) in devices {
        cache
            .entries
            .insert(handle.0.addr().cast_signed(), identity);
    }
}
