use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ChannelName {
    #[serde(rename = "control")]
    Control,
    #[serde(rename = "sync.notifications")]
    SyncNotifications,
    #[serde(rename = "relay.http")]
    RelayHttp,
    #[serde(rename = "relay.ws")]
    RelayWs,
    #[serde(rename = "entitlement.refresh")]
    EntitlementRefresh,
    #[serde(rename = "studio.preview")]
    StudioPreview,
    #[serde(rename = "telemetry")]
    Telemetry,
}

impl ChannelName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::SyncNotifications => "sync.notifications",
            Self::RelayHttp => "relay.http",
            Self::RelayWs => "relay.ws",
            Self::EntitlementRefresh => "entitlement.refresh",
            Self::StudioPreview => "studio.preview",
            Self::Telemetry => "telemetry",
        }
    }

    #[must_use]
    pub const fn required_feature(self) -> Option<&'static str> {
        match self {
            Self::RelayHttp | Self::RelayWs => Some("hc.remote"),
            Self::StudioPreview => Some("hc.ai_effects_generate"),
            Self::Control
            | Self::SyncNotifications
            | Self::EntitlementRefresh
            | Self::Telemetry => None,
        }
    }
}

impl fmt::Display for ChannelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ChannelName {
    type Err = ChannelParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "control" => Ok(Self::Control),
            "sync.notifications" => Ok(Self::SyncNotifications),
            "relay.http" => Ok(Self::RelayHttp),
            "relay.ws" => Ok(Self::RelayWs),
            "entitlement.refresh" => Ok(Self::EntitlementRefresh),
            "studio.preview" => Ok(Self::StudioPreview),
            "telemetry" => Ok(Self::Telemetry),
            _ => Err(ChannelParseError {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown daemon-cloud channel: {value}")]
pub struct ChannelParseError {
    value: String,
}
