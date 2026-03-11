//! HTTP client for daemon communication.
//!
//! Builds and sends requests to the Hypercolor daemon's REST API.
//! When the daemon is not running, all requests return a descriptive error
//! rather than panicking.

use anyhow::{Context, Result};
use serde::Serialize;

/// HTTP client for the Hypercolor daemon REST API.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    /// Base URL for the daemon (e.g., `http://localhost:9420`).
    base_url: String,
    /// Optional API key sent as a bearer token.
    api_key: Option<String>,
    /// Inner `reqwest` async client.
    http: reqwest::Client,
}

impl DaemonClient {
    /// Create a new client targeting the given host and port.
    #[must_use]
    pub fn new(host: &str, port: u16, api_key: Option<&str>) -> Self {
        let base_url = format!("http://{host}:{port}");
        let http = reqwest::Client::new();
        Self {
            base_url,
            api_key: api_key.map(ToOwned::to_owned),
            http,
        }
    }

    /// Send a GET request to the daemon and parse the JSON response.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable or returns a non-success
    /// status code.
    pub async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .with_auth(self.http.get(&url))
            .send()
            .await
            .with_context(|| {
                format!("Failed to connect to daemon at {url}. Is the daemon running?")
            })?;
        parse_api_response(response).await
    }

    /// Send a POST request with a JSON body and parse the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable, the body cannot be
    /// serialized, or the daemon returns a non-success status code.
    pub async fn post(&self, path: &str, body: &impl Serialize) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .with_auth(self.http.post(&url))
            .json(body)
            .send()
            .await
            .with_context(|| {
                format!("Failed to connect to daemon at {url}. Is the daemon running?")
            })?;
        parse_api_response(response).await
    }

    /// Send a PUT request with a JSON body and parse the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable, the body cannot be
    /// serialized, or the daemon returns a non-success status code.
    pub async fn put(&self, path: &str, body: &impl Serialize) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .with_auth(self.http.put(&url))
            .json(body)
            .send()
            .await
            .with_context(|| {
                format!("Failed to connect to daemon at {url}. Is the daemon running?")
            })?;
        parse_api_response(response).await
    }

    /// Send a DELETE request and parse the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable or returns a non-success
    /// status code.
    pub async fn delete(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .with_auth(self.http.delete(&url))
            .send()
            .await
            .with_context(|| {
                format!("Failed to connect to daemon at {url}. Is the daemon running?")
            })?;
        parse_api_response(response).await
    }

    fn with_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(api_key) = &self.api_key {
            request.bearer_auth(api_key)
        } else {
            request
        }
    }
}

async fn parse_api_response(response: reqwest::Response) -> Result<serde_json::Value> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Daemon returned {status}: {body}");
    }

    let json: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse daemon response as JSON")?;

    Ok(json.get("data").cloned().unwrap_or(json))
}
