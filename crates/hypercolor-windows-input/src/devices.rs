//! Device identity, classification, and the handle cache.
//!
//! `RAWINPUTHEADER.hDevice` is opaque and recycled, so it cannot be an
//! identity on its own. Each native lifetime gets a monotonic generation and
//! an interned descriptor containing its device interface path:
//!
//! ```text
//! \\?\HID#VID_046D&PID_C52B&MI_01&Col01#7&1f2a3b4c&0&0000#{884b96c3-...}
//! ```
//!
//! The path is a **session-local key, not a durable identity**. It usually
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

use crate::shared::{RawDeviceDescriptor, RawDeviceKind};

#[derive(Debug)]
pub(crate) struct DeviceMetadata {
    interface_path: Arc<str>,
    label: Arc<str>,
    kind: RawDeviceKind,
}

#[derive(Debug)]
pub(crate) struct DeviceResolution {
    pub(crate) device: Arc<RawDeviceDescriptor>,
    pub(crate) retired: Option<Arc<RawDeviceDescriptor>>,
    pub(crate) discovered: bool,
    pub(crate) metadata_changed: bool,
}

/// Handle-to-identity cache, generation-tagged against handle recycling.
///
/// Windows reuses handles after removal. A record still sitting in a buffered
/// drain that references a handle already removed and reissued would otherwise
/// resolve to the *new* device and pour its keys into the old device's held
/// set, so removal bumps a generation and stale lookups are dropped instead.
#[derive(Debug)]
pub struct DeviceCache {
    entries: HashMap<isize, Arc<RawDeviceDescriptor>>,
    null_keyboard: Option<Arc<RawDeviceDescriptor>>,
    null_mouse: Option<Arc<RawDeviceDescriptor>>,
    session_generation: u64,
    next_device_generation: u64,
}

impl DeviceCache {
    #[must_use]
    pub fn new(session_generation: u64) -> Self {
        Self {
            entries: HashMap::new(),
            null_keyboard: None,
            null_mouse: None,
            session_generation,
            next_device_generation: 1,
        }
    }

