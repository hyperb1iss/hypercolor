//! Hidden native window hosting Servo's CPU-readback context on Windows.
//!
//! Servo's software WARP context can load pages on Windows, but WebGL
//! effects panic during ANGLE surface import before any pixels can be read
//! back, so the CPU path renders through a hidden window's offscreen
//! context instead.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::anyhow;
use dpi::PhysicalSize;
use paint_api::rendering_context::{RenderingContext, WindowRenderingContext};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use tao::event_loop::{EventLoop, EventLoopBuilder};
use tao::platform::windows::EventLoopBuilderExtWindows;
use tao::window::{Window, WindowBuilder};

thread_local! {
    static SERVO_RENDER_WINDOWS: RefCell<Vec<WindowsServoWindow>> = const { RefCell::new(Vec::new()) };
}

struct WindowsServoWindow {
    _event_loop: EventLoop<()>,
    _window: Window,
}

/// Create a hidden-window offscreen rendering context.
///
/// The window and its event loop are retained on the calling thread for the
/// life of the thread so the offscreen context's parent stays valid.
///
/// # Errors
///
/// Returns an error when the hidden window, its handles, or the Servo
/// window rendering context cannot be created.
pub fn hidden_window_rendering_context(
    width: u32,
    height: u32,
) -> anyhow::Result<Rc<dyn RenderingContext>> {
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
