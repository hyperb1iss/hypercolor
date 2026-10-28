//! Reconcile restored processing preferences with already-created input sources.

use hypercolor_types::portable_setup::PortablePreferences;
use std::sync::Arc;

use super::{PortableSetupService, internal};
use crate::domain::DomainError;

impl PortableSetupService {
    /// Finish the live input update before acknowledging a durable restore.
    /// Endpoint selection and enablement still come from this installation.
    pub(super) async fn reconcile_runtime_preferences(
        &self,
        preferences: &PortablePreferences,
    ) -> Result<(), DomainError> {
        let manager = self.state.config_manager.as_ref().ok_or_else(|| {
            DomainError::validation("restored preferences require a configuration store")
        })?;
        let latest = Arc::clone(&manager.get());
        let mut expected = (*latest).clone();
        preferences
            .apply_to_config(&mut expected)
            .map_err(|error| DomainError::validation(error.to_string()))?;
        if PortablePreferences::from_config(&expected) != PortablePreferences::from_config(&latest)
        {
            return Err(DomainError::conflict(
                "lighting preferences changed before live restoration finished",
            ));
        }
        if preferences.audio.is_some() {
            crate::api::config::live::reconfigure_input_manager(&self.state)
                .await
                .map_err(internal)?;
        }
        if preferences.capture.is_some()
            && !crate::api::config::live::capture_runtime_matches(&self.state, &latest).await
        {
            crate::api::config::live::apply_capture_config_transaction(
                &self.state,
                &latest,
                latest.capture.clone(),
            )
            .await
            .map_err(internal)?;
        }
        let current = manager.get();
        let mut expected = (**current).clone();
        preferences
            .apply_to_config(&mut expected)
            .map_err(|error| DomainError::validation(error.to_string()))?;
        if PortablePreferences::from_config(&expected) != PortablePreferences::from_config(&current)
        {
            return Err(DomainError::conflict(
                "lighting preferences changed during live restoration",
            ));
        }
        Ok(())
    }
}
