//! Driver module inventory API functions.

use serde::Deserialize;

use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::{
    DriverModuleDescriptor, DriverPresentation, DriverProtocolDescriptor,
};
use hypercolor_ui::control_surface_api::path_segment;

use super::client;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DriverListResponse {
    pub items: Vec<DriverSummary>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DriverSummary {
    pub descriptor: DriverModuleDescriptor,
    pub presentation: DriverPresentation,
    pub enabled: bool,
    pub config_key: String,
    #[serde(default)]
    pub protocols: Vec<DriverProtocolDescriptor>,
    #[serde(default)]
    pub control_surface_id: Option<String>,
    #[serde(default)]
    pub control_surface_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DriverConfigResponse {
    pub driver_id: String,
    pub config_key: String,
    pub configurable: bool,
    pub current: DriverConfigEntry,
    #[serde(default)]
    pub default: Option<DriverConfigEntry>,
}

pub fn driver_config_url(driver_id: &str) -> String {
    format!("/api/v1/drivers/{}/config", path_segment(driver_id))
}

pub async fn fetch_drivers() -> Result<Vec<DriverSummary>, String> {
    client::fetch_json::<DriverListResponse>("/api/v1/drivers")
        .await
        .map(|response| response.items)
        .map_err(Into::into)
}

#[allow(dead_code)]
pub async fn fetch_driver_config(driver_id: &str) -> Result<DriverConfigResponse, String> {
    client::fetch_json(&driver_config_url(driver_id))
        .await
        .map_err(Into::into)
}
