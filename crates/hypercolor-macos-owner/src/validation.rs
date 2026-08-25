use std::path::PathBuf;

#[cfg(target_os = "macos")]
use std::fs::File;
#[cfg(target_os = "macos")]
use std::io::Read as _;

use serde::Deserialize;

use crate::error::MacosOwnerStoreError;
use crate::model::{
    MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION, MACOS_OWNER_RECORD_SCHEMA_VERSION,
    MACOS_PROTECTED_CONTROL_CREDENTIAL_BYTES, MACOS_PROTECTED_CONTROL_CREDENTIAL_PREFIX,
    MACOS_SERVER_SESSION_ID_BYTES, MACOS_SERVER_SESSION_ID_PREFIX,
    MAX_MACOS_AUDIT_TOKEN_IDENTITY_BYTES, MAX_MACOS_DESIGNATED_REQUIREMENT_HASH_BYTES,
    MAX_MACOS_EXECUTABLE_PATH_BYTES, MacosDaemonSessionAttestation, MacosOwnerIdentity,
    MacosOwnerRecord, MacosProtectedControlCredential, MacosServerSessionId,
};

impl MacosOwnerIdentity {
    /// Validate and construct a diagnostic process identity.
    pub fn new(
        audit_token_identity: impl Into<String>,
        executable_path: impl Into<PathBuf>,
        designated_requirement_hash: impl Into<String>,
        pid: u32,
    ) -> Result<Self, MacosOwnerStoreError> {
        let identity = Self {
            audit_token_identity: audit_token_identity.into(),
            executable_path: executable_path.into(),
            designated_requirement_hash: designated_requirement_hash.into(),
            pid,
        };
        validate_owner_identity(&identity)?;
        Ok(identity)
    }
}

impl<'de> Deserialize<'de> for MacosOwnerIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawIdentity {
            audit_token_identity: String,
            executable_path: PathBuf,
            designated_requirement_hash: String,
            pid: u32,
        }

        let raw = RawIdentity::deserialize(deserializer)?;
        Self::new(
            raw.audit_token_identity,
            raw.executable_path,
            raw.designated_requirement_hash,
            raw.pid,
        )
        .map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for MacosServerSessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        validate_hex_token(
            &value,
            MACOS_SERVER_SESSION_ID_PREFIX,
            MACOS_SERVER_SESSION_ID_BYTES,
            "server_session_id must be a canonical 128-bit token",
        )
        .map_err(serde::de::Error::custom)?;
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for MacosProtectedControlCredential {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        validate_hex_token(
            &value,
            MACOS_PROTECTED_CONTROL_CREDENTIAL_PREFIX,
            MACOS_PROTECTED_CONTROL_CREDENTIAL_BYTES,
            "protected_control_credential must be a canonical 256-bit token",
        )
        .map_err(serde::de::Error::custom)?;
        Ok(Self(value))
    }
}

impl MacosDaemonSessionAttestation {
    #[cfg(target_os = "macos")]
    pub(crate) fn generate(record: &MacosOwnerRecord) -> Result<Self, MacosOwnerStoreError> {
        let mut entropy =
            [0_u8; MACOS_SERVER_SESSION_ID_BYTES + MACOS_PROTECTED_CONTROL_CREDENTIAL_BYTES];
        File::open("/dev/urandom")
            .and_then(|mut source| source.read_exact(&mut entropy))
            .map_err(|source| MacosOwnerStoreError::Read {
                artifact: "daemon session entropy",
                path: PathBuf::from("/dev/urandom"),
                source,
            })?;
        let (session_bytes, credential_bytes) = entropy.split_at(MACOS_SERVER_SESSION_ID_BYTES);
        let session_bytes = session_bytes
            .try_into()
            .expect("session entropy slice has the exact array length");
        let credential_bytes = credential_bytes
            .try_into()
            .expect("credential entropy slice has the exact array length");
        Ok(Self {
            schema_version: MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION,
            owner: record.active_owner,
            owner_epoch: record.owner_epoch,
            owner_identity: record.active_identity.clone(),
            server_session_id: MacosServerSessionId::from_bytes(session_bytes),
            protected_control_credential: MacosProtectedControlCredential::from_bytes(
                credential_bytes,
            ),
        })
    }
}

