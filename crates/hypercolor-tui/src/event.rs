//! Event reader — background task delivering terminal + timer events.

use std::time::Duration;

use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEvent, KeyEventKind, MouseEvent};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Events produced by the event reader.
#[derive(Debug)]
pub enum Event {
    /// A key was pressed.
    Key(KeyEvent),
    /// A mouse action occurred.
    Mouse(MouseEvent),
    /// The terminal was resized.
    Resize(u16, u16),
    /// Periodic tick for data refresh / animation.
    Tick,
    /// Time to redraw the screen.
    Render,
}

/// Reads terminal events and timers in a background task, forwarding them
/// through an unbounded channel.
pub struct EventReader {
    rx: mpsc::UnboundedReceiver<Event>,
    cancel: CancellationToken,
}

impl EventReader {
    /// Spawn the event reader with the given tick and render intervals.
    #[must_use]
    pub fn new(tick_rate: Duration, render_rate: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();

        tokio::spawn(async move {
            let mut event_stream = EventStream::new();
            let mut tick_interval = tokio::time::interval(tick_rate);
            let mut render_interval = tokio::time::interval(render_rate);

            tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            render_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                let event = tokio::select! {
                    () = task_cancel.cancelled() => break,
                    _ = tick_interval.tick() => Event::Tick,
                    _ = render_interval.tick() => Event::Render,
                    Some(Ok(crossterm_event)) = event_stream.next() => {
                        match crossterm_event {
                            CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                                Event::Key(key)
                            }
                            CrosstermEvent::Mouse(mouse) => Event::Mouse(mouse),
                            CrosstermEvent::Resize(w, h) => Event::Resize(w, h),
                            _ => continue,
                        }
                    }
                };

                if tx.send(event).is_err() {
                    break;
                }
            }
        });

        Self { rx, cancel }
    }

    /// Wait for the next event.
    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }

    /// Signal the background task to stop.
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl Drop for EventReader {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
