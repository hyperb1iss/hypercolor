//! Servo feature bootstrap helpers.
//!
//! This module is intentionally minimal for Phase 6.1:
//! - verify that `servo` is wired correctly behind a crate feature
//! - provide a tiny API to create a headless rendering context
//! - keep all Servo-specific types out of non-Servo builds

use std::rc::Rc;

use anyhow::{Result, anyhow};
use dpi::PhysicalSize;
use servo::{RenderingContext, SoftwareRenderingContext};

#[cfg(all(target_os = "linux", feature = "servo-gpu-import"))]
use hypercolor_linux_gpu_interop::{LinuxServoRenderDevice, LinuxServoRenderingContext};
#[cfg(all(target_os = "macos", feature = "servo-gpu-import"))]
use hypercolor_macos_gpu_interop::MacosHardwareRenderingContext;
#[cfg(all(target_os = "windows", feature = "servo-gpu-import"))]
use hypercolor_windows_gpu_interop::{WindowsAngleRenderingContext, WindowsDxgiAdapterIdentity};
#[cfg(target_os = "windows")]
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
#[cfg(target_os = "windows")]
use servo::WindowRenderingContext;
#[cfg(target_os = "windows")]
use std::cell::RefCell;
#[cfg(target_os = "windows")]
use tao::event_loop::{EventLoop, EventLoopBuilder};
#[cfg(target_os = "windows")]
use tao::platform::windows::EventLoopBuilderExtWindows;
#[cfg(target_os = "windows")]
use tao::window::{Window, WindowBuilder};
#[cfg(any(
    all(target_os = "macos", feature = "servo-gpu-import"),
    all(target_os = "windows", feature = "servo-gpu-import")
))]
use tracing::warn;

#[cfg(target_os = "windows")]
thread_local! {
    static SERVO_RENDER_WINDOWS: RefCell<Vec<WindowsServoWindow>> = const { RefCell::new(Vec::new()) };
}

#[cfg(target_os = "windows")]
struct WindowsServoWindow {
    _event_loop: EventLoop<()>,
    _window: Window,
}

pub(crate) struct ServoRenderingContextHandle {
    pub(crate) rendering_context: Rc<dyn RenderingContext>,
    #[cfg(all(target_os = "linux", feature = "servo-gpu-import"))]
    pub(crate) linux_context: Option<Rc<LinuxServoRenderingContext>>,
    #[cfg(all(target_os = "macos", feature = "servo-gpu-import"))]
    pub(crate) macos_hardware_context: Option<Rc<MacosHardwareRenderingContext>>,
    #[cfg(all(target_os = "windows", feature = "servo-gpu-import"))]
    pub(crate) windows_angle_context: Option<Rc<WindowsAngleRenderingContext>>,
}

impl ServoRenderingContextHandle {
    fn new(rendering_context: Rc<dyn RenderingContext>) -> Self {
        Self {
            rendering_context,
            #[cfg(all(target_os = "linux", feature = "servo-gpu-import"))]
            linux_context: None,
            #[cfg(all(target_os = "macos", feature = "servo-gpu-import"))]
            macos_hardware_context: None,
            #[cfg(all(target_os = "windows", feature = "servo-gpu-import"))]
            windows_angle_context: None,
        }
    }

    #[cfg(all(target_os = "linux", feature = "servo-gpu-import"))]
    fn linux_software(context: Rc<LinuxServoRenderingContext>) -> Self {
        let rendering_context: Rc<dyn RenderingContext> = context.clone();
        Self {
            rendering_context,
            linux_context: Some(context),
            #[cfg(all(target_os = "windows", feature = "servo-gpu-import"))]
            windows_angle_context: None,
        }
    }

    #[cfg(all(target_os = "macos", feature = "servo-gpu-import"))]
    fn macos_hardware(context: Rc<MacosHardwareRenderingContext>) -> Self {
        let rendering_context: Rc<dyn RenderingContext> = context.clone();
        Self {
            rendering_context,
            #[cfg(all(target_os = "linux", feature = "servo-gpu-import"))]
            linux_context: None,
            macos_hardware_context: Some(context),
            #[cfg(all(target_os = "windows", feature = "servo-gpu-import"))]
            windows_angle_context: None,
        }
    }

    #[cfg(all(target_os = "windows", feature = "servo-gpu-import"))]
    fn windows_angle(context: Rc<WindowsAngleRenderingContext>) -> Self {
        let rendering_context: Rc<dyn RenderingContext> = context.clone();
        Self {
            rendering_context,
            #[cfg(all(target_os = "linux", feature = "servo-gpu-import"))]
            linux_context: None,
            #[cfg(all(target_os = "macos", feature = "servo-gpu-import"))]
            macos_hardware_context: None,
            windows_angle_context: Some(context),
        }
    }
}

#[cfg(all(target_os = "linux", feature = "servo-gpu-import"))]
pub(crate) fn bootstrap_linux_shared_rendering_context(
    parent: Rc<LinuxServoRenderDevice>,
    width: u32,
    height: u32,
) -> Result<ServoRenderingContextHandle> {
    let context = Rc::new(parent.create_rendering_context(width, height).map_err(|error| {
        anyhow!("failed to create Linux Servo offscreen render target ({width}x{height}): {error:?}")
    })?);
    Ok(ServoRenderingContextHandle::linux_software(context))
}

