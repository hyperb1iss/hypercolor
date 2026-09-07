//! Generic native MIDI worker infrastructure.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};
use std::time::{Duration, Instant};

use hypercolor_worker_retention::{retain_worker, spawn_worker};
use tokio::sync::oneshot;

use super::TransportError;

const NO_ACTIVE_REQUEST: u64 = 0;

/// Opaque driver-owned token used to correlate a MIDI response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MidiResponseToken(u32);

impl MidiResponseToken {
    /// Build a nonzero response token.
    #[must_use]
    pub const fn new(value: u32) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    /// Return the driver-owned token value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Driver callback used to match native MIDI responses to opaque tokens.
pub type MidiResponseMatcher = fn(MidiResponseToken, &[u8]) -> bool;

struct MidiResponseState {
    active_request: AtomicU64,
    next_generation: AtomicU32,
}

impl MidiResponseState {
    fn new() -> Self {
        Self {
            active_request: AtomicU64::new(NO_ACTIVE_REQUEST),
            next_generation: AtomicU32::new(0),
        }
    }

    fn arm(&self, token: MidiResponseToken) -> u64 {
        let generation = loop {
            let generation = self
                .next_generation
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);
            if generation != 0 {
                break generation;
            }
        };
        let request = (u64::from(generation) << 32) | u64::from(token.get());
        self.active_request.store(request, Ordering::Release);
        request
    }

    fn clear(&self) {
        self.active_request
            .store(NO_ACTIVE_REQUEST, Ordering::Release);
    }
}

#[derive(Debug, Eq, PartialEq)]
struct MidiResponse {
    request: u64,
    message: Vec<u8>,
}

/// Nonblocking ingress handle for a native MIDI input callback.
#[derive(Clone)]
pub struct MidiResponseIngress {
    state: Arc<MidiResponseState>,
    responses: std_mpsc::SyncSender<MidiResponse>,
    matcher: MidiResponseMatcher,
}

impl MidiResponseIngress {
    /// Forward one native callback payload when it matches the active request.
    pub fn forward(&self, message: &[u8]) {
        let request = self.state.active_request.load(Ordering::Acquire);
        if request == NO_ACTIVE_REQUEST {
            return;
        }
        let raw_token = u32::try_from(request & u64::from(u32::MAX))
            .expect("masked MIDI response token should fit in u32");
        let token = MidiResponseToken(raw_token);
        if !(self.matcher)(token, message) {
            return;
        }
        let response = MidiResponse {
            request,
            message: message.to_vec(),
        };
        if self.state.active_request.load(Ordering::Acquire) == request {
            let _ = self.responses.try_send(response);
        }
    }
}

/// Native MIDI session owned entirely by one retained worker thread.
pub trait NativeMidiSession: Send + 'static {
    /// Send one raw MIDI packet.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when native output fails.
    fn send(&mut self, packet: &[u8]) -> Result<(), TransportError>;

    /// Close native input and output resources.
    fn close(&mut self);
}

/// Native session and diagnostic port names returned by a driver opener.
pub struct OpenedMidiSession<S> {
    session: S,
    input_name: String,
    output_name: String,
}

impl<S> OpenedMidiSession<S> {
    /// Build an opened native MIDI session.
    #[must_use]
    pub fn new(session: S, input_name: String, output_name: String) -> Self {
        Self {
            session,
            input_name,
            output_name,
        }
    }
}

enum MidiWorkerCommand {
    Send {
        packet: Vec<u8>,
        response_token: Option<MidiResponseToken>,
        completion: oneshot::Sender<Result<(), TransportError>>,
    },
    SendReceive {
        packet: Vec<u8>,
        response_token: MidiResponseToken,
        timeout: Duration,
        completion: oneshot::Sender<Result<Vec<u8>, TransportError>>,
    },
    Receive {
        fallback_token: MidiResponseToken,
        timeout: Duration,
        completion: oneshot::Sender<Result<Vec<u8>, TransportError>>,
    },
    Close {
        completion: oneshot::Sender<()>,
    },
}

struct MidiWorkerOpened {
    input_name: String,
    output_name: String,
}

/// Async client for a retained native MIDI worker.
pub struct MidiWorkerClient {
    commands: std_mpsc::Sender<MidiWorkerCommand>,
    input_name: String,
    output_name: String,
}

impl MidiWorkerClient {
    /// Native input port name selected by the driver opener.
    #[must_use]
    pub fn input_name(&self) -> &str {
        &self.input_name
    }

    /// Native output port name selected by the driver opener.
    #[must_use]
    pub fn output_name(&self) -> &str {
        &self.output_name
    }

