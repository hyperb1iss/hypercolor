//! Live input configuration transactions shared by transports and restores.

mod audio;
mod capture;

pub(crate) use audio::reconfigure_input_manager;
pub(crate) use capture::{
    CaptureConfigTransactionError, apply_capture_config_transaction, capture_runtime_matches,
};
#[cfg(test)]
pub(crate) use capture::{capture_statuses_match, validate_prepared_capture_status};
