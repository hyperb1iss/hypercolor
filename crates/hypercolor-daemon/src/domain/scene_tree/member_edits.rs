//! Atomic, reversible membership edits without replacing scene content.

use std::collections::HashSet;

use hypercolor_types::api::scene::{
    EditMembersRequest, EditMembersResponse, MemberAssignmentTarget, MemberEdit, MemberState,
};

use super::{
    AssignMembersRequest, DomainError, Output, OutputPlacement, ResourceKind, Scene,
    SceneTreeContext, ZoneRole, active_scene, check_scene_revision, ensure_live_zone_mutable,
    member_placement, mint_missing_outputs, reconcile_member_exclusions, scene_document,
    zone_layout_error,
};

/// Commit only the named memberships, preserving all unrelated scene data.
///
/// # Errors
/// Rejects stale revisions, another active scene, mismatched preconditions,
/// duplicate output identities, display zones, and invalid geometry before committing.
pub async fn edit_members(
    ctx: &SceneTreeContext,
    mut request: EditMembersRequest,
    expected_revision: u64,
) -> Result<EditMembersResponse, DomainError> {
    if request.changes.is_empty() == request.assignment.is_none() {
        return Err(DomainError::validation(
            "provide either an assignment target or explicit membership changes",
        ));
    }
    let minted = if let Some(assignment) = &request.assignment {
        mint_missing_outputs(
            ctx,
            &AssignMembersRequest {
                device_id: assignment.device_id.clone(),
                segments: assignment.segments.clone(),
            },
        )
        .await?
    } else {
        Vec::new()
    };
    let mut mutation = ctx.scene.begin_mutation().await;
    check_scene_revision(&mutation, Some(expected_revision))?;
    let scene_id = mutation.active_scene_for_runtime_mutation("editing zone memberships")?;
    if scene_id != request.scene_id {
        return Err(DomainError::conflict("the active scene changed"));
    }
    let previous = active_scene(&mutation)?;
    if let Some(assignment) = request.assignment {
        ensure_live_zone_mutable(&mutation, assignment.zone_id)?;
        request.changes = assignment_changes(&previous, &assignment, minted)?;
        if request.changes.is_empty() {
            return Ok(EditMembersResponse {
                document: scene_document(&previous, expected_revision),
                changes: Vec::new(),
            });
        }
    }
    let mut changes = Vec::with_capacity(request.changes.len());
    let mut ids = HashSet::new();
    for change in request.changes {
        let state = change
            .before
            .as_ref()
            .or(change.after.as_ref())
            .ok_or_else(|| {
                DomainError::validation("membership edit must have a before or after state")
            })?;
        let id = state.output.id.clone();
        if !ids.insert(id.clone()) {
            return Err(DomainError::validation(
                "an output may only appear once per edit",
            ));
        }
        let actual = find_state(&previous, &id);
        match (&change.before, &actual) {
            (None, None) => {}
            (Some(expected), Some(actual)) if same_public_state(expected, actual) => {}
            _ => {
                return Err(DomainError::conflict(
                    "output membership or placement changed",
                ));
            }
        }
        if let Some(before) = &actual {
            ensure_live_zone_mutable(&mutation, before.zone_id)?;
        }
        let after = change
            .after
            .map(|mut after| {
                if after.output.id != id {
                    return Err(DomainError::validation(
                        "membership edits cannot replace output identity",
                    ));
                }
                ensure_live_zone_mutable(&mutation, after.zone_id)?;
                if let Some(before) = &actual {
                    if before.output.device_id != after.output.device_id
                        || before.output.zone_name != after.output.zone_name
                        || before.output.topology != after.output.topology
                    {
                        return Err(DomainError::validation(
                            "membership edits cannot change hardware bindings",
                        ));
                    }
                    let requested = after.output;
                    after.output = before.output.clone();
                    after.output.position = requested.position;
                    after.output.size = requested.size;
                    after.output.rotation = requested.rotation;
                    after.output.scale = requested.scale;
                    after.output.orientation = requested.orientation;
                }
                validate_output(&after.output)?;
                after.output.led_positions =
                    hypercolor_core::spatial::generate_positions(&after.output.topology);
                Ok(after)
            })
            .transpose()?;
        changes.push(MemberEdit {
            before: actual,
            after,
        });
    }

    // Remove the complete touched set first, allowing a move between zones
    // without a temporary duplicate device/segment binding.
    for change in &changes {
        if let Some(before) = &change.before {
            mutation
                .unassign_output(scene_id, &before.output.id)
                .map_err(|_| {
                    DomainError::conflict("output disappeared while editing memberships")
                })?;
        }
    }
    for change in &changes {
        if let Some(after) = &change.after {
            mutation
                .assign_output(
                    scene_id,
                    after.zone_id,
                    after.output.clone(),
                    OutputPlacement::Preserve,
                )
                .map_err(|_| DomainError::not_found(ResourceKind::Zone, after.zone_id))?;
        }
    }
    let candidate = active_scene(&mutation)?;
    // Restore authored ordering as well as placement. Each final insertion
    // index is interpreted against the complete final output list.
    for zone in &candidate.zones {
        let mut inserted: Vec<_> = changes
            .iter()
            .filter_map(|change| change.after.as_ref())
            .filter(|after| after.zone_id == zone.id)
            .collect();
        if inserted.is_empty() {
            continue;
        }
        inserted.sort_by_key(|after| after.index);
        let mut layout = zone.layout.clone();
        layout.zones.retain(|output| !ids.contains(&output.id));
        for after in inserted {
            let index = after.index.min(layout.zones.len());
            layout.zones.insert(index, after.output.clone());
        }
        mutation
            .set_zone_layout(scene_id, zone.id, layout)
            .map_err(|error| zone_layout_error(error, zone.id))?;
    }
    for zone_id in changes
        .iter()
        .flat_map(|change| {
            change
                .before
                .iter()
                .chain(change.after.iter())
                .map(|state| state.zone_id)
        })
        .collect::<HashSet<_>>()
    {
        mutation.retire_zone_preview(scene_id, zone_id);
    }
    let committed = active_scene(&mutation)?;
    for change in &mut changes {
        if let Some(after) = &change.after {
            change.after = find_state(&committed, &after.output.id);
        }
    }
    let commit = ctx.scene.commit(mutation).await?;
    ctx.scene.save_runtime_session().await;
    ctx.layout
        .sync_runtime_connectivity(ctx.scene.layout_runtime())
        .await;
    reconcile_member_exclusions(ctx, scene_id, &previous.zones).await;
    Ok(EditMembersResponse {
        document: scene_document(&committed, commit.revision()),
        changes,
    })
}

