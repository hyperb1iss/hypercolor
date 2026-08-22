use std::cell::Cell;
use std::os::fd::OwnedFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use pipewire as pw;
use pw::properties::properties;

use super::{ProcessBuffer, format};
use crate::{
    CallbackAction, FormatEvent, FormatOffer, LoopReceiver, PortalRemote, StateChange,
    StreamConnectError, StreamError, StreamEventHandler, StreamState,
};

type NativeStream<H> = (
    pw::main_loop::MainLoopRc,
    pw::stream::StreamRc,
    pw::stream::StreamListener<H>,
    Rc<CallbackExit>,
);

#[derive(Default)]
struct CallbackExit {
    failure: Cell<Option<&'static str>>,
    quit: Cell<bool>,
}

/// Opaque synchronous control over the active native stream.
pub struct StreamControl<'a> {
    stream: &'a pw::stream::Stream,
}

impl StreamControl<'_> {
    /// Activates or deactivates native buffer production.
    ///
    /// # Errors
    ///
    /// Returns a native operation error when PipeWire rejects the change.
    pub fn set_active(&self, active: bool) -> Result<(), StreamError> {
        self.stream
            .set_active(active)
            .map_err(|error| operation("failed to update PipeWire stream activity", error))
    }

    /// Replaces the active native format offer.
    ///
    /// # Errors
    ///
    /// Returns a native operation error when serialization or update fails.
    pub fn update_format(&self, offer: &FormatOffer) -> Result<(), StreamError> {
        let bytes = format::serialize_offer(offer)?;
        let pod = pw::spa::pod::Pod::from_bytes(&bytes).ok_or_else(|| StreamError::Operation {
            operation: "failed to deserialize PipeWire format offer",
            detail: "serialized offer was not a complete SPA pod".to_owned(),
        })?;
        self.stream
            .update_params(&mut [pod])
            .map_err(|error| operation("failed to update PipeWire format", error))
    }
}

