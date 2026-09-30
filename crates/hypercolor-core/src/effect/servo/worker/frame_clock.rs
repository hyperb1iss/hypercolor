//! Render-tick frame clock for Servo sessions.
//!
//! Servo withholds `notify_new_frame_ready` while an animating page waits on
//! its refresh driver, and paint is what sends `requestAnimationFrame` ticks.
//! The default driver is a 120Hz timer thread that runs independently of the
//! render loop, so readiness races the tick and the worker wakes mid-tick
//! whenever that timer fires. [`ServoFrameClock`] starts each frame from the
//! worker's own render tick instead.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use euclid::Size2D;
use euclid::default::Size2D as UntypedSize2D;
use gleam::gl::Gl;
use servo::{DeviceIntRect, DevicePixel, RefreshDriver, RenderingContext, RgbaImage};
use surfman::{Connection, Error, Surface, SurfaceTexture};

type FrameStartCallback = Box<dyn Fn() + Send + 'static>;

/// A [`RefreshDriver`] whose frames start when the worker ticks a session.
#[derive(Default)]
pub(super) struct ServoFrameClock {
    pending: RefCell<Vec<FrameStartCallback>>,
}

impl ServoFrameClock {
    /// Start a frame: run every callback Servo registered since the last one.
    pub(super) fn start_frame(&self) {
        let callbacks = std::mem::take(&mut *self.pending.borrow_mut());
        for callback in callbacks {
            callback();
        }
    }

    #[cfg(test)]
    pub(super) fn pending_callbacks(&self) -> usize {
        self.pending.borrow().len()
    }
}

impl RefreshDriver for ServoFrameClock {
    fn observe_next_frame(&self, start_frame_callback: FrameStartCallback) {
        self.pending.borrow_mut().push(start_frame_callback);
    }
}

/// Wraps a platform rendering context so Servo's painter uses the session's
/// [`ServoFrameClock`]. Every other method forwards to the platform context,
/// which keeps GPU import, readback, and WebGL surface plumbing unchanged.
pub(super) struct FrameClockRenderingContext {
    inner: Rc<dyn RenderingContext>,
    frame_clock: Rc<ServoFrameClock>,
}

impl FrameClockRenderingContext {
    pub(super) fn new(inner: Rc<dyn RenderingContext>, frame_clock: Rc<ServoFrameClock>) -> Self {
        Self { inner, frame_clock }
    }
}

impl RenderingContext for FrameClockRenderingContext {
    fn prepare_for_rendering(&self) {
        self.inner.prepare_for_rendering();
    }

    fn read_to_image(&self, source_rectangle: DeviceIntRect) -> Option<RgbaImage> {
        self.inner.read_to_image(source_rectangle)
    }

    fn size(&self) -> PhysicalSize<u32> {
        self.inner.size()
    }

    fn size2d(&self) -> Size2D<u32, DevicePixel> {
        self.inner.size2d()
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        self.inner.resize(size);
    }

    fn present(&self) {
        self.inner.present();
    }

    fn make_current(&self) -> Result<(), Error> {
        self.inner.make_current()
    }

    fn gleam_gl_api(&self) -> Rc<dyn Gl> {
        self.inner.gleam_gl_api()
    }

    fn glow_gl_api(&self) -> Arc<glow::Context> {
        self.inner.glow_gl_api()
    }

    fn create_texture(
        &self,
        surface: Surface,
    ) -> Option<(SurfaceTexture, u32, UntypedSize2D<i32>)> {
        self.inner.create_texture(surface)
    }

    fn destroy_texture(&self, surface_texture: SurfaceTexture) -> Option<Surface> {
        self.inner.destroy_texture(surface_texture)
    }

    fn connection(&self) -> Option<Connection> {
        self.inner.connection()
    }

    fn refresh_driver(&self) -> Option<Rc<dyn RefreshDriver>> {
        Some(Rc::clone(&self.frame_clock) as Rc<dyn RefreshDriver>)
    }
}
