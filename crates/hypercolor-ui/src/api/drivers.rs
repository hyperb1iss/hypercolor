//! Driver module inventory API functions.

use serde::Deserialize;

use hypercolor_types::device::{DriverModuleDescriptor, DriverProtocolDescriptor};

use super::client;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DriverListResponse {
    pub items: Vec<DriverSummary>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DriverSummary {
    pub descriptor: DriverModuleDescriptor,
    pub enabled: bool,
    pub config_key: String,
    #[serde(default)]
    pub protocols: Vec<DriverProtocolDescriptor>,
    #[serde(default)]
    pub control_surface_id: Option<String>,
    #[serde(default)]
    pub control_surface_path: Option<String>,
}

pub async fn fetch_drivers() -> Result<Vec<DriverSummary>, String> {
    client::fetch_json::<DriverListResponse>("/api/v1/drivers")
        .await
        .map(|response| response.items)
        .map_err(Into::into)
}