    /// Send one packet and optionally arm a later response receive.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the worker or native session fails.
    pub async fn send(
        &self,
        packet: Vec<u8>,
        response_token: Option<MidiResponseToken>,
    ) -> Result<(), TransportError> {
        let (completion, result) = oneshot::channel();
        self.commands
            .send(MidiWorkerCommand::Send {
                packet,
                response_token,
                completion,
            })
            .map_err(|_| TransportError::Closed)?;
        result.await.map_err(|_| worker_stopped("send"))?
    }

    /// Send one packet and wait for its correlated response.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the worker, native session, or response
    /// wait fails.
    pub async fn send_receive(
        &self,
        packet: Vec<u8>,
        response_token: MidiResponseToken,
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        let (completion, result) = oneshot::channel();
        self.commands
            .send(MidiWorkerCommand::SendReceive {
                packet,
                response_token,
                timeout,
                completion,
            })
            .map_err(|_| TransportError::Closed)?;
        result.await.map_err(|_| worker_stopped("request"))?
    }

    /// Receive the response armed by the previous send.
    ///
    /// The fallback token is armed only when no request is pending.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the worker or response wait fails.
    pub async fn receive(
        &self,
        timeout: Duration,
        fallback_token: MidiResponseToken,
    ) -> Result<Vec<u8>, TransportError> {
        let (completion, result) = oneshot::channel();
        self.commands
            .send(MidiWorkerCommand::Receive {
                fallback_token,
                timeout,
                completion,
            })
            .map_err(|_| TransportError::Closed)?;
        result.await.map_err(|_| worker_stopped("receive"))?
    }

    /// Close the worker-owned native session.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the worker exits before acknowledging
    /// the close.
    pub async fn close(&self) -> Result<(), TransportError> {
        let (completion, result) = oneshot::channel();
        self.commands
            .send(MidiWorkerCommand::Close { completion })
            .map_err(|_| TransportError::Closed)?;
        result.await.map_err(|_| worker_stopped("close"))
    }
}

/// Open a native MIDI session on a retained worker thread.
///
/// The driver supplies native port selection, a response matcher, and an
/// arbitrary lifetime guard retained until the worker exits.
///
/// # Errors
///
/// Returns [`TransportError`] when the worker cannot start or the driver
/// opener fails.
pub async fn open_native_midi_worker<S, G, F>(
    thread_name: String,
    worker_context: String,
    lifetime_guard: G,
    response_queue_depth: usize,
    matcher: MidiResponseMatcher,
    open: F,
) -> Result<MidiWorkerClient, TransportError>
where
    S: NativeMidiSession,
    G: Send + 'static,
    F: FnOnce(MidiResponseIngress) -> Result<OpenedMidiSession<S>, TransportError> + Send + 'static,
{
    let (commands, command_rx) = std_mpsc::channel();
    let state = Arc::new(MidiResponseState::new());
    let (response_tx, response_rx) = std_mpsc::sync_channel(response_queue_depth);
    let ingress = MidiResponseIngress {
        state: Arc::clone(&state),
        responses: response_tx,
        matcher,
    };
    let (opened_tx, opened_rx) = oneshot::channel();
    let worker = spawn_worker(std::thread::Builder::new().name(thread_name), move || {
        run_native_midi_worker(
            lifetime_guard,
            open,
            ingress,
            state,
            response_rx,
            command_rx,
            opened_tx,
        );
    })
    .map_err(|error| TransportError::IoError {
        detail: format!("failed to start native MIDI worker: {error}"),
    })?;
    retain_worker(worker, worker_context);

    let opened = opened_rx.await.map_err(|_| worker_stopped("open"))??;
    Ok(MidiWorkerClient {
        commands,
        input_name: opened.input_name,
        output_name: opened.output_name,
    })
}

fn run_native_midi_worker<S, G, F>(
    _lifetime_guard: G,
    open: F,
    ingress: MidiResponseIngress,
    state: Arc<MidiResponseState>,
    responses: std_mpsc::Receiver<MidiResponse>,
    commands: std_mpsc::Receiver<MidiWorkerCommand>,
    opened: oneshot::Sender<Result<MidiWorkerOpened, TransportError>>,
) where
    S: NativeMidiSession,
    F: FnOnce(MidiResponseIngress) -> Result<OpenedMidiSession<S>, TransportError>,
{
    let opened_session = match open(ingress) {
        Ok(opened_session) => opened_session,
        Err(error) => {
            let _ = opened.send(Err(error));
            return;
        }
    };
    let OpenedMidiSession {
        mut session,
        input_name,
        output_name,
    } = opened_session;
    let metadata = MidiWorkerOpened {
        input_name,
        output_name,
    };
    if opened.send(Ok(metadata)).is_err() {
        session.close();
        return;
    }

    while let Ok(command) = commands.recv() {
        match command {
            MidiWorkerCommand::Send {
                packet,
                response_token,
                completion,
            } => {
                if let Some(token) = response_token {
                    arm_response(state.as_ref(), &responses, token);
                } else {
                    disarm_response(state.as_ref(), &responses);
                }
                let result = session.send(&packet);
                if result.is_err() {
                    state.clear();
                }
                let _ = completion.send(result);
            }
            MidiWorkerCommand::SendReceive {
                packet,
                response_token,
                timeout,
                completion,
            } => {
                let request = arm_response(state.as_ref(), &responses, response_token);
                let result = session
                    .send(&packet)
                    .and_then(|()| receive_matching_response(&responses, request, timeout));
                state.clear();
                let _ = completion.send(result);
            }
            MidiWorkerCommand::Receive {
                fallback_token,
                timeout,
                completion,
            } => {
                let active = state.active_request.load(Ordering::Acquire);
                let request = if active == NO_ACTIVE_REQUEST {
                    arm_response(state.as_ref(), &responses, fallback_token)
                } else {
                    active
                };
                let result = receive_matching_response(&responses, request, timeout);
                state.clear();
                let _ = completion.send(result);
            }
            MidiWorkerCommand::Close { completion } => {
                state.clear();
                session.close();
                let _ = completion.send(());
                return;
            }
        }
    }
    state.clear();
    session.close();
}