/// Create a headless Servo software rendering context.
///
/// This is the first integration seam for HTML effect rendering. Later phases
/// will layer `ServoBuilder`, `WebView`, and runtime JS/audio injection on top.
///
/// # Errors
///
/// Returns an error if the software OpenGL adapter/context cannot be created.
pub fn bootstrap_software_rendering_context(
    width: u32,
    height: u32,
) -> Result<SoftwareRenderingContext> {
    SoftwareRenderingContext::new(PhysicalSize::new(width, height)).map_err(|error| {
        anyhow!("failed to create Servo SoftwareRenderingContext ({width}x{height}): {error:?}")
    })
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) fn bootstrap_software_rendering_context_handle(
    width: u32,
    height: u32,
) -> Result<ServoRenderingContextHandle> {
    Ok(ServoRenderingContextHandle::new(Rc::new(
        bootstrap_software_rendering_context(width, height)?,
    )))
}

/// Create the rendering context used by Hypercolor's Servo worker.
///
/// Windows uses a hidden native window plus Servo's offscreen context. Servo's
/// software WARP context can load pages there, but WebGL effects panic during
/// ANGLE surface import before any pixels can be read back.
#[cfg(all(target_os = "windows", not(feature = "servo-gpu-import")))]
pub(crate) fn bootstrap_rendering_context(
    width: u32,
    height: u32,
) -> Result<ServoRenderingContextHandle> {
    bootstrap_windows_window_rendering_context(width, height)
}

#[cfg(all(target_os = "windows", feature = "servo-gpu-import"))]
pub(crate) fn bootstrap_rendering_context(
    width: u32,
    height: u32,
) -> Result<ServoRenderingContextHandle> {
    if crate::effect::servo_gpu_import_should_attempt() {
        let adapter_identity = crate::effect::servo_gpu_import_adapter_info().map(|adapter_info| {
            WindowsDxgiAdapterIdentity {
                vendor_id: adapter_info.vendor_id,
                device_id: adapter_info.device_id,
            }
        });
        match WindowsAngleRenderingContext::new(width, height, adapter_identity) {
            Ok(context) => {
                return Ok(ServoRenderingContextHandle::windows_angle(Rc::new(context)));
            }
            Err(error)
                if matches!(
                    crate::effect::servo_gpu_import_mode(),
                    hypercolor_types::config::ServoGpuImportMode::On
                ) =>
            {
                return Err(anyhow!(
                    "failed to create required Windows Servo ANGLE context: {error}"
                ));
            }
            Err(error) => {
                warn!(%error, "Windows Servo GPU import context unavailable; using hidden-window CPU context");
            }
        }
    }

    bootstrap_windows_window_rendering_context(width, height)
}

#[cfg(target_os = "windows")]
fn bootstrap_windows_window_rendering_context(
    width: u32,
    height: u32,
) -> Result<ServoRenderingContextHandle> {
    let event_loop = EventLoopBuilder::new().with_any_thread(true).build();
    let window = WindowBuilder::new()
        .with_title("Hypercolor Servo Renderer")
        .with_visible(false)
        .with_decorations(false)
        .with_inner_size(tao::dpi::PhysicalSize::new(width, height))
        .build(&event_loop)
        .map_err(|error| {
            anyhow!("failed to create hidden Servo rendering window ({width}x{height}): {error}")
        })?;

    let display_handle = window.display_handle().map_err(|error| {
        anyhow!("failed to get hidden Servo rendering display handle: {error:?}")
    })?;
    let window_handle = window.window_handle().map_err(|error| {
        anyhow!("failed to get hidden Servo rendering window handle: {error:?}")
    })?;
    let size = PhysicalSize::new(width, height);
    let parent = Rc::new(
        WindowRenderingContext::new(display_handle, window_handle, size).map_err(|error| {
            anyhow!("failed to create Servo WindowRenderingContext ({width}x{height}): {error:?}")
        })?,
    );
    let context = parent.offscreen_context(size);

    SERVO_RENDER_WINDOWS.with(|windows| {
        windows.borrow_mut().push(WindowsServoWindow {
            _event_loop: event_loop,
            _window: window,
        });
    });

    Ok(ServoRenderingContextHandle::new(Rc::new(context)))
}

#[cfg(all(target_os = "macos", feature = "servo-gpu-import"))]
pub(crate) fn bootstrap_rendering_context(
    width: u32,
    height: u32,
) -> Result<ServoRenderingContextHandle> {
    if crate::effect::servo_gpu_import_should_attempt() {
        match MacosHardwareRenderingContext::new(width, height) {
            Ok(context) => {
                return Ok(ServoRenderingContextHandle::macos_hardware(Rc::new(
                    context,
                )));
            }
            Err(error)
                if matches!(
                    crate::effect::servo_gpu_import_mode(),
                    hypercolor_types::config::ServoGpuImportMode::On
                ) =>
            {
                return Err(anyhow!(
                    "failed to create required macOS Servo hardware context: {error}"
                ));
            }
            Err(error) => {
                warn!(%error, "macOS Servo hardware context unavailable; using software context");
            }
        }
    }

    bootstrap_software_rendering_context_handle(width, height)
}

#[cfg(not(any(
    target_os = "windows",
    all(target_os = "linux", feature = "servo-gpu-import"),
    all(target_os = "macos", feature = "servo-gpu-import")
)))]
pub(crate) fn bootstrap_rendering_context(
    width: u32,
    height: u32,
) -> Result<ServoRenderingContextHandle> {
    bootstrap_software_rendering_context_handle(width, height)
}
