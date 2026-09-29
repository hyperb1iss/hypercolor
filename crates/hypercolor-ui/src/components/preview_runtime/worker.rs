use std::cell::{Cell, RefCell};
use std::rc::Rc;

use hypercolor_leptos_ext::canvas::{
    bitmap_renderer_context, message_image_bitmap, revoke_blob_url, set_canvas_size,
    supports_global, supports_offscreen_canvas_2d_bitmap,
};
use hypercolor_leptos_ext::events::{WorkerMessageHandler, post_worker_canvas_frame};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{HtmlCanvasElement, ImageBitmapRenderingContext, MessageEvent, Worker};

use crate::ws::{CanvasFrame, CanvasPixelFormat};

use super::{PresentedHook, PreviewRenderOutcome, SubmittedFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DispatchDecision {
    DispatchNow,
    Deferred,
}

/// Latest-wins hand-off to an asynchronous presenter: one frame in flight,
/// and at most one newer frame waiting behind it.
pub(super) struct FrameDispatchState<T> {
    in_flight: bool,
    queued: Option<T>,
}

impl<T> Default for FrameDispatchState<T> {
    fn default() -> Self {
        Self {
            in_flight: false,
            queued: None,
        }
    }
}

impl<T> FrameDispatchState<T> {
    pub(super) fn push_or_defer(&mut self, frame: T) -> DispatchDecision {
        if self.in_flight {
            self.queued = Some(frame);
            DispatchDecision::Deferred
        } else {
            self.in_flight = true;
            self.queued = Some(frame);
            DispatchDecision::DispatchNow
        }
    }

    pub(super) fn take_for_dispatch(&mut self) -> Option<T> {
        self.queued.take()
    }

    pub(super) fn next_after_present(&mut self) -> Option<T> {
        if self.queued.is_some() {
            self.in_flight = true;
            return self.queued.take();
        }

        self.in_flight = false;
        None
    }
}

const PREVIEW_WORKER_SOURCE: &str = r#"
let canvas = null;
let ctx = null;
let latestFrame = null;
let framePending = false;
let scratchRgba = null;

self.onmessage = (event) => {
  const frame = decodeFrame(event.data);
  if (!frame) {
    return;
  }

  latestFrame = frame;
  if (!framePending) {
    framePending = true;
    scheduleFlush();
  }
};

function decodeFrame(data) {
  if (!Array.isArray(data) || data.length !== 4) {
    return null;
  }

  const width = data[0] >>> 0;
  const height = data[1] >>> 0;
  const format = data[2] | 0;
  const pixels = data[3];
  if (!(pixels instanceof Uint8Array)) {
    return null;
  }

  return { width, height, format, pixels };
}

function scheduleFlush() {
  if (typeof self.requestAnimationFrame === "function") {
    self.requestAnimationFrame(flushFrame);
    return;
  }

  self.setTimeout(flushFrame, 0);
}

// Answer every frame with a bitmap or null, so the page never waits on a
// frame that failed.
async function flushFrame() {
  framePending = false;
  const frame = latestFrame;
  if (!frame) {
    return;
  }

  let bitmap = null;
  try {
    bitmap = frame.format === 2 ? await createJpegBitmap(frame) : rasterizeFrame(frame);
  } catch {
    bitmap = null;
  }

  if (bitmap) {
    self.postMessage(bitmap, [bitmap]);
  } else {
    self.postMessage(null);
  }
}

function rasterizeFrame(frame) {
  if (!ensureCanvas(frame.width, frame.height)) {
    return null;
  }

  const imageData = createImageData(frame);
  if (!imageData) {
    return null;
  }

  ctx.putImageData(imageData, 0, 0);
  return canvas.transferToImageBitmap();
}

async function createJpegBitmap(frame) {
  if (typeof createImageBitmap !== "function") {
    return null;
  }

  try {
    const blob = new Blob([frame.pixels], { type: "image/jpeg" });
    return await createImageBitmap(blob);
  } catch {
    return null;
  }
}

function ensureCanvas(width, height) {
  if (!canvas) {
    if (typeof OffscreenCanvas !== "function") {
      return false;
    }

    canvas = new OffscreenCanvas(width, height);
    ctx = canvas.getContext("2d", { alpha: false, desynchronized: true });
    if (!ctx) {
      canvas = null;
      return false;
    }
  }

  if (canvas.width !== width) {
    canvas.width = width;
  }
  if (canvas.height !== height) {
    canvas.height = height;
  }

  return true;
}

function createImageData(frame) {
  const pixels = frame.pixels;
  if (!(pixels instanceof Uint8Array)) {
    return null;
  }

  if (frame.format === 1) {
    return new ImageData(
      new Uint8ClampedArray(pixels.buffer, pixels.byteOffset, pixels.byteLength),
      frame.width,
      frame.height,
    );
  }

  if (frame.format !== 0) {
    return null;
  }

  const requiredLength = frame.width * frame.height * 4;
  if (!scratchRgba || scratchRgba.length !== requiredLength) {
    scratchRgba = new Uint8ClampedArray(requiredLength);
  }

  for (let src = 0, dst = 0; src + 2 < pixels.length; src += 3, dst += 4) {
    scratchRgba[dst] = pixels[src];
    scratchRgba[dst + 1] = pixels[src + 1];
    scratchRgba[dst + 2] = pixels[src + 2];
    scratchRgba[dst + 3] = 255;
  }

  return new ImageData(scratchRgba, frame.width, frame.height);
}

"#;

/// Consecutive frames the worker may fail to decode before it counts as
/// broken. One bad frame is skipped; a worker that can decode nothing is
/// replaced.
const MAX_CONSECUTIVE_SKIPS: u32 = 3;

pub(super) struct PreviewWorkerRuntime {
    worker: Worker,
    worker_url: String,
    failed: Rc<Cell<bool>>,
    broken: Rc<Cell<bool>>,
    dispatch_state: Rc<RefCell<FrameDispatchState<SubmittedFrame>>>,
    in_flight: Rc<RefCell<Option<SubmittedFrame>>>,
    last_shape: Option<(u32, u32, CanvasPixelFormat)>,
    onmessage: WorkerMessageHandler,
    _onerror: Closure<dyn FnMut(JsValue)>,
}

impl PreviewWorkerRuntime {
    pub(super) fn new(
        canvas: &HtmlCanvasElement,
        frame: &CanvasFrame,
        presented: Option<PresentedHook>,
    ) -> Result<Self, ()> {
        set_canvas_size(canvas, frame.width, frame.height);
        let bitmap_ctx = bitmap_renderer_context(canvas).ok_or(())?;
        probe_worker_support(frame.pixel_format())?;

        let (worker, worker_url) = create_worker().map_err(|_| ())?;
        let failed = Rc::new(Cell::new(false));
        let broken = Rc::new(Cell::new(false));
        let dispatch_state = Rc::new(RefCell::new(FrameDispatchState::<SubmittedFrame>::default()));
        let in_flight = Rc::new(RefCell::new(None::<SubmittedFrame>));
        let consecutive_skips = Cell::new(0_u32);
        let failed_handle = Rc::clone(&failed);
        let broken_handle = Rc::clone(&broken);
        let dispatch_state_handle = Rc::clone(&dispatch_state);
        let in_flight_handle = Rc::clone(&in_flight);
        let canvas_handle = canvas.clone();
        let bitmap_ctx_handle = bitmap_ctx.clone();
        let worker_handle = worker.clone();

        let onmessage = WorkerMessageHandler::attach(&worker, move |event| {
            let shown = in_flight_handle.borrow_mut().take();
            if event.data().is_null() {
                consecutive_skips.set(consecutive_skips.get() + 1);
                if consecutive_skips.get() >= MAX_CONSECUTIVE_SKIPS {
                    failed_handle.set(true);
                    broken_handle.set(true);
                    return;
                }
            } else if present_bitmap(&canvas_handle, &bitmap_ctx_handle, &event) {
                consecutive_skips.set(0);
                if let Some(shown) = shown {
                    shown.report(presented.as_ref());
                }
            } else {
                failed_handle.set(true);
                return;
            }

            let next_frame = dispatch_state_handle.borrow_mut().next_after_present();
            if let Some(next) = next_frame {
                let posted = post_frame(&worker_handle, &next.frame);
                in_flight_handle.borrow_mut().replace(next);
                if posted.is_err() {
                    failed_handle.set(true);
                }
            }
        });

        // A worker whose script fails to load or throws reports only through
        // this event; without it the page would wait on its reply forever.
        let failed_on_error = Rc::clone(&failed);
        let broken_on_error = Rc::clone(&broken);
        let onerror = Closure::<dyn FnMut(JsValue)>::new(move |_| {
            failed_on_error.set(true);
            broken_on_error.set(true);
        });
        worker.set_onerror(Some(onerror.as_ref().unchecked_ref()));

        Ok(Self {
            worker,
            worker_url,
            failed,
            broken,
            dispatch_state,
            in_flight,
            last_shape: None,
            onmessage,
            _onerror: onerror,
        })
    }

    /// The worker failed on its own rather than on a frame shape change, so
    /// recreating it would fail the same way.
    pub(super) fn is_broken(&self) -> bool {
        self.broken.get()
    }

    pub(super) fn render(&mut self, submitted: SubmittedFrame) -> PreviewRenderOutcome {
        if self.failed.get() {
            return PreviewRenderOutcome::Reinitialize;
        }

        let frame = &submitted.frame;
        let next_shape = (frame.width, frame.height, frame.pixel_format());
        if self
            .last_shape
            .is_some_and(|shape| shape_change_needs_new_worker(shape, next_shape))
        {
            self.failed.set(true);
            return PreviewRenderOutcome::Reinitialize;
        }

        let decision = self.dispatch_state.borrow_mut().push_or_defer(submitted);
        if decision == DispatchDecision::DispatchNow {
            let next = self
                .dispatch_state
                .borrow_mut()
                .take_for_dispatch()
                .expect("dispatch-now state should hold the frame being sent");
            let posted = post_frame(&self.worker, &next.frame);
            self.in_flight.borrow_mut().replace(next);
            if posted.is_err() {
                self.failed.set(true);
                return PreviewRenderOutcome::Reinitialize;
            }
        }

        self.last_shape = Some(next_shape);
        PreviewRenderOutcome::Presented
    }
}

/// Every JPEG decodes on its own and presents at the bitmap's size, so only
/// a format change, or a raw frame changing size, needs a fresh worker.
fn shape_change_needs_new_worker(
    previous: (u32, u32, CanvasPixelFormat),
    next: (u32, u32, CanvasPixelFormat),
) -> bool {
    previous.2 != next.2 || (next.2 != CanvasPixelFormat::Jpeg && previous != next)
}

fn probe_worker_canvas_support() -> Result<(), ()> {
    if supports_offscreen_canvas_2d_bitmap() {
        Ok(())
    } else {
        Err(())
    }
}

fn probe_worker_jpeg_support() -> Result<(), ()> {
    supports_global("createImageBitmap").then_some(()).ok_or(())
}

fn probe_worker_support(format: CanvasPixelFormat) -> Result<(), ()> {
    match format {
        CanvasPixelFormat::Jpeg => probe_worker_jpeg_support(),
        CanvasPixelFormat::Rgb | CanvasPixelFormat::Rgba => probe_worker_canvas_support(),
    }
}

impl Drop for PreviewWorkerRuntime {
    fn drop(&mut self) {
        self.onmessage.detach_from(&self.worker);
        self.worker.set_onerror(None);
        self.worker.terminate();
        revoke_blob_url(&self.worker_url);
    }
}

fn create_worker() -> Result<(Worker, String), JsValue> {
    let (worker, url) = tachys::renderer::dom::create_static_worker(PREVIEW_WORKER_SOURCE)?;
    Ok((worker.dyn_into()?, url))
}

fn post_frame(worker: &Worker, frame: &CanvasFrame) -> Result<(), JsValue> {
    post_worker_canvas_frame(
        worker,
        frame.width,
        frame.height,
        frame.pixel_format().tag(),
        frame.pixels_js(),
    )
}

fn present_bitmap(
    canvas: &HtmlCanvasElement,
    bitmap_ctx: &ImageBitmapRenderingContext,
    event: &MessageEvent,
) -> bool {
    let Some(bitmap) = message_image_bitmap(event) else {
        return false;
    };

    set_canvas_size(canvas, bitmap.width(), bitmap.height());
    bitmap_ctx.transfer_from_image_bitmap(&bitmap);
    bitmap.close();
    true
}

#[cfg(test)]
mod tests {
    use super::{DispatchDecision, FrameDispatchState, shape_change_needs_new_worker};
    use crate::ws::CanvasPixelFormat;

    #[test]
    fn jpeg_size_changes_keep_the_worker() {
        let jpeg = |width, height| (width, height, CanvasPixelFormat::Jpeg);
        let rgba = |width, height| (width, height, CanvasPixelFormat::Rgba);
        assert!(!shape_change_needs_new_worker(
            jpeg(320, 240),
            jpeg(480, 360)
        ));
        assert!(!shape_change_needs_new_worker(
            jpeg(480, 360),
            jpeg(480, 360)
        ));
        assert!(shape_change_needs_new_worker(
            jpeg(480, 360),
            rgba(480, 360)
        ));
        assert!(shape_change_needs_new_worker(
            rgba(320, 240),
            rgba(480, 360)
        ));
        assert!(!shape_change_needs_new_worker(
            rgba(320, 240),
            rgba(320, 240)
        ));
    }

    #[test]
    fn dispatch_state_coalesces_frames_while_one_is_in_flight() {
        let mut state = FrameDispatchState::default();

        assert_eq!(state.push_or_defer(1), DispatchDecision::DispatchNow);
        assert_eq!(state.take_for_dispatch(), Some(1));
        assert_eq!(state.push_or_defer(2), DispatchDecision::Deferred);
        assert_eq!(state.push_or_defer(3), DispatchDecision::Deferred);
        assert_eq!(state.next_after_present(), Some(3));
        assert_eq!(state.next_after_present(), None);
    }
}
