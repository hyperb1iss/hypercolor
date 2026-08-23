//! Stored daemon credentials shared by native clients.

use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One validated credential from `servers.toml`.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredServerCredential {
    instance_id: String,
    api_key: String,
    host: Option<IpAddr>,
    port: Option<u16>,
}

impl StoredServerCredential {
    /// Stable daemon instance identifier.
    #[must_use]
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Stored bearer credential.
    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Bound endpoint when both host and port were persisted.
    #[must_use]
    pub const fn endpoint(&self) -> Option<(IpAddr, u16)> {
        match (self.host, self.port) {
            (Some(host), Some(port)) => Some((host, port)),
            _ => None,
        }
    }
}

impl fmt::Debug for StoredServerCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredServerCredential")
            .field("instance_id", &self.instance_id)
            .field("api_key", &"[redacted]")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish()
    }
}

/// Failure to read or parse the shared native-client credential file.
#[derive(Debug, thiserror::Error)]
pub enum StoredServersError {
    /// The file could not be read.
    #[error("failed to read stored servers at {path}: {source}")]
    Read {
        /// Credential file path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The TOML document was malformed.
    #[error("failed to parse stored servers at {path}: {source}")]
    Parse {
        /// Credential file path.
        path: PathBuf,
        /// TOML decoder error.
        #[source]
        source: Box<toml::de::Error>,
    },
}

#[derive(Deserialize)]
struct StoredServersFile {
    #[serde(default)]
    servers: Vec<StoredServerConfig>,
}

#[derive(Deserialize)]
struct StoredServerConfig {
    instance_id: String,
    api_key: String,
    host: Option<IpAddr>,
    port: Option<u16>,
}

/// Load and validate native-client credentials from a `servers.toml` file.
pub fn load_server_credentials(
    path: &Path,
) -> Result<Vec<StoredServerCredential>, StoredServersError> {
    let contents = std::fs::read_to_string(path).map_err(|source| StoredServersError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse_server_credentials(path, &contents)
}

fn parse_server_credentials(
    path: &Path,
    contents: &str,
) -> Result<Vec<StoredServerCredential>, StoredServersError> {
    let file = toml::from_str::<StoredServersFile>(contents).map_err(|source| {
        StoredServersError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        }
    })?;

    Ok(file
        .servers
        .into_iter()
        .filter_map(|entry| {
            let instance_id = entry.instance_id.trim();
            let api_key = entry.api_key.trim();
            if instance_id.is_empty() || api_key.is_empty() {
                return None;
            }
            Some(StoredServerCredential {
                instance_id: instance_id.to_owned(),
                api_key: api_key.to_owned(),
                host: entry.host,
                port: entry.port,
            })
        })
        .collect())
}