/// One owned native stream session and its retained command lane.
pub struct StreamSession<C: 'static, H, K>
where
    H: StreamEventHandler,
    K: Fn(&StreamControl<'_>, C) -> CallbackAction + 'static,
{
    // The native hook must unlink before PipeWire destroys its stream.
    _listener: pw::stream::StreamListener<H>,
    stream: pw::stream::StreamRc,
    mainloop: pw::main_loop::MainLoopRc,
    receiver: Option<LoopReceiver<C>>,
    command_handler: Option<K>,
    callback_exit: Rc<CallbackExit>,
    has_run: bool,
}

impl<C: 'static, H, K> StreamSession<C, H, K>
where
    H: StreamEventHandler,
    K: Fn(&StreamControl<'_>, C) -> CallbackAction + 'static,
{
    /// Runs the native loop once and restores the detached command receiver.
    ///
    /// # Errors
    ///
    /// Returns an error when this session has already run.
    pub fn run(&mut self) -> Result<(), StreamError> {
        if self.has_run {
            return Err(StreamError::Operation {
                operation: "failed to run PipeWire stream",
                detail: "stream session already ran".to_owned(),
            });
        }
        self.has_run = true;
        if let Some(callback) = self.callback_exit.failure.take() {
            return Err(StreamError::CallbackPanicked { callback });
        }
        if self.callback_exit.quit.take() {
            return Ok(());
        }
        let receiver = self
            .receiver
            .take()
            .expect("stream session owns its receiver before the loop runs");
        let command_handler = self
            .command_handler
            .take()
            .expect("stream session owns its command handler before the loop runs");
        let callback_exit = Rc::clone(&self.callback_exit);
        let mainloop = self.mainloop.clone();
        let stream = self.stream.clone();
        let attached = receiver
            ._inner
            .attach(self.mainloop.loop_(), move |command| {
                let control = StreamControl { stream: &stream };
                if invoke_callback("command", &callback_exit, || {
                    command_handler(&control, command)
                }) == CallbackAction::Quit
                {
                    mainloop.quit();
                }
            });
        self.mainloop.run();
        self.receiver = Some(LoopReceiver {
            _inner: attached.deattach(),
        });
        self.callback_exit
            .failure
            .take()
            .map_or(Ok(()), |callback| {
                Err(StreamError::CallbackPanicked { callback })
            })
    }

    /// Disconnects the native stream and returns the command receiver.
    pub fn disconnect(self) -> (LoopReceiver<C>, Result<(), StreamError>) {
        let native_result = self
            .stream
            .disconnect()
            .map_err(|error| operation("failed to disconnect PipeWire stream", error));
        let result = match self.callback_exit.failure.take() {
            Some(callback) => Err(StreamError::CallbackPanicked { callback }),
            None => native_result,
        };
        let receiver = self
            .receiver
            .expect("stream session owns its receiver outside the running loop");
        (receiver, result)
    }
}

/// Constructs and connects one native capture stream.
///
/// # Errors
///
/// Returns the native failure together with the retained command receiver.
pub fn connect_stream<C, H, K>(
    remote: PortalRemote,
    offer: &FormatOffer,
    receiver: LoopReceiver<C>,
    handler: H,
    command_handler: K,
) -> Result<StreamSession<C, H, K>, StreamConnectError<C>>
where
    C: 'static,
    H: StreamEventHandler,
    K: Fn(&StreamControl<'_>, C) -> CallbackAction + 'static,
{
    match connect_native(remote, offer, handler) {
        Ok((mainloop, stream, listener, callback_exit)) => Ok(StreamSession {
            mainloop,
            stream,
            _listener: listener,
            receiver: Some(receiver),
            command_handler: Some(command_handler),
            callback_exit,
            has_run: false,
        }),
        Err(error) => Err(StreamConnectError::new(error, receiver)),
    }
}

fn connect_native<H>(
    remote: PortalRemote,
    offer: &FormatOffer,
    handler: H,
) -> Result<NativeStream<H>, StreamError>
where
    H: StreamEventHandler,
{
    pw::init();
    let (descriptor, file) = remote.into_parts();
    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|error| operation("failed to create PipeWire main loop", error))?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|error| operation("failed to create PipeWire context", error))?;
    let fd: OwnedFd = file.into();
    let core = context
        .connect_fd_rc(fd, None)
        .map_err(|error| operation("failed to connect to screencast PipeWire remote", error))?;
    let stream = pw::stream::StreamRc::new(
        core,
        "hypercolor-screen-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|error| operation("failed to create PipeWire capture stream", error))?;
    let callback_exit = Rc::new(CallbackExit::default());
    let listener = stream
        .add_local_listener_with_user_data(handler)
        .param_changed({
            let mainloop = mainloop.clone();
            let callback_exit = Rc::clone(&callback_exit);
            move |stream, handler, id, param| {
                if id != pw::spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let control = StreamControl { stream };
                let event: FormatEvent = format::parse_event(param);
                if invoke_callback("format", &callback_exit, || {
                    handler.format_changed(&control, event)
                }) == CallbackAction::Quit
                {
                    mainloop.quit();
                }
            }
        })
        .state_changed({
            let mainloop = mainloop.clone();
            let callback_exit = Rc::clone(&callback_exit);
            move |_, handler, previous, current| {
                let event = StateChange {
                    previous: stream_state(previous),
                    current: stream_state(current),
                };
                if invoke_callback("state", &callback_exit, || handler.state_changed(event))
                    == CallbackAction::Quit
                {
                    mainloop.quit();
                }
            }
        })
        .process({
            let mainloop = mainloop.clone();
            let callback_exit = Rc::clone(&callback_exit);
            move |stream, handler| {
                if invoke_callback("process", &callback_exit, || {
                    handler.process(ProcessBuffer::new(stream))
                }) == CallbackAction::Quit
                {
                    mainloop.quit();
                }
            }
        })
        .register()
        .map_err(|error| operation("failed to register PipeWire capture listener", error))?;

    let bytes = format::serialize_offer(offer)?;
    let pod = pw::spa::pod::Pod::from_bytes(&bytes).ok_or_else(|| StreamError::Operation {
        operation: "failed to deserialize PipeWire format offer",
        detail: "serialized offer was not a complete SPA pod".to_owned(),
    })?;
    let native_result = stream
        .connect(
            pw::spa::utils::Direction::Input,
            Some(descriptor.node_id()),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut [pod],
        )
        .map_err(|error| operation("failed to connect PipeWire capture stream", error));
    if let Err(error) = native_result {
        return Err(match callback_exit.failure.take() {
            Some(callback) => StreamError::CallbackPanicked { callback },
            None => error,
        });
    }
    Ok((mainloop, stream, listener, callback_exit))
}

fn invoke_callback(
    callback: &'static str,
    exit: &CallbackExit,
    invoke: impl FnOnce() -> CallbackAction,
) -> CallbackAction {
    match catch_unwind(AssertUnwindSafe(invoke)) {
        Ok(action) => {
            if action == CallbackAction::Quit {
                exit.quit.set(true);
            }
            action
        }
        Err(_) => {
            if exit.failure.get().is_none() {
                exit.failure.set(Some(callback));
            }
            exit.quit.set(true);
            CallbackAction::Quit
        }
    }
}

fn stream_state(state: pw::stream::StreamState) -> StreamState {
    match state {
        pw::stream::StreamState::Unconnected => StreamState::Unconnected,
        pw::stream::StreamState::Connecting => StreamState::Connecting,
        pw::stream::StreamState::Paused => StreamState::Paused,
        pw::stream::StreamState::Streaming => StreamState::Streaming,
        pw::stream::StreamState::Error(error) => StreamState::Error(error.to_owned()),
    }
}

fn operation(operation: &'static str, error: impl std::fmt::Display) -> StreamError {
    StreamError::Operation {
        operation,
        detail: error.to_string(),
    }
}