    /// Number of devices currently resolved and streaming.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
            + usize::from(self.null_keyboard.is_some())
            + usize::from(self.null_mouse.is_some())
    }

    /// Resolve a kind-safe null-handle device for this session.
    pub fn resolve_null(&mut self, kind: RawDeviceKind) -> DeviceResolution {
        let existing = match kind {
            RawDeviceKind::Keyboard => self.null_keyboard.as_ref(),
            RawDeviceKind::Mouse => self.null_mouse.as_ref(),
        };
        if let Some(existing) = existing {
            return DeviceResolution {
                device: Arc::clone(existing),
                retired: None,
                discovered: false,
                metadata_changed: false,
            };
        }

        let device_generation = self.allocate_device_generation();
        let kind_name = kind_name(kind);
        let device = Arc::new(RawDeviceDescriptor {
            source_id: Arc::from(format!(
                "windows:null:{kind_name}:s{}:d{device_generation}",
                self.session_generation
            )),
            interface_path: None,
            label: Arc::from(format!("Windows null-device {kind_name}")),
            kind,
            session_generation: self.session_generation,
            device_generation,
        });
        match kind {
            RawDeviceKind::Keyboard => self.null_keyboard = Some(Arc::clone(&device)),
            RawDeviceKind::Mouse => self.null_mouse = Some(Arc::clone(&device)),
        }
        DeviceResolution {
            device,
            retired: None,
            discovered: true,
            metadata_changed: false,
        }
    }

    /// Look up a handle, resolving it on first sight.
    ///
    /// Lazy resolution is needed as well as `GIDC_ARRIVAL` handling because a
    /// device can appear in a report before its arrival notification is
    /// processed, and because `RIDEV_DEVNOTIFY` only fires on *change* —
    /// devices already attached at registration produce no arrival at all.
    pub fn resolve(
        &mut self,
        handle: HANDLE,
        expected_kind: RawDeviceKind,
    ) -> Option<DeviceResolution> {
        if handle.is_invalid() {
            return Some(self.resolve_null(expected_kind));
        }
        if let Some(entry) = self.entries.get(&handle.0.addr().cast_signed())
            && entry.kind == expected_kind
        {
            return Some(DeviceResolution {
                device: Arc::clone(entry),
                retired: None,
                discovered: false,
                metadata_changed: false,
            });
        }
        let metadata = query_metadata(handle)?;
        if metadata.kind != expected_kind {
            return None;
        }
        Some(self.install(handle, metadata))
    }

    /// Re-read an arrival notification's metadata without duplicating an
    /// already synthesized first-data arrival.
    pub fn refresh(&mut self, handle: HANDLE) -> Option<DeviceResolution> {
        if handle.is_invalid() {
            return None;
        }
        let metadata = query_metadata(handle)?;
        Some(self.refresh_metadata(handle, metadata))
    }

    /// Drop a removed handle, returning the identity it held so the caller can
    /// synthesize release edges for it.
    pub fn remove(&mut self, handle: HANDLE) -> Option<Arc<RawDeviceDescriptor>> {
        self.entries.remove(&handle.0.addr().cast_signed())
    }

    /// Every device currently known, for readiness reporting.
    pub fn identities(&self) -> impl Iterator<Item = &Arc<RawDeviceDescriptor>> {
        self.entries.values()
    }

    fn install(&mut self, handle: HANDLE, metadata: DeviceMetadata) -> DeviceResolution {
        let device_generation = self.allocate_device_generation();
        let kind_name = kind_name(metadata.kind);
        let device = Arc::new(RawDeviceDescriptor {
            source_id: Arc::from(format!(
                "windows:{kind_name}:s{}:d{device_generation}:{}",
                self.session_generation, metadata.interface_path
            )),
            interface_path: Some(metadata.interface_path),
            label: metadata.label,
            kind: metadata.kind,
            session_generation: self.session_generation,
            device_generation,
        });
        let retired = self
            .entries
            .insert(handle.0.addr().cast_signed(), Arc::clone(&device));
        DeviceResolution {
            device,
            retired,
            discovered: true,
            metadata_changed: false,
        }
    }

    fn refresh_metadata(&mut self, handle: HANDLE, metadata: DeviceMetadata) -> DeviceResolution {
        let key = handle.0.addr().cast_signed();
        let Some(existing) = self.entries.get(&key) else {
            return self.install(handle, metadata);
        };
        if existing.kind != metadata.kind
            || existing.interface_path.as_ref() != Some(&metadata.interface_path)
        {
            return self.install(handle, metadata);
        }
        if existing.label == metadata.label {
            return DeviceResolution {
                device: Arc::clone(existing),
                retired: None,
                discovered: false,
                metadata_changed: false,
            };
        }

        let device = Arc::new(RawDeviceDescriptor {
            source_id: Arc::clone(&existing.source_id),
            interface_path: existing.interface_path.clone(),
            label: metadata.label,
            kind: existing.kind,
            session_generation: existing.session_generation,
            device_generation: existing.device_generation,
        });
        self.entries.insert(key, Arc::clone(&device));
        DeviceResolution {
            device,
            retired: None,
            discovered: false,
            metadata_changed: true,
        }
    }

    fn allocate_device_generation(&mut self) -> u64 {
        let generation = self.next_device_generation;
        self.next_device_generation = self
            .next_device_generation
            .checked_add(1)
            .expect("Raw Input device generation exhausted");
        generation
    }
}

/// Resolve one handle to its interface path, kind, and synthesized label.
fn query_metadata(handle: HANDLE) -> Option<DeviceMetadata> {
    let kind = query_kind(handle)?;
    let path = query_device_path(handle)?;
    Some(DeviceMetadata {
        label: Arc::from(synthesize_label(&path, kind)),
        interface_path: Arc::from(path),
        kind,
    })
}

const fn kind_name(kind: RawDeviceKind) -> &'static str {
    match kind {
        RawDeviceKind::Keyboard => "keyboard",
        RawDeviceKind::Mouse => "mouse",
    }
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
pub(crate) fn enumerate_devices(keyboard: bool, mouse: bool) -> Vec<(HANDLE, DeviceMetadata)> {
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
            let metadata = query_metadata(entry.hDevice)?;
            let wanted = match metadata.kind {
                RawDeviceKind::Keyboard => keyboard,
                RawDeviceKind::Mouse => mouse,
            };
            wanted.then_some((entry.hDevice, metadata))
        })
        .collect()
}

/// Seed a cache from an enumeration result.
pub(crate) fn seed_cache(cache: &mut DeviceCache, devices: Vec<(HANDLE, DeviceMetadata)>) {
    for (handle, metadata) in devices {
        cache.install(handle, metadata);
    }
}

#[cfg(test)]
mod tests;
