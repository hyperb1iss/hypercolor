//! Govee network driver.

pub mod capabilities;
pub mod lan;

pub use capabilities::{
    GoveeCapabilities, SkuFamily, SkuProfile, fallback_profile, profile_for_sku,
};
