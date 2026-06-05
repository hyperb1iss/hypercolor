//! Shared helpers for applying driver-owned control changes.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, anyhow, bail};
use hypercolor_types::controls::{
    AppliedControlChange, ApplyControlChangesResponse, ApplyImpact, ControlChange,
    ControlFieldDescriptor, ControlValue, ControlValueMap,
};

use crate::{DriverHost, ValidatedControlChanges, control_surface};

/// Validate a batch of control changes against field descriptors.
///
/// Driver-specific validation can be supplied with `validate_change`.
pub fn validate_control_changes(
    driver_label: &str,
    fields: impl IntoIterator<Item = ControlFieldDescriptor>,
    changes: &[ControlChange],
    mut validate_change: impl FnMut(&ControlChange) -> Result<()>,
) -> Result<ValidatedControlChanges> {
    let fields = fields
        .into_iter()
        .map(|field| (field.id.clone(), field))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut impacts = Vec::new();

    for change in changes {
        if !seen.insert(change.field_id.as_str()) {
            bail!(
                "duplicate {driver_label} control field: {}",
                change.field_id
            );
        }
        let field = fields
            .get(&change.field_id)
            .ok_or_else(|| anyhow!("unknown {driver_label} control field: {}", change.field_id))?;
        field
            .value_type
            .validate_value(&change.value)
            .with_context(|| {
                format!("invalid {driver_label} control field: {}", change.field_id)
            })?;
        validate_change(change)?;
        push_unique_impact(&mut impacts, field.apply_impact.clone());
    }

    Ok(ValidatedControlChanges {
        changes: changes.to_vec(),
        impacts,
    })
}

/// Apply changed values into a value map and compute old/new revisions.
pub fn apply_value_changes(
    mut values: ControlValueMap,
    changes: &[ControlChange],
    revision: impl Fn(&ControlValueMap) -> u64,
    mut value_for_change: impl FnMut(&ControlChange) -> ControlValue,
) -> (ControlValueMap, u64, u64) {
    let previous_revision = revision(&values);
    for change in changes {
        values.insert(change.field_id.clone(), value_for_change(change));
    }
    let revision = revision(&values);
    (values, previous_revision, revision)
}

/// Apply and persist driver-scoped value changes.
pub async fn apply_driver_value_changes(
    host: &dyn DriverHost,
    driver_id: &str,
    values: ControlValueMap,
    changes: ValidatedControlChanges,
) -> Result<ApplyControlChangesResponse> {
    let control_host = host
        .control_host()
        .ok_or_else(|| anyhow!("driver control host services are unavailable"))?;
    let (values, previous_revision, revision) = apply_value_changes(
        values,
        &changes.changes,
        control_surface::value_map_revision,
        |change| change.value.clone(),
    );
    control_host
        .driver_config_store()
        .save_driver_values(driver_id, values.clone())
        .await?;

    Ok(apply_response(
        format!("driver:{driver_id}"),
        previous_revision,
        revision,
        changes,
        values,
    ))
}

/// Build a successful apply response from accepted changes and final values.
#[must_use]
pub fn apply_response(
    surface_id: impl Into<String>,
    previous_revision: u64,
    revision: u64,
    changes: ValidatedControlChanges,
    values: ControlValueMap,
) -> ApplyControlChangesResponse {
    ApplyControlChangesResponse {
        surface_id: surface_id.into(),
        previous_revision,
        revision,
        accepted: changes
            .changes
            .into_iter()
            .map(|change| AppliedControlChange {
                field_id: change.field_id,
                value: change.value,
            })
            .collect(),
        rejected: Vec::new(),
        impacts: changes.impacts,
        values,
    }
}

fn push_unique_impact(impacts: &mut Vec<ApplyImpact>, impact: ApplyImpact) {
    if !impacts.contains(&impact) {
        impacts.push(impact);
    }
}
