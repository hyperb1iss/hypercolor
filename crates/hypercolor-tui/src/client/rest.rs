//! REST client for the Hypercolor daemon HTTP API.

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

use crate::state::{DaemonState, DeviceSummary, EffectSummary};

/// HTTP client for the daemon REST API.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    base_url: String,
    http: reqwest::Client,
}

impl DaemonClient {
    /// Create a client targeting the given host and port.
    #[must_use]
    pub fn new(host: &str, port: u16) -> Self {
        let base_url = format!("http://{host}:{port}");
        Self {
            base_url,
            http: reqwest::Client::new(),
        }
    }

    /// Fetch the daemon's current state.
    pub async fn get_status(&self) -> Result<DaemonState> {
        self.get_data("/status").await
    }

    /// Fetch all available effects.
    pub async fn get_effects(&self) -> Result<Vec<EffectSummary>> {
        self.get_data("/effects").await
    }

    /// Fetch all connected devices.
    pub async fn get_devices(&self) -> Result<Vec<DeviceSummary>> {
        self.get_data("/devices").await
    }

    /// Fetch the favorites list (effect IDs).
    pub async fn get_favorites(&self) -> Result<Vec<String>> {
        self.get_data("/library/favorites").await
    }

    /// Apply an effect by ID.
    pub async fn apply_effect(
        &self,
        effect_id: &str,
        controls: Option<&serde_json::Value>,
    ) -> Result<()> {
        let url = format!("{}/api/v1/effects/{effect_id}/apply", self.base_url);
        let mut req = self.http.post(&url);
        if let Some(body) = controls {
            req = req.json(body);
        } else {
            req = req.json(&serde_json::json!({}));
        }
        let response = req.send().await.with_context(|| {
            format!("Failed to apply effect {effect_id}. Is the daemon running?")
        })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Apply effect failed ({status}): {body}");
        }
        Ok(())
    }

    /// Toggle favorite for an effect.
    pub async fn toggle_favorite(&self, effect_id: &str, is_favorite: bool) -> Result<()> {
        if is_favorite {
            let url = format!("{}/api/v1/library/favorites/{effect_id}", self.base_url);
            self.http.delete(&url).send().await?;
        } else {
            let url = format!("{}/api/v1/library/favorites", self.base_url);
            self.http
                .post(&url)
                .json(&serde_json::json!({ "effect_id": effect_id }))
                .send()
                .await?;
        }
        Ok(())
    }

    /// Update a control value on the active effect.
    pub async fn update_control(&self, control_id: &str, value: &serde_json::Value) -> Result<()> {
        let url = format!("{}/api/v1/effects/current/controls", self.base_url);
        self.http
            .patch(&url)
            .json(&serde_json::json!({ control_id: value }))
            .send()
            .await
            .with_context(|| "Failed to update control")?;
        Ok(())
    }

    // ── Internal helpers ────────────────────────────────────

    async fn get_data<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to connect to daemon at {url}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("API request failed ({status}): {body}");
        }

        // The API wraps responses in { "data": T, "meta": {...} }
        let envelope: serde_json::Value = response.json().await?;
        if let Some(data) = envelope.get("data") {
            Ok(serde_json::from_value(data.clone())?)
        } else {
            // Some endpoints return the data directly
            Ok(serde_json::from_value(envelope)?)
        }
    }
}
