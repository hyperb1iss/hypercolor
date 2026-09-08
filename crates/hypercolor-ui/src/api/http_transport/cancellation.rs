//! Cooperative cancellation for browser-owned HTTP operations.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::task::{Context, Poll, Waker};

type Waiter = RefCell<Option<Waker>>;

#[derive(Default)]
struct State {
    cancelled: Cell<bool>,
    waiters: RefCell<Vec<Weak<Waiter>>>,
}

/// A clonable signal, independent of request and response body ownership.
#[derive(Clone, Default)]
pub struct HttpCancellation(Rc<State>);

impl HttpCancellation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Wake every pending read, write, and headers waiter. Repeated calls are harmless.
    pub fn cancel(&self) {
        if self.0.cancelled.replace(true) {
            return;
        }
        let waiters = std::mem::take(&mut *self.0.waiters.borrow_mut());
        for waiter in waiters {
            if let Some(waiter) = waiter.upgrade()
                && let Some(waker) = waiter.borrow_mut().take()
            {
                waker.wake();
            }
        }
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.get()
    }

    #[must_use]
    pub fn cancelled(&self) -> HttpCancelled {
        let waiter = Rc::new(RefCell::new(None));
        let mut waiters = self.0.waiters.borrow_mut();
        waiters.retain(|entry| entry.strong_count() != 0);
        waiters.push(Rc::downgrade(&waiter));
        HttpCancelled {
            cancellation: self.clone(),
            waiter,
        }
    }
}

/// Completes when its cancellation signal fires; dropping it unregisters the waiter.
pub struct HttpCancelled {
    cancellation: HttpCancellation,
    waiter: Rc<Waiter>,
}

impl Future for HttpCancelled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.cancellation.is_cancelled() {
            return Poll::Ready(());
        }
        self.waiter.borrow_mut().replace(context.waker().clone());
        Poll::Pending
    }
}
