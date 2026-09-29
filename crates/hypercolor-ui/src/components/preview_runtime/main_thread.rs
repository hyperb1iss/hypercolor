//! JPEG preview decoded on the main thread, for pages where the preview
//! worker cannot start. A Remote session streams JPEG on every path, so
//! this keeps its preview moving without falling back to raw pixels.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use hypercolor_leptos_ext::canvas::{
    bitmap_renderer_context, context_2d, set_canvas_size, supports_global,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobPropertyBag, CanvasRenderingContext2d, HtmlCanvasElement, ImageBitmap,
    ImageBitmapRenderingContext,
};

use crate::ws::{CanvasFrame, CanvasPixelFormat};

use super::worker::{DispatchDecision, FrameDispatchState};
use super::{PresentedHook, PreviewRenderOutcome, SubmittedFrame};

enum Surface {
    Bitmap(ImageBitmapRenderingContext),
    Canvas2d(CanvasRenderingContext2d),
}

struct Presenter {
    canvas: HtmlCanvasElement,
    surface: Surface,
    dispatch: RefCell<FrameDispatchState<SubmittedFrame>>,
    presented: Option<PresentedHook>,
    closed: Cell<bool>,
}

pub(super) struct MainThreadJpegRuntime {
    presenter: Rc<Presenter>,
}

impl MainThreadJpegRuntime {
    /// Present through the canvas's bitmap renderer when it has one (a
    /// failed worker start may already have claimed it), else through 2D.
    pub(super) fn new(
        canvas: &HtmlCanvasElement,
        presented: Option<PresentedHook>,
    ) -> Result<Self, ()> {
        if !supports_global("createImageBitmap") {
            return Err(());
        }
        let surface = bitmap_renderer_context(canvas)
            .map(Surface::Bitmap)
            .or_else(|| context_2d(canvas).map(Surface::Canvas2d))
            .ok_or(())?;
        Ok(Self {
            presenter: Rc::new(Presenter {
                canvas: canvas.clone(),
                surface,
                dispatch: RefCell::new(FrameDispatchState::default()),
                presented,
                closed: Cell::new(false),
            }),
        })
    }

    pub(super) fn render(&mut self, submitted: SubmittedFrame) -> PreviewRenderOutcome {
        if submitted.frame.pixel_format() != CanvasPixelFormat::Jpeg {
            return PreviewRenderOutcome::Reinitialize;
        }
        let decision = self
            .presenter
            .dispatch
            .borrow_mut()
            .push_or_defer(submitted);
        if decision == DispatchDecision::DispatchNow {
            let next = self.presenter.dispatch.borrow_mut().take_for_dispatch();
            if let Some(next) = next {
                decode_and_present(Rc::clone(&self.presenter), next);
            }
        }
        PreviewRenderOutcome::Presented
    }
}

impl Drop for MainThreadJpegRuntime {
    fn drop(&mut self) {
        self.presenter.closed.set(true);
    }
}

/// Decode one frame, present it, then continue with the newest frame that
/// arrived meanwhile. A frame that fails to decode is skipped rather than
/// stalling the frames behind it.
fn decode_and_present(presenter: Rc<Presenter>, submitted: SubmittedFrame) {
    wasm_bindgen_futures::spawn_local(async move {
        let decoded = decode_jpeg(&submitted.frame).await;
        if presenter.closed.get() {
            if let Ok(bitmap) = decoded {
                bitmap.close();
            }
            return;
        }
        if let Ok(bitmap) = decoded {
            presenter.present(&bitmap);
            bitmap.close();
            submitted.report(presenter.presented.as_ref());
        }
        let next = presenter.dispatch.borrow_mut().next_after_present();
        if let Some(next) = next {
            decode_and_present(presenter, next);
        }
    });
}

impl Presenter {
    fn present(&self, bitmap: &ImageBitmap) {
        set_canvas_size(&self.canvas, bitmap.width(), bitmap.height());
        match &self.surface {
            Surface::Bitmap(context) => context.transfer_from_image_bitmap(bitmap),
            Surface::Canvas2d(context) => {
                let _ = context.draw_image_with_image_bitmap(bitmap, 0.0, 0.0);
            }
        }
    }
}

async fn decode_jpeg(frame: &CanvasFrame) -> Result<ImageBitmap, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("window unavailable"))?;
    let parts = js_sys::Array::of1(frame.pixels_js());
    let options = BlobPropertyBag::new();
    options.set_type("image/jpeg");
    let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &options)?;
    JsFuture::from(window.create_image_bitmap_with_blob(&blob)?)
        .await?
        .dyn_into::<ImageBitmap>()
        .map_err(|_| JsValue::from_str("decoded preview is not an ImageBitmap"))
}