fn find_state(scene: &Scene, id: &str) -> Option<MemberState> {
    scene.zones.iter().find_map(|zone| {
        zone.layout
            .zones
            .iter()
            .enumerate()
            .find(|(_, output)| output.id == id)
            .map(|(index, output)| MemberState {
                zone_id: zone.id,
                output: output.clone(),
                index,
            })
    })
}

fn same_public_state(expected: &MemberState, actual: &MemberState) -> bool {
    expected.zone_id == actual.zone_id
        && expected.index == actual.index
        && expected.output.id == actual.output.id
        && expected.output.device_id == actual.output.device_id
        && expected.output.zone_name == actual.output.zone_name
        && member_placement(&expected.output) == member_placement(&actual.output)
}

fn validate_output(output: &Output) -> Result<(), DomainError> {
    if output.id.trim().is_empty() || output.device_id.trim().is_empty() {
        return Err(DomainError::validation(
            "output and device identities must not be empty",
        ));
    }
    let values = [
        output.position.x,
        output.position.y,
        output.size.x,
        output.size.y,
        output.rotation,
        output.scale,
    ];
    if values.iter().any(|value| !value.is_finite())
        || output.size.x <= 0.0
        || output.size.y <= 0.0
        || output.scale <= 0.0
    {
        return Err(DomainError::validation(
            "output placement must be finite with positive size and scale",
        ));
    }
    super::super::layout::validate_output_sampling_radii(output)
}

fn assignment_changes(
    scene: &Scene,
    target: &MemberAssignmentTarget,
    mut minted: Vec<Output>,
) -> Result<Vec<MemberEdit>, DomainError> {
    let mut hinted = HashSet::new();
    for hint in &target.placements {
        if !hinted.insert(&hint.segment) {
            return Err(DomainError::validation(
                "placement hints must name each segment once",
            ));
        }
        for output in minted
            .iter_mut()
            .filter(|output| output.zone_name == hint.segment)
        {
            output.position = hint.position;
            output.size = hint.size;
            output.rotation = hint.rotation;
            output.scale = hint.scale;
            output.orientation = hint.orientation;
        }
    }
    let selected = |output: &Output| {
        output.device_id == target.device_id
            && (target.segments.is_empty()
                || output
                    .zone_name
                    .as_ref()
                    .is_some_and(|segment| target.segments.contains(segment)))
    };
    let mut outputs: Vec<_> = scene
        .zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display)
        .flat_map(|zone| zone.layout.zones.iter())
        .filter(|output| selected(output))
        .cloned()
        .collect();
    // Existing authored outputs take precedence over a generated segment.
    // One segment can contain several independently placed attachments.
    let held_segments: HashSet<_> = outputs
        .iter()
        .map(|output| output.zone_name.clone())
        .collect();
    outputs.extend(
        minted
            .into_iter()
            .filter(|output| selected(output) && !held_segments.contains(&output.zone_name)),
    );
    if outputs.is_empty() {
        return Err(DomainError::not_found(
            ResourceKind::Device,
            &target.device_id,
        ));
    }
    if target.segments.iter().any(|segment| {
        !outputs
            .iter()
            .any(|output| output.zone_name.as_ref() == Some(segment))
    }) {
        return Err(DomainError::validation(
            "assignment contains an unknown light segment",
        ));
    }
    let mut index = scene
        .zones
        .iter()
        .find(|zone| zone.id == target.zone_id)
        .map_or(0, |zone| zone.layout.zones.len());
    Ok(outputs
        .into_iter()
        .filter_map(|output| {
            let before = find_state(scene, &output.id);
            if before
                .as_ref()
                .is_some_and(|before| before.zone_id == target.zone_id)
            {
                return None;
            }
            let after = MemberState {
                zone_id: target.zone_id,
                output,
                index,
            };
            index += 1;
            Some(MemberEdit {
                before,
                after: Some(after),
            })
        })
        .collect())
}
