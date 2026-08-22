use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use block2::RcBlock;
use hypercolor_core::session::SessionMonitor;
use hypercolor_types::session::SessionEvent;
use hypercolor_worker_retention::{retain_worker, spawn_worker};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceSessionDidBecomeActiveNotification,
    NSWorkspaceSessionDidResignActiveNotification,
};
use objc2_core_foundation::{
    CFRunLoop, CFRunLoopSource, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSNotificationName};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::{MacosSessionNotification, decode_session_notification};

const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const RUN_LOOP_POLL_INTERVAL_SECONDS: f64 = 0.1;
const IO_MESSAGE_CAN_SYSTEM_SLEEP: u32 = 0xE000_0270;
const IO_MESSAGE_SYSTEM_WILL_SLEEP: u32 = 0xE000_0280;
const IO_MESSAGE_SYSTEM_HAS_POWERED_ON: u32 = 0xE000_0300;

type IoConnect = u32;
type IoObject = u32;
type IoNotificationPort = *mut c_void;
type IoServiceInterestCallback = unsafe extern "C" fn(
    refcon: *mut c_void,
    service: IoObject,
    message_type: u32,
    message_argument: *mut c_void,
);

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IORegisterForSystemPower(
        refcon: *mut c_void,
        notification_port: *mut IoNotificationPort,
        callback: IoServiceInterestCallback,
        notifier: *mut IoObject,
    ) -> IoConnect;
    fn IODeregisterForSystemPower(notifier: *mut IoObject) -> i32;
    fn IOAllowPowerChange(kernel_port: IoConnect, notification_id: isize) -> i32;
    fn IONotificationPortGetRunLoopSource(
        notification_port: IoNotificationPort,
    ) -> *mut CFRunLoopSource;
    fn IONotificationPortDestroy(notification_port: IoNotificationPort);
    fn IOServiceClose(connection: IoConnect) -> i32;
}

/// Native NSWorkspace and IOKit session monitor.
#[derive(Default)]
pub struct MacosSessionMonitor;

impl MacosSessionMonitor {
    /// Create a native macOS session monitor.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SessionMonitor for MacosSessionMonitor {
    fn name(&self) -> &'static str {
        "macos-workspace-iokit"
    }

    async fn run(
        self: Box<Self>,
        tx: mpsc::Sender<SessionEvent>,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let control = Arc::new(RunLoopControl::default());
        let (ready_tx, ready_rx) = oneshot::channel();
        let (done_tx, mut done_rx) = oneshot::channel();
        let worker = spawn_worker(
            std::thread::Builder::new().name("hypercolor-macos-session".to_owned()),
            {
                let control = Arc::clone(&control);
                move || {
                    let result = run_native_monitor(tx, &control, ready_tx);
                    let _ = done_tx.send(result);
                }
            },
        )
        .context("failed to spawn macOS session notification thread")?;
        let mut worker = ManagedWorker::new(worker, control);

        let mut completed = None;
        tokio::select! {
            () = cancel.cancelled() => worker.request_stop(),
            ready = ready_rx => {
                if ready.is_err() {
                    completed = Some((&mut done_rx).await);
                } else {
                    tokio::select! {
                        () = cancel.cancelled() => worker.request_stop(),
                        result = &mut done_rx => completed = Some(result),
                    }
                }
            }
        }

        let result = if let Some(result) = completed {
            result
        } else {
            tokio::time::timeout(WORKER_STOP_TIMEOUT, &mut done_rx)
                .await
                .map_err(|_| anyhow!("macOS session notification thread did not stop in time"))?
        }
        .context("macOS session notification thread exited without a result")?;

        worker.join().await?;
        result
    }
}

struct ManagedWorker {
    handle: Option<JoinHandle<()>>,
    control: Arc<RunLoopControl>,
}

impl ManagedWorker {
    fn new(handle: JoinHandle<()>, control: Arc<RunLoopControl>) -> Self {
        Self {
            handle: Some(handle),
            control,
        }
    }

    fn request_stop(&self) {
        self.control.request_stop();
    }

    async fn join(&mut self) -> anyhow::Result<()> {
        let handle = self
            .handle
            .take()
            .ok_or_else(|| anyhow!("macOS session worker handle was already consumed"))?;
        tokio::task::spawn_blocking(move || handle.join())
            .await
            .context("failed to join macOS session notification task")?
            .map_err(|_| anyhow!("macOS session notification thread panicked"))
    }
}

impl Drop for ManagedWorker {
    fn drop(&mut self) {
        self.control.request_stop();
        if let Some(handle) = self.handle.take() {
            retain_worker(handle, "macOS session notification thread");
        }
    }
}

