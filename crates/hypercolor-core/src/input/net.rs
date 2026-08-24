//! Network throughput sampling for display faces.
//!
//! Refreshes interface counters at 1 Hz inline during sampling (a cheap
//! `/proc/net/dev` read via sysinfo) and reports the busiest non-loopback
//! interface as [`NetStats`]. Emits a snapshot only when one was refreshed;
//! the render loop carries the latest snapshot forward between refreshes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use hypercolor_types::net::NetStats;
use sysinfo::Networks;

use super::traits::{
    DataSource, DataSourceKind, DataSourceRole, InputData, InputSource, SourceRoleBinding,
};
use super::{SourceIssue, SourceKind, SourceStatusHandle, SourceStatusReporter};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// Input source publishing 1 Hz network throughput snapshots.
pub struct NetSource {
    name: String,
    networks: Option<Networks>,
    last_refresh: Option<Instant>,
    last_iface: Option<String>,
    status: SourceStatusReporter,
}

impl NetSource {
    #[must_use]
    pub fn new() -> Self {
        Self {
            name: "net".to_owned(),
            networks: None,
            last_refresh: None,
            last_iface: None,
            status: SourceStatusReporter::new(
                "network",
                SourceKind::Network,
                "sysinfo",
                true,
                true,
                true,
            ),
        }
    }

    fn refresh_stats(&mut self, elapsed: Duration) -> Option<NetStats> {
        let networks = self.networks.as_mut()?;
        networks.refresh(true);

        let elapsed_secs = elapsed.as_secs_f64();
        if elapsed_secs <= f64::EPSILON {
            return None;
        }

        let busiest = networks
            .iter()
            .filter(|(name, _)| !is_loopback(name))
            .max_by_key(|(name, data)| {
                // Sticky tiebreak: keep reporting the previous interface
                // through idle periods instead of flapping alphabetically.
                let sticky = u64::from(self.last_iface.as_deref() == Some(name.as_str()));
                (data.received() + data.transmitted(), sticky)
            });
        let (iface, data) = busiest?;

        self.last_iface = Some(iface.clone());
        Some(NetStats {
            rx_bps: bytes_per_second(data.received(), elapsed_secs),
            tx_bps: bytes_per_second(data.transmitted(), elapsed_secs),
            iface: iface.clone(),
        })
    }
}

impl Default for NetSource {
    fn default() -> Self {
        Self::new()
    }
}

impl InputSource for NetSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> Result<()> {
        if self.networks.is_none() {
            self.status.begin_session()?;
            self.networks = Some(Networks::new_with_refreshed_list());
            self.last_refresh = Some(Instant::now());
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.networks = None;
        self.last_refresh = None;
        self.last_iface = None;
        self.status.stop();
    }

    fn sample(&mut self) -> Result<InputData> {
        let Some(last_refresh) = self.last_refresh else {
            return Ok(InputData::None);
        };
        let elapsed = last_refresh.elapsed();
        if elapsed < REFRESH_INTERVAL {
            return Ok(InputData::None);
        }

        self.last_refresh = Some(Instant::now());
        if let Some(stats) = self.refresh_stats(elapsed) {
            if let Some(status) = self.status.session() {
                let sampled_at = Instant::now();
                status.record_sample(
                    sampled_at,
                    sampled_at + REFRESH_INTERVAL + REFRESH_INTERVAL,
                    1,
                )?;
            }
            Ok(InputData::Net(Arc::new(stats)))
        } else {
            if let Some(status) = self.status.session() {
                status.unavailable(
                    SourceIssue::new(
                        "network_interface_unavailable",
                        "no eligible non-loopback network interface is available",
                        true,
                    )
                    .with_remediation("connect or enable a network interface"),
                );
            }
            Ok(InputData::None)
        }
    }

    fn is_running(&self) -> bool {
        self.networks.is_some()
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }
}

impl SourceRoleBinding for NetSource {
    type Role = DataSourceRole;
}

impl DataSource for NetSource {
    fn data_source_kind(&self) -> DataSourceKind {
        DataSourceKind::Network
    }
}

fn is_loopback(name: &str) -> bool {
    name == "lo" || name.starts_with("lo:")
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "byte rates are non-negative and far below u64::MAX"
)]
fn bytes_per_second(bytes: u64, elapsed_secs: f64) -> u64 {
    ((bytes as f64) / elapsed_secs).round() as u64
}
