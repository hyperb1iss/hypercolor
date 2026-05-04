#[derive(Debug, thiserror::Error)]
pub enum CloudClientError {
    #[error("invalid cloud base url: {0}")]
    InvalidBaseUrl(String),

    #[error("cloud request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("cloud credential store failed: {0}")]
    CredentialStore(#[from] keyring_core::Error),

    #[error("invalid daemon identity material: {0}")]
    IdentityEncoding(#[from] hypercolor_daemon_link::IdentityEncodingError),

    #[error("invalid persisted daemon id: {0}")]
    InvalidDaemonId(String),

    #[error("persisted daemon cloud identity is incomplete")]
    IncompleteIdentity,
}
