use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::updates::ReleaseChannel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeatureKey {
    #[serde(rename = "hc.cloud_sync")]
    CloudSync,
    #[serde(rename = "hc.remote")]
    Remote,
    #[serde(rename = "hc.signed_builds")]
    SignedBuilds,
    #[serde(rename = "hc.marketplace_publish")]
    MarketplacePublish,
    #[serde(rename = "hc.marketplace_paid")]
    MarketplacePaid,
    #[serde(rename = "hc.ai_effects_generate")]
    AiEffectsGenerate,
}

impl FeatureKey {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CloudSync => "hc.cloud_sync",
            Self::Remote => "hc.remote",
            Self::SignedBuilds => "hc.signed_builds",
            Self::MarketplacePublish => "hc.marketplace_publish",
            Self::MarketplacePaid => "hc.marketplace_paid",
            Self::AiEffectsGenerate => "hc.ai_effects_generate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntitlementClaims {
    pub iss: String,
    pub sub: String,
    pub aud: Vec<String>,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
    pub kid: String,
    pub token_version: u32,
    pub device_install_id: Uuid,
    pub tier: String,
    pub features: Vec<FeatureKey>,
    pub channels: Vec<ReleaseChannel>,
    pub rate_limits: RateLimits,
    pub update_until: i64,
}

impl EntitlementClaims {
    #[must_use]
    pub fn has_feature(&self, feature: FeatureKey) -> bool {
        self.features.contains(&feature)
    }

    #[must_use]
    pub fn allows_channel(&self, channel: ReleaseChannel) -> bool {
        self.channels.contains(&channel)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimits {
    pub remote_bandwidth_gb_month: u32,
    pub remote_concurrent_tunnels: u32,
    pub studio_sessions_month: u32,
    pub studio_max_session_seconds: u32,
    pub studio_max_session_tokens: u32,
    pub studio_default_model: String,
}
