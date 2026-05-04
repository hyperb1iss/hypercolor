#![forbid(unsafe_code)]

pub mod auth;
pub mod config;
pub mod devices;
pub mod error;

pub use auth::DeviceTokenPoll;
pub use config::{CloudClient, CloudClientConfig};
pub use devices::{DEVICE_REGISTRATION_PATH, DeviceRegistrationInput, signed_device_registration};
pub use error::CloudClientError;

pub use hypercolor_cloud_api as api;
pub use hypercolor_daemon_link as daemon_link;
