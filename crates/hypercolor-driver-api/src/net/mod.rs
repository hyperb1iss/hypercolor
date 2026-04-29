//! Native network-device utilities shared by built-in network drivers.

mod credentials;
mod mdns;

pub use credentials::CredentialStore;
pub use mdns::{MdnsBrowser, MdnsService};
