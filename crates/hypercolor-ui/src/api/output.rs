//! The output resource — `GET`/`PATCH /api/v1/output` (Spec 78 §4).
//!
//! Power and brightness share one route. The wrappers below keep the
//! UI's percentage vocabulary at the slider boundary while the wire
//! stays on the canonical `0.0..=1.0` float.

use super::{ApiResult, client};

pub use hypercolor_types::api::output::{OutputPatchRequest, OutputPowerMode, OutputResource};

const OUTPUT_PATH: &str = "/api/v1/output";

/// Read the live output resource.
pub async fn fetch_output() -> ApiResult<OutputResource> {
    client::fetch_json(OUTPUT_PATH).await
}

/// Apply a partial output patch.
pub async fn patch_output(body: &OutputPatchRequest) -> ApiResult<OutputResource> {
    client::patch_json(OUTPUT_PATH, body).await
}

/// Pause output while preserving the live scene, effects, and controls.
pub async fn pause_output() -> ApiResult<()> {
    set_output_power(OutputPowerMode::Paused).await
}

/// Resume output for the preserved live scene.
pub async fn resume_output() -> ApiResult<()> {
    set_output_power(OutputPowerMode::Running).await
}

async fn set_output_power(power: OutputPowerMode) -> ApiResult<()> {
    client::patch_json_discard(
        OUTPUT_PATH,
        &OutputPatchRequest {
            power: Some(power),
            brightness: None,
        },
    )
    .await
}

/// Update the global output brightness, in percent.
pub async fn set_global_brightness(brightness: u8) -> ApiResult<u8> {
    let patched = patch_output(&OutputPatchRequest {
        power: None,
        brightness: Some(f32::from(brightness) / 100.0),
    })
    .await?;
    Ok(brightness_percent(patched.brightness))
}

/// Fetch the current global brightness, in percent.
pub async fn fetch_global_brightness() -> ApiResult<u8> {
    Ok(brightness_percent(fetch_output().await?.brightness))
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "brightness is clamped to the unit interval before scaling"
)]
fn brightness_percent(brightness: f32) -> u8 {
    (brightness.clamp(0.0, 1.0) * 100.0).round() as u8
}
