use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};

use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use hypercolor_core::session::SessionMonitor;
use hypercolor_types::session::SessionEvent;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Power::{
    RegisterSuspendResumeNotification, UnregisterSuspendResumeNotification,
};
use windows_sys::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, HWND_MESSAGE, IsWindow, MSG,
    PostMessageW, PostQuitMessage, PostThreadMessageW, RegisterClassW, SetWindowLongPtrW,
    TranslateMessage, UnregisterClassW, WM_CLOSE, WM_DESTROY, WM_NCCREATE, WM_NCDESTROY, WNDCLASSW,
};

use crate::decode::decode_window_message;

static CLASS_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Standalone Windows monitor backed by a message-only Win32 window.
#[derive(Default)]
pub struct StandaloneSessionMonitor;

impl StandaloneSessionMonitor {
    /// Create a standalone Windows session monitor.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SessionMonitor for StandaloneSessionMonitor {
    fn name(&self) -> &'static str {
        "windows-message-window"
    }

    async fn run(
        self: Box<Self>,
        tx: mpsc::Sender<SessionEvent>,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let (ready_tx, ready_rx) = oneshot::channel();
        let (done_tx, mut done_rx) = oneshot::channel();
        let worker = std::thread::Builder::new()
            .name("hypercolor-session-events".to_owned())
            .spawn(move || {
                let result = run_message_loop(tx, ready_tx);
                let _ = done_tx.send(result);
            })
            .context("failed to spawn Windows session message-pump thread")?;

        let worker_result = match ready_rx.await {
            Ok(endpoint) => {
                tokio::select! {
                    () = cancel.cancelled() => {
                        post_close(endpoint)?;
                        done_rx.await.context("Windows session worker exited without a result")?
                    }
                    result = &mut done_rx => {
                        result.context("Windows session worker exited without a result")?
                    }
                }
            }
            Err(_) => done_rx
                .await
                .context("Windows session worker failed before window creation")?,
        };

        tokio::task::spawn_blocking(move || worker.join())
            .await
            .context("failed to join Windows session message-pump task")?
            .map_err(|_| anyhow!("Windows session message-pump thread panicked"))?;

        worker_result
    }
}

struct WindowContext {
    tx: mpsc::Sender<SessionEvent>,
    wts_registered: AtomicBool,
    power_registration: AtomicIsize,
}

#[derive(Clone, Copy)]
struct WorkerEndpoint {
    window: isize,
    thread_id: u32,
}

fn run_message_loop(
    tx: mpsc::Sender<SessionEvent>,
    ready: oneshot::Sender<WorkerEndpoint>,
) -> anyhow::Result<()> {
    let sequence = CLASS_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let class_name = wide(&format!(
        "HypercolorSessionEvents.{}.{}",
        std::process::id(),
        sequence
    ));
    let window_name = wide("HypercolorSessionEvents");
    let context = Box::new(WindowContext {
        tx,
        wts_registered: AtomicBool::new(false),
        power_registration: AtomicIsize::new(0),
    });

    // SAFETY: A null module name requests the module handle of this process.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        bail!("GetModuleHandleW failed with error {}", last_error());
    }

    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: ptr::null_mut(),
        hCursor: ptr::null_mut(),
        hbrBackground: ptr::null_mut(),
        lpszMenuName: ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };

    // SAFETY: The class and UTF-16 class name remain valid for this call.
    if unsafe { RegisterClassW(&class) } == 0 {
        bail!("RegisterClassW failed with error {}", last_error());
    }

    // SAFETY: All pointers remain valid through window creation. WindowContext
    // is boxed and outlives the window and its final callback.
    let window = unsafe {
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
            instance,
            (&raw const *context).cast::<c_void>(),
        )
    };
    if window.is_null() {
        let error = last_error();
        unregister_class(&class_name, instance);
        bail!("CreateWindowExW failed with error {error}");
    }

    // SAFETY: The message-only HWND is valid and owned by this thread.
    if unsafe { WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) } == 0 {
        let error = last_error();
        destroy_window(window);
        unregister_class(&class_name, instance);
        bail!("WTSRegisterSessionNotification failed with error {error}");
    }
    context.wts_registered.store(true, Ordering::Release);

    // Message-only windows do not receive ordinary broadcast messages. This
    // registration targets suspend and resume notifications at this HWND.
    // SAFETY: The message-only HWND is valid and owned by this thread.
    let power_registration =
        unsafe { RegisterSuspendResumeNotification(window, DEVICE_NOTIFY_WINDOW_HANDLE) };
    if power_registration == 0 {
        let error = last_error();
        destroy_window(window);
        unregister_class(&class_name, instance);
        bail!("RegisterSuspendResumeNotification failed with error {error}");
    }
    context
        .power_registration
        .store(power_registration, Ordering::Release);

    // SAFETY: GetCurrentThreadId has no pointer or lifetime requirements.
    let endpoint = WorkerEndpoint {
        window: window as isize,
        thread_id: unsafe { GetCurrentThreadId() },
    };
    if ready.send(endpoint).is_err() {
        destroy_window(window);
    } else {
        debug!("Windows standalone session monitor active");
        pump_messages();
    }

    // SAFETY: The handle came from CreateWindowExW on this worker thread.
    if unsafe { IsWindow(window) } != 0 {
        destroy_window(window);
    }
    unregister_class(&class_name, instance);
    debug!("Windows standalone session monitor stopped");
    Ok(())
}

