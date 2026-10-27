//! Configuration participation in a host-owned durable transaction.

use std::sync::{Arc, MutexGuard};

use anyhow::{Context, Result};
use hypercolor_types::config::HypercolorConfig;

use super::{ConfigManager, ConfigWriterState, normalize_config};
use crate::persistence::{AtomicWriteOutcome, AtomicWriteReservation};

/// Scoped to a synchronous transaction callback on a blocking worker. The
/// ordinary config writer lock stays held until the complete callback returns.
pub struct PreparedConfigMutation<'a> {
    manager: &'a ConfigManager,
    writer: MutexGuard<'a, ConfigWriterState>,
    source: Arc<HypercolorConfig>,
    candidate: Arc<HypercolorConfig>,
    reservation: Option<AtomicWriteReservation>,
    payload: Vec<u8>,
    durable: bool,
}

impl ConfigManager {
    /// Prepare a conditional config change while retaining its normal writer
    /// authority for the host's complete multi-store persistence/publication.
    pub fn with_prepared_mutation<R>(
        &self,
        expected: &Arc<HypercolorConfig>,
        mutate: impl FnOnce(&mut HypercolorConfig),
        transaction: impl FnOnce(&mut PreparedConfigMutation<'_>) -> Result<R>,
    ) -> Result<Option<R>> {
        let writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let source = self.config.load_full();
        if !Arc::ptr_eq(expected, &source) {
            return Ok(None);
        }
        let mut candidate = (*source).clone();
        mutate(&mut candidate);
        let candidate = Arc::new(normalize_config(candidate));
        let payload = toml::to_string_pretty(candidate.as_ref())
            .context("failed to serialize prepared config")?
            .into_bytes();
        let mut prepared = PreparedConfigMutation {
            manager: self,
            writer,
            source,
            candidate,
            reservation: Some(self.persistence.reserve()),
            payload,
            durable: false,
        };
        transaction(&mut prepared).map(Some)
    }
}

impl PreparedConfigMutation<'_> {
    /// Admit only after the host's transaction journal is durable.
    pub fn persist(&mut self) -> AtomicWriteOutcome {
        let outcome = self
            .reservation
            .take()
            .expect("config candidate persisted once")
            .admit(std::mem::take(&mut self.payload))
            .commit_and_wait_durable();
        self.durable = outcome == AtomicWriteOutcome::Written;
        outcome
    }

    /// Publish only after every participating store has become durable.
    pub fn publish(&mut self) {
        assert!(self.durable, "config candidate durable before publication");
        self.manager
            .publish_config(&mut self.writer, &self.source, Arc::clone(&self.candidate));
        self.manager
            .note_persisted(&mut self.writer, &self.candidate);
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigManager;
    use crate::persistence::AtomicWriteOutcome;
    use std::sync::Arc;

    #[test]
    fn dropping_unadmitted_config_candidate_changes_neither_disk_nor_memory() {
        let directory = tempfile::tempdir().expect("config directory");
        let path = directory.path().join("config.toml");
        let manager = ConfigManager::new(path.clone()).expect("config manager");
        let original = Arc::clone(&manager.get());
        manager
            .with_prepared_mutation(
                &original,
                |config| config.daemon.canvas_width = 800,
                |_| {
                    assert!(manager.write_lock.try_lock().is_err());
                    Ok(())
                },
            )
            .expect("prepared callback")
            .expect("current source");
        assert!(manager.is_current(&original));
        assert!(!path.exists());
    }

    #[test]
    fn prepared_config_retains_other_fields_and_publishes_only_after_durability() {
        let directory = tempfile::tempdir().expect("config directory");
        let path = directory.path().join("config.toml");
        let manager = ConfigManager::new(path.clone()).expect("config manager");
        let original = Arc::clone(&manager.get());
        let mut desired = (*original).clone();
        desired.daemon.canvas_width = 800;
        manager
            .with_prepared_mutation(
                &original,
                |config| config.daemon.canvas_width = 800,
                |prepared| {
                    assert_eq!(prepared.persist(), AtomicWriteOutcome::Written);
                    assert!(manager.is_current(&original));
                    prepared.publish();
                    Ok(())
                },
            )
            .expect("prepared callback")
            .expect("current source");
        assert_eq!(
            serde_json::to_value(manager.get().as_ref()).expect("live config"),
            serde_json::to_value(&desired).expect("desired config")
        );
        assert_eq!(
            ConfigManager::load(&path)
                .expect("durable config")
                .daemon
                .canvas_width,
            800
        );
        assert!(
            manager
                .with_prepared_mutation(&original, |_| {}, |_| Ok(()))
                .expect("stale comparison")
                .is_none()
        );
    }
}
