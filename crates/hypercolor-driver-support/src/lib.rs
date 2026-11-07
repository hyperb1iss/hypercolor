//! Native host utilities shared by Hypercolor driver implementations.
//!
//! Driver boundary traits and values live in `hypercolor-driver-api`. This
//! crate owns concrete host services that implement or support that boundary.

mod configured;
pub mod control_apply;
pub mod control_surface;
mod credentials;
mod deadline;
pub mod discovery;
pub mod framing;
pub mod http;
mod mdns;
pub mod network;
pub mod pairing;
mod session;

pub use configured::{ConfiguredDevice, ConfiguredDevices, ResolveDevices};
pub use credentials::CredentialStore;
pub use mdns::{MdnsBrowser, MdnsScope, MdnsService};
pub use session::{
    Admitted, DEFAULT_CLEANUP, DEFAULT_OPEN, DEFAULT_RELEASE, DEFAULT_SETTLE, DeviceSession,
    FrameDelivery, Session, SessionSink, SessionTable,
};