fn pump_messages() {
    let mut message = MSG::default();
    loop {
        // SAFETY: The MSG pointer is valid. A null HWND receives all messages
        // on this worker thread, including WM_QUIT.
        let result = unsafe { GetMessageW(&raw mut message, ptr::null_mut(), 0, 0) };
        if result <= 0 {
            if result < 0 {
                warn!(error = last_error(), "Windows session message pump failed");
            }
            break;
        }

        // SAFETY: The message was initialized by GetMessageW and follows the
        // documented Win32 dispatch sequence.
        unsafe {
            TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: Windows supplies a valid CREATESTRUCTW for WM_NCCREATE. Its
        // lpCreateParams is the boxed WindowContext passed to CreateWindowExW.
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        // SAFETY: The context pointer remains valid until after WM_NCDESTROY.
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        return 1;
    }

    if message == crate::WM_POWERBROADCAST_MESSAGE {
        if let Some(event) = decode_window_message(message, wparam as u32) {
            publish_event(window, event);
        }
        return 1;
    }

    if message == crate::WM_WTSSESSION_CHANGE_MESSAGE {
        if let Some(event) = decode_window_message(message, wparam as u32) {
            publish_event(window, event);
        }
        return 0;
    }

    match message {
        WM_CLOSE => {
            destroy_window(window);
            0
        }
        WM_DESTROY => {
            unregister_notifications(window);
            // SAFETY: Posting WM_QUIT affects only the current message queue.
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_NCDESTROY => {
            // SAFETY: Clearing userdata prevents later callbacks from
            // observing the context after its owner leaves the message loop.
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
                DefWindowProcW(window, message, wparam, lparam)
            }
        }
        _ => {
            // SAFETY: Unhandled messages are forwarded unchanged to the
            // documented Win32 default procedure.
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
    }
}

fn publish_event(window: HWND, event: SessionEvent) {
    // SAFETY: GWLP_USERDATA contains the WindowContext pointer installed
    // during WM_NCCREATE and cleared during WM_NCDESTROY.
    let context = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as *const WindowContext };
    let Some(context) = (unsafe { context.as_ref() }) else {
        return;
    };

    if let Err(error) = context.tx.try_send(event) {
        warn!(%error, "failed to publish Windows session event");
    }
}

fn post_close(endpoint: WorkerEndpoint) -> anyhow::Result<()> {
    // SAFETY: The handle came from this worker's successful CreateWindowExW.
    if unsafe { PostMessageW(endpoint.window as HWND, WM_CLOSE, 0, 0) } != 0 {
        return Ok(());
    }
    let window_error = last_error();

    // SAFETY: The thread ID came from the worker after its message queue and
    // window were created. WM_QUIT lets the worker perform its own cleanup.
    if unsafe {
        PostThreadMessageW(
            endpoint.thread_id,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_QUIT,
            0,
            0,
        )
    } != 0
    {
        warn!(
            window_error,
            "window close failed; stopped Windows session worker through its thread queue"
        );
        return Ok(());
    }

    bail!(
        "failed to stop Windows session worker: PostMessageW error {window_error}, \
         PostThreadMessageW error {}",
        last_error()
    )
}

fn destroy_window(window: HWND) {
    // SAFETY: The HWND belongs to this message-pump thread and is valid at
    // each call site.
    if unsafe { DestroyWindow(window) } == 0 {
        warn!(
            error = last_error(),
            "failed to destroy Windows session message window"
        );
    }
}

fn unregister_notifications(window: HWND) {
    // SAFETY: GWLP_USERDATA contains the WindowContext pointer installed
    // during WM_NCCREATE and valid through this WM_DESTROY callback.
    let context = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as *const WindowContext };
    let Some(context) = (unsafe { context.as_ref() }) else {
        return;
    };
    let power_registration = context.power_registration.swap(0, Ordering::AcqRel);
    if power_registration != 0 {
        // SAFETY: The handle came from RegisterSuspendResumeNotification and
        // is released once during WM_DESTROY.
        if unsafe { UnregisterSuspendResumeNotification(power_registration) } == 0 {
            warn!(
                error = last_error(),
                "failed to unregister Windows suspend-resume notifications"
            );
        }
    }

    if context.wts_registered.swap(false, Ordering::AcqRel) {
        // SAFETY: Registration is paired with this still-valid window.
        if unsafe { WTSUnRegisterSessionNotification(window) } == 0 {
            warn!(
                error = last_error(),
                "failed to unregister WTS session notifications"
            );
        }
    }
}

fn unregister_class(class_name: &[u16], instance: *mut c_void) {
    // SAFETY: The UTF-16 class name is nul-terminated and instance is the
    // module handle used for RegisterClassW.
    if unsafe { UnregisterClassW(class_name.as_ptr(), instance) } == 0 {
        warn!(
            error = last_error(),
            "failed to unregister Windows session window class"
        );
    }
}

fn last_error() -> u32 {
    // SAFETY: GetLastError reads thread-local state and takes no pointers.
    unsafe { GetLastError() }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