fn arm_response(
    state: &MidiResponseState,
    responses: &std_mpsc::Receiver<MidiResponse>,
    token: MidiResponseToken,
) -> u64 {
    while responses.try_recv().is_ok() {}
    state.arm(token)
}

fn disarm_response(state: &MidiResponseState, responses: &std_mpsc::Receiver<MidiResponse>) {
    while responses.try_recv().is_ok() {}
    state.clear();
}

fn receive_matching_response(
    responses: &std_mpsc::Receiver<MidiResponse>,
    request: u64,
    timeout: Duration,
) -> Result<Vec<u8>, TransportError> {
    let started = Instant::now();
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        match responses.recv_timeout(remaining) {
            Ok(response) if response.request == request => return Ok(response.message),
            Ok(_) => {}
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                return Err(TransportError::Timeout {
                    timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                });
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                return Err(TransportError::Closed);
            }
        }
    }
}

fn worker_stopped(operation: &str) -> TransportError {
    TransportError::IoError {
        detail: format!("native MIDI worker stopped before {operation} completed"),
    }
}

/// Close native MIDI input before output.
pub fn close_input_before_output<I, O>(
    input: &mut Option<I>,
    output: &mut Option<O>,
    close_input: impl FnOnce(I),
    close_output: impl FnOnce(O),
) {
    if let Some(input) = input.take() {
        close_input(input);
    }
    if let Some(output) = output.take() {
        close_output(output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const ANY_RESPONSE: MidiResponseToken = MidiResponseToken(1);

    fn any_response(_token: MidiResponseToken, message: &[u8]) -> bool {
        !message.is_empty()
    }

    #[test]
    fn callback_ingress_is_nonblocking_and_bounded() {
        let state = Arc::new(MidiResponseState::new());
        let (responses, rx) = std_mpsc::sync_channel(2);
        let ingress = MidiResponseIngress {
            state: Arc::clone(&state),
            responses,
            matcher: any_response,
        };

        ingress.forward(&[0x01]);
        assert_eq!(rx.try_recv(), Err(std_mpsc::TryRecvError::Empty));

        state.arm(ANY_RESPONSE);
        for value in 0..128_u8 {
            ingress.forward(&[value]);
        }
        assert_eq!(rx.try_recv().map(|response| response.message), Ok(vec![0]));
        assert_eq!(rx.try_recv().map(|response| response.message), Ok(vec![1]));
        assert_eq!(rx.try_recv(), Err(std_mpsc::TryRecvError::Empty));
    }

    #[test]
    fn callback_generation_rejects_response_racing_a_rearm() {
        let state = Arc::new(MidiResponseState::new());
        let (responses, rx) = std_mpsc::sync_channel(2);
        let ingress = MidiResponseIngress {
            state: Arc::clone(&state),
            responses,
            matcher: any_response,
        };
        let first_request = state.arm(ANY_RESPONSE);
        let stale = MidiResponse {
            request: first_request,
            message: vec![0x01],
        };
        let second_request = state.arm(ANY_RESPONSE);
        ingress
            .responses
            .try_send(stale)
            .expect("stale response queues");
        ingress.forward(&[0x02]);

        let response = receive_matching_response(&rx, second_request, Duration::from_secs(1))
            .expect("current generation response is returned");
        assert_eq!(response, vec![0x02]);
    }

    #[test]
    fn close_always_stops_input_before_output() {
        let order = RefCell::new(Vec::new());
        let mut input = Some(());
        let mut output = Some(());
        close_input_before_output(
            &mut input,
            &mut output,
            |()| order.borrow_mut().push("input"),
            |()| order.borrow_mut().push("output"),
        );

        assert_eq!(order.into_inner(), vec!["input", "output"]);
        assert!(input.is_none());
        assert!(output.is_none());
    }
}
