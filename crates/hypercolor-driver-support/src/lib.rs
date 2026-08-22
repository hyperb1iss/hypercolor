//! Native host utilities shared by Hypercolor driver implementations.
//!
//! Driver boundary traits and values live in `hypercolor-driver-api`. This
//! crate owns concrete host services that implement or support that boundary.

mod credentials;
mod mdns;

pub use credentials::CredentialStore;
pub use mdns::{MdnsBrowser, MdnsService};
