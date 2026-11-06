//! The installer's record of one installation, `installation.json`.

use serde::Deserialize;
use uuid::Uuid;

/// The record's file in the state root.
pub const WINDOWS_INSTALLATION_FILE: &str = "installation.json";
const INSTALLATION_SCHEMA_VERSION: u32 = 1;

/// Who runs Hypercolor's runtime on this installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsRuntimeMode {
    /// The app runs in the recorded user's session and supervises the
    /// daemon as a child.
    AppSupervised,
}

/// What the installer recorded.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsInstallation {
    schema_version: u32,
    /// The identity every update plan binds to.
    pub installation_id: Uuid,
    user_sid: String,
    /// Who runs the runtime.
    pub runtime_mode: WindowsRuntimeMode,
}

impl WindowsInstallation {
    /// Decode the record.
    ///
    /// # Errors
    ///
    /// Returns the reason the record is not one this build reads. The SID
    /// is written into security descriptors, so anything but the textual
    /// form of a SID is refused here.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let record: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if record.schema_version != INSTALLATION_SCHEMA_VERSION {
            return Err(format!(
                "unsupported installation schema {}",
                record.schema_version
            ));
        }
        if !is_sid_text(&record.user_sid) {
            return Err(format!("{:?} is not a SID", record.user_sid));
        }
        Ok(record)
    }

    /// The recorded user, in the textual form of a SID.
    #[must_use]
    pub fn user_sid(&self) -> &str {
        &self.user_sid
    }
}

fn is_sid_text(text: &str) -> bool {
    text.strip_prefix("S-").is_some_and(|rest| {
        let mut parts = rest.split('-');
        let revision = parts.next();
        let values = parts.collect::<Vec<_>>();
        revision == Some("1")
            && values.len() >= 2
            && values
                .iter()
                .all(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
    })
}

#[cfg(test)]
mod tests {
    use super::{WindowsInstallation, WindowsRuntimeMode};

    const SID: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";

    fn record(user_sid: &str) -> Vec<u8> {
        serde_json::json!({
            "schema_version": 1,
            "installation_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
            "user_sid": user_sid,
            "runtime_mode": "app_supervised",
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_record_names_the_user_and_the_runtime_mode() {
        let installation = WindowsInstallation::decode(&record(SID)).expect("record");

        assert_eq!(installation.user_sid(), SID);
        assert_eq!(installation.runtime_mode, WindowsRuntimeMode::AppSupervised);
    }

    #[test]
    fn nothing_but_a_sid_reaches_a_security_descriptor() {
        for user_sid in [
            "",
            "BU",
            "S-1",
            "S-1-5",
            "S-2-5-32",
            "S-1-5-21-1)(A;;GA;;;WD",
            "S-1-5--1",
        ] {
            WindowsInstallation::decode(&record(user_sid)).expect_err(user_sid);
        }
    }

    #[test]
    fn a_record_of_another_schema_or_mode_is_refused() {
        let other_schema = serde_json::json!({
            "schema_version": 2,
            "installation_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
            "user_sid": SID,
            "runtime_mode": "app_supervised",
        });
        let other_mode = serde_json::json!({
            "schema_version": 1,
            "installation_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
            "user_sid": SID,
            "runtime_mode": "service",
        });
        for value in [other_schema, other_mode] {
            WindowsInstallation::decode(value.to_string().as_bytes()).expect_err("refused");
        }
    }
}
