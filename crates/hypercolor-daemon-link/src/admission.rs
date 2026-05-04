use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use crate::channel::ChannelName;
use crate::frame::{DeniedChannel, WelcomeFrame};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionSet {
    admitted: BTreeSet<ChannelName>,
    denied: BTreeMap<ChannelName, DeniedChannel>,
}

impl AdmissionSet {
    #[must_use]
    pub fn from_welcome(welcome: &WelcomeFrame) -> Self {
        Self {
            admitted: welcome.available_channels.iter().copied().collect(),
            denied: welcome.denied_by_channel(),
        }
    }

    pub fn check(&self, channel: ChannelName) -> Result<(), AdmissionError> {
        if self.admitted.contains(&channel) {
            return Ok(());
        }

        if let Some(denied) = self.denied.get(&channel) {
            return Err(AdmissionError::ChannelDenied {
                channel,
                feature: denied.feature.clone(),
            });
        }

        Err(AdmissionError::UnknownChannel {
            channel: channel.to_string(),
        })
    }

    pub fn check_name(&self, channel: &str) -> Result<(), AdmissionError> {
        let channel =
            ChannelName::from_str(channel).map_err(|_| AdmissionError::UnknownChannel {
                channel: channel.to_owned(),
            })?;
        self.check(channel)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    #[error("channel denied: {channel}")]
    ChannelDenied {
        channel: ChannelName,
        feature: Option<String>,
    },
    #[error("unknown channel: {channel}")]
    UnknownChannel { channel: String },
}
