use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use hypercolor_types::portable_setup::{PortableLogicalDevice, SetupBindings};
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{LogicalDevice, LogicalDeviceKind, serialize_segments};
use crate::domain::DomainError;
use crate::domain::portable_setup::prepared::PreparedStoreReplacement;
use crate::persistence::AtomicFileWriter;

pub(crate) async fn prepare_portable_replace(
    store: Arc<RwLock<HashMap<String, LogicalDevice>>>,
    path: &Path,
    expected: &BTreeMap<Uuid, PortableLogicalDevice>,
    desired: &BTreeMap<Uuid, PortableLogicalDevice>,
    bindings: &SetupBindings,
) -> Result<PreparedStoreReplacement<HashMap<String, LogicalDevice>>, DomainError> {
    let guard = store.write_owned().await;
    let mut candidate = guard.clone();
    let mut actual = BTreeMap::new();
    for (id, device) in guard.iter() {
        if device.kind == LogicalDeviceKind::Default {
            continue;
        }
        let Some(portable) = bindings.outputs.get(id) else {
            continue;
        };
        let Some(physical) = bindings.devices.get(&device.physical_device_id) else {
            continue;
        };
        actual.insert(
            *portable,
            PortableLogicalDevice {
                device: *physical,
                name: device.name.clone(),
                led_start: device.led_start,
                led_count: device.led_count,
                enabled: device.enabled,
            },
        );
        candidate.remove(id);
    }
    let expected_bound = expected
        .iter()
        .filter(|(_, value)| bindings.local_device(value.device).is_some())
        .map(|(id, value)| (*id, value.clone()))
        .collect::<BTreeMap<_, _>>();
    if actual != expected_bound {
        return Err(DomainError::conflict(
            "logical devices changed before setup restoration",
        ));
    }
    for (id, value) in desired {
        let Some(physical_device_id) = bindings.local_device(value.device) else {
            continue;
        };
        let local_id = bindings.local_output(*id);
        candidate.insert(
            local_id.clone(),
            LogicalDevice {
                id: local_id,
                physical_device_id,
                name: value.name.clone(),
                led_start: value.led_start,
                led_count: value.led_count,
                enabled: value.enabled,
                kind: LogicalDeviceKind::Segment,
            },
        );
    }
    let payload = serialize_segments(&candidate).map_err(DomainError::Internal)?;
    let writer =
        AtomicFileWriter::new(path).map_err(|error| DomainError::Internal(error.into()))?;
    Ok(PreparedStoreReplacement::new(
        guard, candidate, &writer, payload,
    ))
}
