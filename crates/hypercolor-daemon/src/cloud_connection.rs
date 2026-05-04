use hypercolor_cloud_client::daemon_link::{DeniedChannel, WelcomeFrame, frame::DenialReason};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloudConnectionRuntimeState {
    #[default]
    Idle,
    Connecting,
    Connected,
    Backoff,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, ToSchema)]
pub struct CloudConnectionSnapshot {
    pub runtime_state: CloudConnectionRuntimeState,
    pub connected: bool,
    pub session_id: Option<String>,
    pub available_channels: Vec<String>,
    pub denied_channels: Vec<CloudDeniedChannelStatus>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CloudDeniedChannelStatus {
    pub name: String,
    pub reason: String,
    pub feature: Option<String>,
}

#[derive(Debug, Default)]
pub struct CloudConnectionRuntime {
    snapshot: CloudConnectionSnapshot,
}

impl CloudConnectionRuntime {
    pub fn snapshot(&self) -> CloudConnectionSnapshot {
        self.snapshot.clone()
    }

    pub fn mark_idle(&mut self) {
        self.snapshot = CloudConnectionSnapshot::default();
    }

    pub fn mark_connecting(&mut self) {
        self.snapshot.runtime_state = CloudConnectionRuntimeState::Connecting;
        self.snapshot.connected = false;
        self.snapshot.session_id = None;
        self.snapshot.available_channels.clear();
        self.snapshot.denied_channels.clear();
    }

    pub fn mark_connected(&mut self, welcome: &WelcomeFrame) {
        self.snapshot = CloudConnectionSnapshot {
            runtime_state: CloudConnectionRuntimeState::Connected,
            connected: true,
            session_id: Some(welcome.session_id.to_string()),
            available_channels: welcome
                .available_channels
                .iter()
                .map(|channel| channel.as_str().to_owned())
                .collect(),
            denied_channels: welcome
                .denied_channels
                .iter()
                .map(CloudDeniedChannelStatus::from_denied_channel)
                .collect(),
            last_error: None,
        };
    }

    pub fn mark_backoff(&mut self, error: impl Into<String>) {
        self.snapshot.runtime_state = CloudConnectionRuntimeState::Backoff;
        self.snapshot.connected = false;
        self.snapshot.session_id = None;
        self.snapshot.available_channels.clear();
        self.snapshot.denied_channels.clear();
        self.snapshot.last_error = Some(error.into());
    }
}

impl CloudDeniedChannelStatus {
    fn from_denied_channel(denied: &DeniedChannel) -> Self {
        Self {
            name: denied.name.as_str().to_owned(),
            reason: denial_reason_code(denied.reason).to_owned(),
            feature: denied.feature.clone(),
        }
    }
}

const fn denial_reason_code(reason: DenialReason) -> &'static str {
    match reason {
        DenialReason::EntitlementMissing => "entitlement_missing",
        DenialReason::CapabilityMissing => "capability_missing",
        DenialReason::Disabled => "disabled",
    }
}
