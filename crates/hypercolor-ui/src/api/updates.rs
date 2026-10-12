//! Optional GitHub release notifications supplied by the OSS daemon.

pub use hypercolor_types::api::updates::{ReleaseUpdate, UpdatePreferencesPatch, UpdateStatus};

use super::{ApiResult, client};

const ENDPOINT: &str = "/api/v1/release-notifications";

pub async fn fetch_release_notifications() -> ApiResult<UpdateStatus> {
    client::fetch_json(ENDPOINT).await
}

pub async fn check_release_notifications() -> ApiResult<UpdateStatus> {
    client::post_json(&format!("{ENDPOINT}/check"), &()).await
}

pub async fn patch_release_notifications(
    preferences: &UpdatePreferencesPatch,
) -> ApiResult<UpdateStatus> {
    client::patch_json(ENDPOINT, preferences).await
}
