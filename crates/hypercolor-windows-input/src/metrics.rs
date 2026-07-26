//! Screen geometry and cursor sampling, under one pinned DPI context.
//!
//! The hazard here is not DPI scaling itself but *mixing* contexts between two
//! reads. `GetSystemMetrics` is explicitly not DPI-aware for per-monitor-aware
//! callers, `GetCursorPos` can return logical coordinates, and both follow the
//! *calling thread's* DPI context — which a worker inherits from the process
//! default but which a `SetThreadDpiAwarenessContext` call elsewhere can
//! change out from under it. Reading the cursor under one context and the
//! screen rect under another yields a normalized ratio that is simply wrong.
//!
//! So the worker pins per-monitor-v2 as its first act and reads both under it.
//! If pinning fails, normalization stays *self-consistent* because both reads
//! still share whatever context is active — the failure mode is a
//! possibly-scaled pixel value, never a mismatched ratio.

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::decode::ScreenRect;
use crate::shared::RawCursor;

/// Pin this thread to per-monitor-v2 DPI awareness.
///
/// Per-monitor-v2 is chosen because it makes both the cursor and the screen
/// metrics return true physical pixels, which is what `MouseData.x`/`y`
/// promise. Returns whether the pin took effect, for diagnostics only —
/// failure is not fatal, per the module note.
pub fn pin_dpi_context() -> bool {
    // SAFETY: sets a thread-local awareness context; the returned previous
    // context is a cookie we deliberately drop, since the worker owns this
    // thread for its whole lifetime and never restores it.
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    !previous.is_invalid()
}

/// Read the virtual desktop rect, in physical pixels under the pinned context.
///
/// Re-read on every wake rather than cached: a message-only window receives no
/// broadcasts, so `WM_DISPLAYCHANGE` never arrives and there is no invalidation
/// signal to cache against. These resolve from user32 shared memory and cost
/// nothing.
#[must_use]
pub fn virtual_screen_rect() -> ScreenRect {
    // SAFETY: `GetSystemMetrics` reads process-global metrics and cannot fail
    // in a way that requires cleanup; unknown indices simply return 0.
    unsafe {
        ScreenRect {
            left: GetSystemMetrics(SM_XVIRTUALSCREEN),
            top: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// Read the primary monitor rect, which absolute pointer reports without
/// `MOUSE_VIRTUAL_DESKTOP` are scaled against.
#[must_use]
pub fn primary_screen_rect() -> ScreenRect {
    // SAFETY: as `virtual_screen_rect`.
    unsafe {
        ScreenRect {
            left: 0,
            top: 0,
            width: GetSystemMetrics(SM_CXSCREEN),
            height: GetSystemMetrics(SM_CYSCREEN),
        }
    }
}

/// Sample the cursor, normalized against the virtual desktop.
///
/// Returns `None` when the cursor is unreadable, which happens on the secure
/// desktop (UAC prompt, lock screen, Ctrl+Alt+Del) where `GetCursorPos`
/// returns access-denied. Callers hold their previous value rather than
/// blanking, so effects do not lurch on unlock.
#[must_use]
pub fn sample_cursor(virtual_screen: ScreenRect) -> Option<RawCursor> {
    let mut point = POINT::default();
    // SAFETY: `point` is a live, properly aligned `POINT` owned by this frame;
    // the call only writes into it. Access-denied on the secure desktop
    // surfaces as an `Err` rather than a partial write.
    unsafe { GetCursorPos(&raw mut point) }.ok()?;
    let (norm_x, norm_y) = virtual_screen.normalize(point.x, point.y);
    Some(RawCursor {
        x: point.x,
        y: point.y,
        norm_x,
        norm_y,
    })
}
