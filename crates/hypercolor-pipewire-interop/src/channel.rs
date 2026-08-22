use std::fmt;

/// Sending half of a channel serviced by the native capture loop.
pub struct LoopSender<C> {
    inner: platform::Sender<C>,
}

impl<C> Clone for LoopSender<C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<C> LoopSender<C> {
    /// Sends one command to the native capture loop.
    ///
    /// # Errors
    ///
    /// Returns the original command when the loop can no longer accept it.
    pub fn send(&self, command: C) -> Result<(), LoopSendError<C>> {
        platform::send(&self.inner, command).map_err(LoopSendError)
    }
}

/// Receiving half retained by the native capture loop.
pub struct LoopReceiver<C: 'static> {
    pub(crate) _inner: platform::Receiver<C>,
}

/// A command rejected by a closed native capture loop.
pub struct LoopSendError<C>(C);

impl<C> LoopSendError<C> {
    /// Returns the rejected command.
    #[must_use]
    pub fn into_inner(self) -> C {
        self.0
    }
}

impl<C> fmt::Debug for LoopSendError<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LoopSendError(..)")
    }
}

impl<C> fmt::Display for LoopSendError<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native capture loop is closed")
    }
}

impl<C: 'static> std::error::Error for LoopSendError<C> {}

/// Creates a command channel for one native capture loop.
#[must_use]
pub fn loop_channel<C: 'static>() -> (LoopSender<C>, LoopReceiver<C>) {
    let (sender, receiver) = platform::channel();
    (
        LoopSender { inner: sender },
        LoopReceiver { _inner: receiver },
    )
}

#[cfg(target_os = "linux")]
mod platform {
    pub(super) type Sender<C> = pipewire::channel::Sender<C>;
    pub(super) type Receiver<C> = pipewire::channel::Receiver<C>;

    pub(super) fn channel<C: 'static>() -> (Sender<C>, Receiver<C>) {
        pipewire::channel::channel()
    }

    pub(super) fn send<C>(sender: &Sender<C>, command: C) -> Result<(), C> {
        sender.send(command)
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    pub(super) type Sender<C> = std::sync::mpsc::Sender<C>;
    pub(super) type Receiver<C> = std::sync::mpsc::Receiver<C>;

    pub(super) fn channel<C: 'static>() -> (Sender<C>, Receiver<C>) {
        std::sync::mpsc::channel()
    }

    pub(super) fn send<C>(sender: &Sender<C>, command: C) -> Result<(), C> {
        sender.send(command).map_err(|error| error.0)
    }
}

#[cfg(test)]
mod tests {
    use super::loop_channel;

    #[test]
    fn sender_preserves_the_exact_command() {
        let (sender, receiver) = loop_channel();
        sender.send(41_u8).expect("open receiver accepts command");

        #[cfg(not(target_os = "linux"))]
        assert_eq!(receiver._inner.recv().expect("command remains queued"), 41);

        #[cfg(target_os = "linux")]
        drop(receiver);
    }

    #[test]
    fn closed_channel_returns_the_exact_command() {
        let (sender, receiver) = loop_channel();
        drop(receiver);

        let error = sender.send(String::from("retain me")).expect_err("closed");
        assert_eq!(error.into_inner(), "retain me");
    }
}
