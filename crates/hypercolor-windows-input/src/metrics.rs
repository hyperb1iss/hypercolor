//! Physical monitor topology and cursor sampling under one pinned DPI context.

use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::core::BOOL;

use crate::decode::ScreenRect;
use crate::shared::RawCursor;

const MONITOR_INFO_PRIMARY: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonitorTopology {
    pub monitors: Vec<ScreenRect>,
    pub virtual_screen: ScreenRect,
    pub primary_screen: ScreenRect,
}

struct MonitorEnumeration {
    monitors: Vec<(ScreenRect, bool)>,
    failure: Option<String>,
}

/// Pin this thread to per-monitor-v2 DPI awareness.
pub(crate) fn pin_dpi_context() -> bool {
    // SAFETY: this sets only the current worker thread's DPI context. The
    // worker owns the thread for its lifetime and deliberately never restores
    // the previous context.
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    dpi_context_call_succeeded(previous)
}

fn dpi_context_call_succeeded(previous: DPI_AWARENESS_CONTEXT) -> bool {
    !previous.0.is_null()
}

/// Enumerate physical rectangles without changing the caller's lasting DPI context.
pub(crate) fn physical_monitor_topology() -> Result<MonitorTopology, String> {
    // SAFETY: changes only the current thread and returns its prior context,
    // which is restored before this function returns.
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if !dpi_context_call_succeeded(previous) {
        return Err(windows::core::Error::from_thread().to_string());
    }

    let topology = monitor_topology();
    // SAFETY: `previous` is the valid context cookie returned by the first
    // call on this same thread.
    let restored = unsafe { SetThreadDpiAwarenessContext(previous) };
    if !dpi_context_call_succeeded(restored) {
        return Err(format!(
            "failed to restore caller DPI context: {}",
            windows::core::Error::from_thread()
        ));
    }
    topology
}

/// Enumerate every physical monitor and derive coherent primary and virtual rectangles.
pub(crate) fn monitor_topology() -> Result<MonitorTopology, String> {
    let mut enumeration = MonitorEnumeration {
        monitors: Vec::new(),
        failure: None,
    };
    let data = LPARAM((&raw mut enumeration).expose_provenance().cast_signed());
    // SAFETY: `data` points to `enumeration` for the duration of this
    // synchronous call. The callback validates each monitor before storing it.
    let completed = unsafe { EnumDisplayMonitors(None, None, Some(enumerate_monitor), data) };
    if let Some(failure) = enumeration.failure {
        return Err(failure);
    }
    if !completed.as_bool() {
        return Err(windows::core::Error::from_thread().to_string());
    }
    topology_from_monitors(enumeration.monitors)
        .ok_or_else(|| "Windows reported no valid physical monitors".to_owned())
}

unsafe extern "system" fn enumerate_monitor(
    monitor: HMONITOR,
    _device_context: HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `monitor_topology` passes a live `MonitorEnumeration` pointer and
    // EnumDisplayMonitors invokes this callback synchronously.
    let enumeration: &mut MonitorEnumeration =
        unsafe { &mut *std::ptr::with_exposed_provenance_mut(data.0.cast_unsigned()) };
    let mut info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    // SAFETY: `monitor` is supplied by EnumDisplayMonitors and `info` is a
    // live, correctly sized output structure.
    if !unsafe { GetMonitorInfoW(monitor, &raw mut info) }.as_bool() {
        enumeration.failure = Some(windows::core::Error::from_thread().to_string());
        return BOOL(0);
    }
    let Some(rect) = screen_rect(info.rcMonitor) else {
        enumeration.failure =
            Some("Windows returned an invalid physical monitor rectangle".to_owned());
        return BOOL(0);
    };
    enumeration
        .monitors
        .push((rect, info.dwFlags & MONITOR_INFO_PRIMARY != 0));
    BOOL(1)
}

fn screen_rect(rect: RECT) -> Option<ScreenRect> {
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    (width > 0 && height > 0).then_some(ScreenRect {
        left: rect.left,
        top: rect.top,
        width,
        height,
    })
}

fn topology_from_monitors(monitors: Vec<(ScreenRect, bool)>) -> Option<MonitorTopology> {
    let primary_screen = monitors
        .iter()
        .find_map(|(rect, primary)| primary.then_some(*rect))
        .or_else(|| monitors.first().map(|(rect, _)| *rect))?;
    let left = monitors.iter().map(|(rect, _)| rect.left).min()?;
    let top = monitors.iter().map(|(rect, _)| rect.top).min()?;
    let right = monitors
        .iter()
        .map(|(rect, _)| rect.left.checked_add(rect.width))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .max()?;
    let bottom = monitors
        .iter()
        .map(|(rect, _)| rect.top.checked_add(rect.height))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .max()?;
    let virtual_screen = ScreenRect {
        left,
        top,
        width: right.checked_sub(left)?,
        height: bottom.checked_sub(top)?,
    };
    let monitors = monitors.into_iter().map(|(rect, _)| rect).collect();
    Some(MonitorTopology {
        monitors,
        virtual_screen,
        primary_screen,
    })
}

/// Sample the cursor in physical pixels, normalized against the selected desktop.
pub(crate) fn sample_cursor(virtual_screen: ScreenRect) -> Option<RawCursor> {
    let mut point = POINT::default();
    // SAFETY: `point` is a live, aligned output value. Secure-desktop access
    // failures return an error and publish no partial coordinate.
    unsafe { GetCursorPos(&raw mut point) }.ok()?;
    let (norm_x, norm_y) = virtual_screen.normalize(point.x, point.y);
    Some(RawCursor {
        x: point.x,
        y: point.y,
        norm_x,
        norm_y,
    })
}

#[cfg(test)]
mod tests;
