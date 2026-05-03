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

#[cfg(target_os = "windows")]
thread_local! {
    static SERVO_RENDER_WINDOWS: RefCell<Vec<WindowsServoWindow>> = const { RefCell::new(Vec::new()) };
}

#[cfg(target_os = "windows")]
struct WindowsServoWindow {
    _event_loop: EventLoop<()>,
    _window: Window,
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

/// Create the rendering context used by Hypercolor's Servo worker.
///
/// Windows uses a hidden native window plus Servo's offscreen context. Servo's
/// software WARP context can load pages there, but WebGL effects panic during
/// ANGLE surface import before any pixels can be read back.
#[cfg(target_os = "windows")]
pub(crate) fn bootstrap_rendering_context(
    width: u32,
    height: u32,
) -> Result<Rc<dyn RenderingContext>> {
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

    Ok(Rc::new(context))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn bootstrap_rendering_context(
    width: u32,
    height: u32,
) -> Result<Rc<dyn RenderingContext>> {
    Ok(Rc::new(bootstrap_software_rendering_context(
        width, height,
    )?))
}
