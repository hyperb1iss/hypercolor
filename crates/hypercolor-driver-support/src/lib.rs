//! Native host utilities shared by Hypercolor driver implementations.
//!
//! Driver boundary traits and values live in `hypercolor-driver-api`. This
//! crate owns concrete host services that implement or support that boundary.

pub mod control_apply;
pub mod control_surface;
mod credentials;
mod mdns;
pub mod network;
pub mod pairing;

pub use credentials::CredentialStore;
pub use mdns::{MdnsBrowser, MdnsService};