#[derive(Default)]
struct RunLoopControl {
    stopping: AtomicBool,
    run_loop: Mutex<usize>,
}

impl RunLoopControl {
    fn install(&self, run_loop: &CFRunLoop) {
        *self
            .run_loop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            std::ptr::from_ref(run_loop).expose_provenance();
    }

    fn clear(&self) {
        *self
            .run_loop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
    }

    fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
        let run_loop = self
            .run_loop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *run_loop == 0 {
            return;
        }
        // SAFETY: the worker clears this address under the same mutex before
        // releasing its retained run loop. Core Foundation permits stopping
        // and waking a run loop from another thread.
        let run_loop = unsafe { &*std::ptr::with_exposed_provenance::<CFRunLoop>(*run_loop) };
        run_loop.stop();
        run_loop.wake_up();
    }
}

struct RunLoopInstallation<'a> {
    control: &'a RunLoopControl,
}

impl Drop for RunLoopInstallation<'_> {
    fn drop(&mut self) {
        self.control.clear();
    }
}

struct CallbackContext {
    tx: mpsc::Sender<SessionEvent>,
    root_port: AtomicU32,
}

fn run_native_monitor(
    tx: mpsc::Sender<SessionEvent>,
    control: &RunLoopControl,
    ready: oneshot::Sender<()>,
) -> anyhow::Result<()> {
    autoreleasepool(|_| {
        let run_loop = CFRunLoop::current()
            .ok_or_else(|| anyhow!("macOS session thread has no Core Foundation run loop"))?;
        control.install(&run_loop);
        let _run_loop_installation = RunLoopInstallation { control };

        let context = Box::new(CallbackContext {
            tx,
            root_port: AtomicU32::new(0),
        });
        let resources = NativeResources::install(&run_loop, context)?;
        let _ = ready.send(());

        while !control.stopping.load(Ordering::Acquire) {
            // SAFETY: Core Foundation exports this process-lifetime run-loop
            // mode and the worker owns the run loop for this call.
            let mode = unsafe { kCFRunLoopDefaultMode };
            CFRunLoop::run_in_mode(mode, RUN_LOOP_POLL_INTERVAL_SECONDS, true);
        }

        drop(resources);
        Ok(())
    })
}

struct NativeResources {
    run_loop: RetainedRunLoop,
    workspace_center: Retained<NSNotificationCenter>,
    workspace_observers: Vec<Retained<AnyObject>>,
    power: PowerRegistration,
    _context: Box<CallbackContext>,
}

type RetainedRunLoop = objc2_core_foundation::CFRetained<CFRunLoop>;

impl NativeResources {
    fn install(run_loop: &RetainedRunLoop, context: Box<CallbackContext>) -> anyhow::Result<Self> {
        let power = PowerRegistration::install(run_loop, &context)?;
        let workspace = NSWorkspace::sharedWorkspace();
        let workspace_center = workspace.notificationCenter();
        let workspace_observers = vec![
            observe_workspace_notification(
                &workspace_center,
                // SAFETY: AppKit exports this process-lifetime notification name.
                unsafe { NSWorkspaceSessionDidResignActiveNotification },
                MacosSessionNotification::SessionResigned,
                context.tx.clone(),
            ),
            observe_workspace_notification(
                &workspace_center,
                // SAFETY: AppKit exports this process-lifetime notification name.
                unsafe { NSWorkspaceSessionDidBecomeActiveNotification },
                MacosSessionNotification::SessionBecameActive,
                context.tx.clone(),
            ),
        ];

        Ok(Self {
            run_loop: run_loop.clone(),
            workspace_center,
            workspace_observers,
            power,
            _context: context,
        })
    }
}

impl Drop for NativeResources {
    fn drop(&mut self) {
        for observer in &self.workspace_observers {
            // SAFETY: each token came from this notification center and stays
            // retained until after removal completes.
            unsafe { self.workspace_center.removeObserver(observer) };
        }
        self.power.teardown(&self.run_loop);
    }
}

fn observe_workspace_notification(
    center: &NSNotificationCenter,
    name: &NSNotificationName,
    notification: MacosSessionNotification,
    tx: mpsc::Sender<SessionEvent>,
) -> Retained<AnyObject> {
    let block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
        publish_notification(&tx, notification);
    });
    // SAFETY: the block captures only a thread-safe Tokio sender and an owned
    // value. The returned observer token retains the escaping block.
    unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block) }
        .into()
}

struct PowerRegistration {
    root_port: IoConnect,
    notification_port: IoNotificationPort,
    notifier: IoObject,
    source: *mut CFRunLoopSource,
}

