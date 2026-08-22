//! Driver module inventory API functions.

use crate::control_surface_api::path_segment;
pub use hypercolor_types::api::drivers::{DriverConfigResponse, DriverListResponse, DriverSummary};

use super::client;

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
