#[derive(Debug, thiserror::Error)]
pub enum CloudClientError {
    #[error("invalid cloud base url: {0}")]
    InvalidBaseUrl(String),

    #[error("cloud request failed: {0}")]
    Request(#[from] reqwest::Error),
}
