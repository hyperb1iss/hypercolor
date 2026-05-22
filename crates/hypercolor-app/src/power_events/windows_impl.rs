//! Windows implementation of the power-event listener.
//!
//! Win32 message pumps must own a thread, so we spawn a dedicated worker
//! that registers a message-only window class, drains messages, and emits
//! resume events through a tokio channel. A second async task consumes the
//! channel and POSTs to the daemon's discovery endpoint.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::mpsc;
use url::Url;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
    RegisterClassW, TranslateMessage, WM_POWERBROADCAST, WNDCLASSW,
};

/// `PBT_APMRESUMEAUTOMATIC` — system has woken from sleep without user
/// interaction. Microsoft docs:
/// <https://learn.microsoft.com/en-us/windows/win32/power/pbt-apmresumeautomatic>.
const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;
/// `PBT_APMRESUMESUSPEND` — system has woken from sleep on user input.
/// Either flavor warrants a device rediscover.
const PBT_APMRESUMESUSPEND: u32 = 0x0007;

/// Channel sender shared with the WndProc. There is exactly one listener
/// per process — initialized on first `start` call — so the static
/// `OnceLock` is the right shape.
static RESUME_TX: OnceLock<mpsc::UnboundedSender<()>> = OnceLock::new();

/// Spawn the listener.
///
/// Idempotent: subsequent calls are no-ops, since the static channel
/// already exists.
pub fn start(daemon_url: Url) {
    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    if RESUME_TX.set(tx).is_err() {
        tracing::debug!("power-event listener already running; ignoring duplicate start");
        return;
    }

    // Dispatcher: turns resume events into HTTP nudges to the daemon.
    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::new();
        let discover_url = match daemon_url.join("/api/v1/devices/discover") {
            Ok(url) => url,
            Err(err) => {
                tracing::warn!(%err, "could not derive /api/v1/devices/discover from daemon URL");
                return;
            }
        };
        while rx.recv().await.is_some() {
            tracing::info!(url = %discover_url, "power-resume detected; triggering device rediscovery");
            match client
                .post(discover_url.clone())
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    tracing::info!("post-resume device rediscovery dispatched");
                }
                Ok(response) => {
                    tracing::warn!(
                        status = %response.status(),
                        "post-resume device rediscovery returned non-success"
                    );
                }
                Err(err) => {
                    tracing::warn!(%err, "post-resume device rediscovery request failed");
                }
            }
        }
    });

    // Win32 message-pump worker. Standalone std::thread because Win32
    // window messages must be pumped on the thread that owns the HWND.
    std::thread::Builder::new()
        .name("hypercolor-power-events".to_owned())
        .spawn(run_message_loop)
        .map(|_| ())
        .unwrap_or_else(|err| {
            tracing::warn!(%err, "failed to spawn power-event message-pump thread");
        });
}

fn run_message_loop() {
    let class_name = wide_str("HypercolorPowerEventsClass");

    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        // SAFETY: GetModuleHandleW(NULL) returns the current module handle.
        hInstance: unsafe { GetModuleHandleW(ptr::null()) },
        hIcon: ptr::null_mut(),
        hCursor: ptr::null_mut(),
        hbrBackground: ptr::null_mut(),
        lpszMenuName: ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };

    // SAFETY: WNDCLASSW pointer is valid for the call; class_name buffer
    // outlives the call (`as_ptr` borrow of `Vec<u16>` on stack).
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        tracing::warn!(
            err = unsafe { windows_sys::Win32::Foundation::GetLastError() },
            "RegisterClassW failed; power events disabled"
        );
        return;
    }

    let window_name = wide_str("HypercolorPowerEvents");
    // SAFETY: All pointers are valid for the lifetime of the call. The
    // returned HWND is owned by this thread until the message loop exits.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut(),
            class.hInstance,
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        tracing::warn!(
            err = unsafe { windows_sys::Win32::Foundation::GetLastError() },
            "CreateWindowExW failed; power events disabled"
        );
        return;
    }

    tracing::info!("Windows power-event listener active");

    let mut msg = MSG {
        hwnd: ptr::null_mut(),
        message: 0,
        wParam: 0,
        lParam: 0,
        time: 0,
        pt: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
    };
    loop {
        // SAFETY: GetMessageW with a non-null window pulls messages for
        // that window only. Return values: >0 = msg, 0 = WM_QUIT, <0 = err.
        let result = unsafe { GetMessageW(&mut msg, hwnd, 0, 0) };
        if result <= 0 {
            break;
        }
        unsafe {
            // SAFETY: Standard Win32 message dispatch sequence.
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    tracing::info!("Windows power-event listener exiting");
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_POWERBROADCAST {
        #[allow(clippy::cast_possible_truncation)]
        let event = wparam as u32;
        if event == PBT_APMRESUMEAUTOMATIC || event == PBT_APMRESUMESUSPEND {
            if let Some(tx) = RESUME_TX.get() {
                let _ = tx.send(());
            }
        }
        return 1; // TRUE — per Microsoft docs, return TRUE from WM_POWERBROADCAST
    }
    // SAFETY: DefWindowProcW is the documented Win32 default handler for
    // every message we don't intercept; hwnd / msg / wparam / lparam are
    // forwarded unchanged from the OS.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn wide_str(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(std::iter::once(0)).collect()
}
