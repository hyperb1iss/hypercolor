//! Servo-backed HTML effect renderer (feature-gated).
//!
//! The renderer runs Servo on a dedicated worker thread so the public
//! [`EffectRenderer`](super::EffectRenderer) impl stays `Send` while the
//! Servo runtime itself remains pinned to one OS thread. This submodule
//! splits the previously monolithic `servo_renderer.rs` into focused
//! units so the 2400-line pile can evolve without churning one file.
//!
//! Submodule map:
//!
//! - [`circuit_breaker`] — consecutive-failure tracker with exponential
//!   cooldown so transient faults can't poison the shared worker forever.
//! - [`delegate`] — `WebViewDelegate` implementation exposing frame
//!   readiness, console messages, and page-load state to the worker.
//! - [`renderer`] — `EffectRenderer` facade that drives the worker from
//!   the daemon's render loop.
//! - [`worker`] — OS thread spawn/teardown, `ServoWorkerRuntime`, and the
//!   shared `SERVO_WORKER` global.
//! - [`worker_client`] — client-side `Idle → Loading → Running → Stopping`
//!   state machine wrapping the command channel.

mod circuit_breaker;
mod delegate;
mod renderer;
mod session;
mod worker;
mod worker_client;

pub use delegate::{ConsoleMessage, HypercolorWebViewDelegate};
pub use renderer::ServoRenderer;
pub use session::{SessionConfig, ServoSessionHandle, note_servo_session_error};
