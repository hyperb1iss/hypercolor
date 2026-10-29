use std::collections::BTreeMap;
use std::fmt::Display;

use hypercolor_types::portable_setup::PortableSetupV1;

use super::RestoreMode;
use crate::domain::DomainError;

/// Apply only the sender's changes. Missing members mean deletion only when a
/// previous accepted manifest proves they existed on that sender.
pub(super) fn candidate(
    current: &PortableSetupV1,
    incoming: &PortableSetupV1,
    mode: &RestoreMode,
    apply_selection: bool,
) -> Result<PortableSetupV1, DomainError> {
    if current.setup_id != incoming.setup_id {
        return Err(DomainError::validation("setup identities do not match"));
    }
    let previous = match mode {
        RestoreMode::Merge => None,
        RestoreMode::Synchronize { previous } => {
            if previous.setup_id != incoming.setup_id {
                return Err(DomainError::validation(
                    "previous setup identity does not match",
                ));
            }
            Some(previous.as_ref())
        }
    };
    let mut result = current.clone();
    macro_rules! merge_maps {
        ($($field:ident),+ $(,)?) => {$({
            result.$field = merge_map(
                stringify!($field), &current.$field, &incoming.$field,
                previous.map(|value| &value.$field),
            )?;
        })+};
    }
    merge_maps!(
        devices,
        logical_devices,
        scenes,
        layouts,
        presets,
        playlists,
        attachments,
        attachment_templates,
        effects,
        assets,
        display_faces,
        device_output
    );
    result.name = merge_value(
        "name",
        &current.name,
        &incoming.name,
        previous.map(|value| &value.name),
    )?;
    if apply_selection {
        result.selection = merge_value(
            "selection",
            &current.selection,
            &incoming.selection,
            previous.map(|value| &value.selection),
        )?;
        result.output = merge_value(
            "output",
            &current.output,
            &incoming.output,
            previous.map(|value| &value.output),
        )?;
    }
    result
        .validate()
        .map_err(|error| DomainError::validation(error.to_string()))?;
    Ok(result)
}

fn merge_value<T: Clone + PartialEq>(
    path: &str,
    current: &T,
    incoming: &T,
    previous: Option<&T>,
) -> Result<T, DomainError> {
    match previous {
        Some(previous) if previous == incoming => Ok(current.clone()),
        Some(previous) if current != previous && current != incoming => Err(conflict(path)),
        _ => Ok(incoming.clone()),
    }
}

fn merge_map<K: Ord + Clone + Display, V: Clone + PartialEq>(
    path: &str,
    current: &BTreeMap<K, V>,
    incoming: &BTreeMap<K, V>,
    previous: Option<&BTreeMap<K, V>>,
) -> Result<BTreeMap<K, V>, DomainError> {
    let mut result = current.clone();
    for (id, value) in incoming {
        let old = previous.and_then(|entries| entries.get(id));
        if previous.is_some() && old == Some(value) {
            continue;
        }
        if previous.is_some() && current.get(id) != old && current.get(id) != Some(value) {
            return Err(conflict(&format!("{path}.{id}")));
        }
        result.insert(id.clone(), value.clone());
    }
    if let Some(previous) = previous {
        for (id, old) in previous {
            if !incoming.contains_key(id) {
                if current.get(id).is_some_and(|value| value != old) {
                    return Err(conflict(&format!("{path}.{id}")));
                }
                result.remove(id);
            }
        }
    }
    Ok(result)
}

fn conflict(path: &str) -> DomainError {
    DomainError::conflict(format!("local lighting changed at {path}"))
}

#[cfg(test)]
mod tests {
    use super::merge_map;
    use std::collections::BTreeMap;

    #[test]
    fn manual_empty_restore_does_not_delete_existing_members() {
        let current = BTreeMap::from([("local", "authored")]);
        assert_eq!(
            merge_map("scenes", &current, &BTreeMap::new(), None)
                .expect("portable fixture should be valid"),
            current
        );
    }

    #[test]
    fn sync_removes_only_explicit_deletions_and_retains_local_additions() {
        let previous = BTreeMap::from([("removed", "old"), ("unchanged", "old")]);
        let current = BTreeMap::from([
            ("removed", "old"),
            ("unchanged", "local edit"),
            ("local", "new"),
        ]);
        let incoming = BTreeMap::from([("unchanged", "old")]);
        let actual = merge_map("scenes", &current, &incoming, Some(&previous))
            .expect("portable fixture should be valid");
        assert_eq!(
            actual,
            BTreeMap::from([("unchanged", "local edit"), ("local", "new")])
        );
    }

    #[test]
    fn sync_preserves_divergent_local_edits_and_already_applied_changes() {
        let previous = BTreeMap::from([("edited", "old")]);
        let current = BTreeMap::from([("edited", "local")]);
        assert!(merge_map("scenes", &current, &BTreeMap::new(), Some(&previous)).is_err());
        let incoming = BTreeMap::from([("edited", "remote")]);
        assert!(merge_map("scenes", &current, &incoming, Some(&previous)).is_err());
        assert_eq!(
            merge_map("scenes", &incoming, &incoming, Some(&previous))
                .expect("portable fixture should be valid"),
            incoming
        );
    }
}
