use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use hypercolor_types::portable_setup::{PortableDisplayFace, SetupBindings};
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{DisplayPreference, DisplayPreferencesStore};
use crate::domain::DomainError;
use crate::domain::portable_setup::prepared::PreparedStoreReplacement;
use crate::persistence::serialize_json_pretty;

impl DisplayPreferencesStore {
    pub(crate) async fn prepare_portable_replace(
        store: Arc<RwLock<Self>>,
        expected: &BTreeMap<Uuid, PortableDisplayFace>,
        desired: &BTreeMap<Uuid, PortableDisplayFace>,
        bindings: &SetupBindings,
    ) -> Result<PreparedStoreReplacement<Self>, DomainError> {
        let guard = store.write_owned().await;
        let materialize = |source: &BTreeMap<Uuid, PortableDisplayFace>| {
            source
                .iter()
                .filter_map(|(id, face)| {
                    bindings.local_device(*id).map(|device| {
                        (
                            device,
                            DisplayPreference {
                                effect_id: face.effect_id,
                                controls: face.controls.clone(),
                                blend_mode: face.blend_mode,
                                opacity: face.opacity,
                            },
                        )
                    })
                })
                .collect::<HashMap<_, _>>()
        };
        let actual = guard
            .preferences
            .iter()
            .filter(|(id, _)| bindings.devices.contains_key(id))
            .map(|(id, value)| (*id, value.clone()))
            .collect::<HashMap<_, _>>();
        if actual != materialize(expected) {
            return Err(DomainError::conflict(
                "display faces changed before setup restoration",
            ));
        }
        let mut candidate = guard.clone();
        candidate
            .preferences
            .retain(|id, _| !bindings.devices.contains_key(id));
        candidate.preferences.extend(materialize(desired));
        let payload = serialize_json_pretty(&candidate.preferences)
            .map_err(|error| DomainError::Internal(error.into()))?;
        let writer = candidate.writer.clone();
        Ok(PreparedStoreReplacement::new(
            guard, candidate, &writer, payload,
        ))
    }
}
