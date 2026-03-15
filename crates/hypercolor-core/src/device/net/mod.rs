//! Shared network-device infrastructure.
//!
//! This module holds reusable helpers shared by network-native backends such
//! as WLED, Hue, and Nanoleaf.

mod credentials;
mod mdns;

pub use credentials::{CredentialStore, Credentials};
pub use mdns::{MdnsBrowser, MdnsService};
