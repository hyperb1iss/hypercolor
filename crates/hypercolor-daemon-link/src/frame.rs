use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ulid::Ulid;

use crate::channel::ChannelName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    Hello,
    Welcome,
    WelcomeUpdate,
    Msg,
    Open,
    Close,
    Ack,
    Error,
    Ping,
    Pong,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame<P = Value> {
    pub channel: ChannelName,
    pub kind: FrameKind,
    pub msg_id: Ulid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<Ulid>,
    pub payload: P,
    #[serde(default)]
    pub compressed: bool,
}

impl<P> Frame<P> {
    #[must_use]
    pub const fn new(channel: ChannelName, kind: FrameKind, msg_id: Ulid, payload: P) -> Self {
        Self {
            channel,
            kind,
            msg_id,
            in_reply_to: None,
            payload,
            compressed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloFrame {
    pub protocol_version: u16,
    pub daemon_capabilities: DaemonCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entitlement_jwt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_resume: Option<TunnelResume>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonCapabilities {
    pub sync: bool,
    pub relay: bool,
    pub entitlement_refresh: bool,
    pub telemetry: bool,
    #[serde(default)]
    pub studio_preview: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelResume {
    pub session_id: Ulid,
    pub last_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WelcomeFrame {
    pub session_id: Ulid,
    pub available_channels: Vec<ChannelName>,
    pub denied_channels: Vec<DeniedChannel>,
    pub server_capabilities: ServerCapabilities,
    pub heartbeat_interval_s: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeniedChannel {
    pub name: ChannelName,
    pub reason: DenialReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialReason {
    EntitlementMissing,
    CapabilityMissing,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub tunnel_resume: bool,
    pub compression: Vec<String>,
    #[serde(default)]
    pub max_frame_bytes: Option<u64>,
}

impl WelcomeFrame {
    #[must_use]
    pub fn denied_by_channel(&self) -> BTreeMap<ChannelName, DeniedChannel> {
        self.denied_channels
            .iter()
            .cloned()
            .map(|denied| (denied.name, denied))
            .collect()
    }
}
