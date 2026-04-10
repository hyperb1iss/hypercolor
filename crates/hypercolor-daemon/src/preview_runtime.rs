use hypercolor_core::bus::CanvasFrame;
use tokio::sync::watch;

#[derive(Clone, Debug)]
pub struct PreviewRuntime {
    canvas: watch::Sender<CanvasFrame>,
    screen_canvas: watch::Sender<CanvasFrame>,
}

impl PreviewRuntime {
    #[must_use]
    pub fn new() -> Self {
        let (canvas, _) = watch::channel(CanvasFrame::empty());
        let (screen_canvas, _) = watch::channel(CanvasFrame::empty());
        Self {
            canvas,
            screen_canvas,
        }
    }

    #[must_use]
    pub fn canvas_sender(&self) -> &watch::Sender<CanvasFrame> {
        &self.canvas
    }

    #[must_use]
    pub fn canvas_receiver(&self) -> watch::Receiver<CanvasFrame> {
        self.canvas.subscribe()
    }

    #[must_use]
    pub fn canvas_receiver_count(&self) -> usize {
        self.canvas.receiver_count()
    }

    #[must_use]
    pub fn screen_canvas_sender(&self) -> &watch::Sender<CanvasFrame> {
        &self.screen_canvas
    }

    #[must_use]
    pub fn screen_canvas_receiver(&self) -> watch::Receiver<CanvasFrame> {
        self.screen_canvas.subscribe()
    }

    #[must_use]
    pub fn screen_canvas_receiver_count(&self) -> usize {
        self.screen_canvas.receiver_count()
    }
}

impl Default for PreviewRuntime {
    fn default() -> Self {
        Self::new()
    }
}