impl PowerRegistration {
    fn install(run_loop: &CFRunLoop, context: &CallbackContext) -> anyhow::Result<Self> {
        let mut notification_port = std::ptr::null_mut();
        let mut notifier = 0;
        // SAFETY: the boxed callback context has a stable address and outlives
        // the registration. All output pointers are valid for this call.
        let root_port = unsafe {
            IORegisterForSystemPower(
                std::ptr::from_ref(context).cast_mut().cast(),
                &raw mut notification_port,
                power_callback,
                &raw mut notifier,
            )
        };
        if root_port == 0 {
            bail!("IORegisterForSystemPower returned no root power connection");
        }
        context.root_port.store(root_port, Ordering::Release);

        // SAFETY: the notification port is live until teardown.
        let source = unsafe { IONotificationPortGetRunLoopSource(notification_port) };
        if source.is_null() {
            cleanup_power_registration(root_port, notification_port, &mut notifier);
            bail!("IOKit returned no run-loop source for system power notifications");
        }

        // SAFETY: IOKit owns the source while the notification port is live.
        let source_ref = unsafe { &*source };
        // SAFETY: Core Foundation exports this process-lifetime run-loop mode.
        let mode = unsafe { kCFRunLoopCommonModes };
        run_loop.add_source(Some(source_ref), mode);

        Ok(Self {
            root_port,
            notification_port,
            notifier,
            source,
        })
    }

    fn teardown(&mut self, run_loop: &CFRunLoop) {
        if !self.source.is_null() {
            // SAFETY: the IOKit-owned source remains live until the
            // notification port is destroyed below.
            let source = unsafe { &*self.source };
            // SAFETY: Core Foundation exports this process-lifetime run-loop mode.
            let mode = unsafe { kCFRunLoopCommonModes };
            run_loop.remove_source(Some(source), mode);
            self.source = std::ptr::null_mut();
        }
        cleanup_power_registration(self.root_port, self.notification_port, &mut self.notifier);
        self.root_port = 0;
        self.notification_port = std::ptr::null_mut();
    }
}

fn cleanup_power_registration(
    root_port: IoConnect,
    notification_port: IoNotificationPort,
    notifier: &mut IoObject,
) {
    if *notifier != 0 {
        // SAFETY: the notifier belongs to this registration and is consumed
        // before its notification port and connection are released.
        let result = unsafe { IODeregisterForSystemPower(notifier) };
        if result != 0 {
            warn!(
                result,
                "failed to deregister macOS system-power notification"
            );
        }
        *notifier = 0;
    }
    if !notification_port.is_null() {
        // SAFETY: the port came from IORegisterForSystemPower and is no longer
        // registered with the root power domain.
        unsafe { IONotificationPortDestroy(notification_port) };
    }
    if root_port != 0 {
        // SAFETY: the connection came from IORegisterForSystemPower and all
        // dependent objects were released above.
        let result = unsafe { IOServiceClose(root_port) };
        if result != 0 {
            warn!(result, "failed to close macOS root power connection");
        }
    }
}

unsafe extern "C" fn power_callback(
    refcon: *mut c_void,
    _service: IoObject,
    message_type: u32,
    message_argument: *mut c_void,
) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: IOKit invokes the callback only while the boxed context remains
    // alive in NativeResources.
    let context = unsafe { &*refcon.cast::<CallbackContext>() };

    match message_type {
        IO_MESSAGE_CAN_SYSTEM_SLEEP => acknowledge_power_change(context, message_argument),
        IO_MESSAGE_SYSTEM_WILL_SLEEP => {
            publish_notification(&context.tx, MacosSessionNotification::SystemWillSleep);
            acknowledge_power_change(context, message_argument);
        }
        IO_MESSAGE_SYSTEM_HAS_POWERED_ON => {
            publish_notification(&context.tx, MacosSessionNotification::SystemPoweredOn);
        }
        _ => {}
    }
}

fn acknowledge_power_change(context: &CallbackContext, message_argument: *mut c_void) {
    let root_port = context.root_port.load(Ordering::Acquire);
    if root_port == 0 {
        return;
    }
    // SAFETY: IOKit supplies the notification identifier through the opaque
    // callback argument and requires this acknowledgment for these messages.
    let result = unsafe { IOAllowPowerChange(root_port, message_argument.addr() as isize) };
    if result != 0 {
        warn!(
            result,
            "failed to acknowledge macOS system-power notification"
        );
    }
}

fn publish_notification(tx: &mpsc::Sender<SessionEvent>, notification: MacosSessionNotification) {
    let Some(event) = decode_session_notification(notification) else {
        return;
    };
    match tx.try_send(event) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(event)) => {
            warn!(?event, "macOS session event queue is full");
        }
        Err(mpsc::error::TrySendError::Closed(event)) => {
            debug!(?event, "macOS session event receiver is closed");
        }
    }
}
