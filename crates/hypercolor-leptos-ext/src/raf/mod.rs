//! Browser-only animation frame helpers.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

const IDEAL_FRAME_MS: f64 = 1000.0 / 60.0;

#[derive(Clone)]
pub struct Scheduler {
    inner: Rc<SchedulerInner>,
}

struct SchedulerInner {
    window: web_sys::Window,
    callback: RefCell<Option<Closure<dyn FnMut(f64)>>>,
    pending_frame: Cell<Option<i32>>,
    active: Cell<bool>,
    user: RefCell<Box<dyn FnMut(FrameInfo)>>,
    last_time_ms: Cell<Option<f64>>,
    mode: Cell<Mode>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    OneShot,
    Continuous,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameInfo {
    pub time_ms: f64,
    pub monotonic_ms: f64,
    pub delta_ms: f64,
    pub dropped_frames: u32,
}

impl Scheduler {
    #[must_use]
    pub fn new<F>(callback: F) -> Self
    where
        F: FnMut(FrameInfo) + 'static,
    {
        let window = web_sys::window().expect("browser window is available for animation frames");
        Self {
            inner: Rc::new(SchedulerInner {
                window,
                callback: RefCell::new(None),
                pending_frame: Cell::new(None),
                active: Cell::new(false),
                user: RefCell::new(Box::new(callback)),
                last_time_ms: Cell::new(None),
                mode: Cell::new(Mode::Idle),
            }),
        }
    }

    pub fn schedule(&self) {
        if self.inner.pending_frame.get().is_some() {
            return;
        }
        self.inner.mode.set(Mode::OneShot);
        self.inner.active.set(true);
        request_next_frame(&self.inner);
    }

    pub fn schedule_continuous(&self) {
        if self.inner.pending_frame.get().is_some()
            && self.inner.mode.get() == Mode::Continuous
            && self.inner.active.get()
        {
            return;
        }
        self.inner.mode.set(Mode::Continuous);
        self.inner.active.set(true);
        request_next_frame(&self.inner);
    }

    pub fn pause(&self) {
        self.inner.pause();
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.inner.pending_frame.get().is_some()
    }
}

impl SchedulerInner {
    fn pause(&self) {
        self.active.set(false);
        self.mode.set(Mode::Idle);
        if let Some(request_id) = self.pending_frame.take() {
            let _ = self.window.cancel_animation_frame(request_id);
        }
        self.callback.borrow_mut().take();
    }

    fn monotonic_ms(&self, time_ms: f64) -> f64 {
        self.window
            .performance()
            .map_or(time_ms, |performance| performance.now())
    }
}

impl Drop for SchedulerInner {
    fn drop(&mut self) {
        if let Some(request_id) = self.pending_frame.take() {
            let _ = self.window.cancel_animation_frame(request_id);
        }
        self.callback.borrow_mut().take();
    }
}

fn request_next_frame(inner: &Rc<SchedulerInner>) {
    if inner.pending_frame.get().is_some() {
        return;
    }

    let weak = Rc::downgrade(inner);
    let callback = Closure::<dyn FnMut(f64)>::new(move |time_ms| {
        if let Some(inner) = Weak::upgrade(&weak) {
            on_animation_frame(&inner, time_ms);
        }
    });

    if let Ok(request_id) = inner
        .window
        .request_animation_frame(callback.as_ref().unchecked_ref())
    {
        inner.pending_frame.set(Some(request_id));
        inner.callback.borrow_mut().replace(callback);
    }
}

fn on_animation_frame(inner: &Rc<SchedulerInner>, time_ms: f64) {
    inner.pending_frame.take();

    if !inner.active.get() {
        inner.callback.borrow_mut().take();
        return;
    }

    let delta_ms = inner
        .last_time_ms
        .replace(Some(time_ms))
        .map_or(0.0, |previous| (time_ms - previous).max(0.0));
    let dropped_frames = estimate_dropped_frames(delta_ms);
    let frame = FrameInfo {
        time_ms,
        monotonic_ms: inner.monotonic_ms(time_ms),
        delta_ms,
        dropped_frames,
    };

    (inner.user.borrow_mut())(frame);

    let keep_running = inner.active.get() && inner.mode.get() == Mode::Continuous;
    if !keep_running {
        inner.active.set(false);
        inner.mode.set(Mode::Idle);
    }

    inner.callback.borrow_mut().take();

    if keep_running {
        request_next_frame(inner);
    }
}

fn estimate_dropped_frames(delta_ms: f64) -> u32 {
    if delta_ms <= IDEAL_FRAME_MS * 1.5 {
        return 0;
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    { (delta_ms / IDEAL_FRAME_MS).floor() as u32 }.saturating_sub(1)
}