pub(crate) fn validate_owner_record(record: &MacosOwnerRecord) -> Result<(), MacosOwnerStoreError> {
    validate_version(
        "owner record",
        record.schema_version,
        MACOS_OWNER_RECORD_SCHEMA_VERSION,
    )?;
    if record.owner_epoch == 0 {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "owner record",
            detail: "owner_epoch must be positive",
        });
    }
    validate_owner_identity(&record.active_identity)?;
    if let Some(conflict) = &record.conflict
        && (conflict.active_owner != record.active_owner
            || conflict.active_epoch != record.owner_epoch)
    {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "owner record",
            detail: "conflict must identify the active owner epoch",
        });
    }
    if let Some(conflict) = &record.conflict {
        validate_owner_identity(&conflict.contender_identity)?;
    }
    Ok(())
}

pub(crate) fn validate_daemon_session_attestation(
    attestation: &MacosDaemonSessionAttestation,
) -> Result<(), MacosOwnerStoreError> {
    validate_version(
        "daemon session attestation",
        attestation.schema_version,
        MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION,
    )?;
    if attestation.owner_epoch == 0 {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "daemon session attestation",
            detail: "owner_epoch must be positive",
        });
    }
    validate_owner_identity(&attestation.owner_identity)?;
    validate_hex_token(
        attestation.server_session_id.as_str(),
        MACOS_SERVER_SESSION_ID_PREFIX,
        MACOS_SERVER_SESSION_ID_BYTES,
        "server_session_id must be a canonical 128-bit token",
    )?;
    validate_hex_token(
        attestation.protected_control_credential.expose_secret(),
        MACOS_PROTECTED_CONTROL_CREDENTIAL_PREFIX,
        MACOS_PROTECTED_CONTROL_CREDENTIAL_BYTES,
        "protected_control_credential must be a canonical 256-bit token",
    )
}

fn validate_hex_token(
    value: &str,
    prefix: &str,
    entropy_bytes: usize,
    detail: &'static str,
) -> Result<(), MacosOwnerStoreError> {
    let hex = value
        .strip_prefix(prefix)
        .filter(|hex| hex.len() == entropy_bytes * 2)
        .filter(|hex| {
            hex.bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        });
    if hex.is_none() {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "daemon session attestation",
            detail,
        });
    }
    Ok(())
}

fn validate_owner_identity(identity: &MacosOwnerIdentity) -> Result<(), MacosOwnerStoreError> {
    validate_bounded_identity_text(
        "audit_token_identity",
        &identity.audit_token_identity,
        MAX_MACOS_AUDIT_TOKEN_IDENTITY_BYTES,
    )?;
    let executable_path =
        identity
            .executable_path
            .to_str()
            .ok_or(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "executable_path",
                detail: "must be valid UTF-8",
            })?;
    validate_bounded_identity_text(
        "executable_path",
        executable_path,
        MAX_MACOS_EXECUTABLE_PATH_BYTES,
    )?;
    if !is_macos_absolute_path(&identity.executable_path) {
        return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field: "executable_path",
            detail: "must be absolute",
        });
    }
    validate_bounded_identity_text(
        "designated_requirement_hash",
        &identity.designated_requirement_hash,
        MAX_MACOS_DESIGNATED_REQUIREMENT_HASH_BYTES,
    )?;
    if identity.pid == 0 {
        return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field: "pid",
            detail: "must be positive",
        });
    }
    Ok(())
}

/// Whether `path` is absolute under the macOS rule.
///
/// The vocabulary describes macOS paths, so absoluteness means a leading
/// slash on every build; the host rule would reject valid records on the
/// stub targets that also validate them.
pub(crate) fn is_macos_absolute_path(path: &std::path::Path) -> bool {
    path.to_str().is_some_and(|text| text.starts_with('/'))
}

pub(crate) fn validate_bounded_identity_text(
    field: &'static str,
    value: &str,
    maximum_bytes: usize,
) -> Result<(), MacosOwnerStoreError> {
    if value.is_empty() {
        Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field,
            detail: "must not be empty",
        })
    } else if value.len() > maximum_bytes {
        Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field,
            detail: "exceeds its byte limit",
        })
    } else {
        Ok(())
    }
}

pub(crate) fn validate_version(
    artifact: &'static str,
    found: u32,
    expected: u32,
) -> Result<(), MacosOwnerStoreError> {
    if found == expected {
        Ok(())
    } else {
        Err(MacosOwnerStoreError::UnsupportedVersion {
            artifact,
            found,
            expected,
        })
    }
}
