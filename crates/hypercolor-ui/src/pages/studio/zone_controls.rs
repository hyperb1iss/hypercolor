//! Zone lifecycle mutations behind the Studio zone tab strip: create,
//! rename, recolor, pause, reorder, and delete.
//!
//! Every mutation carries the live scene's `revision` as the `If-Match`
//! precondition; a `Stale` outcome reloads the scene so the user retries
//! against the fresh revision rather than clobbering a concurrent edit.
//! Nothing here edits local state ahead of the daemon: the tab strip
//! re-renders from the refreshed scene, so a rejected write leaves the
//! strip exactly as it was. Each change runs through Studio history, so
//! Ctrl+Z takes it back.

use std::collections::HashSet;

use leptos::prelude::*;

use crate::api;
use crate::toasts;

use super::StudioContext;
use super::device_pane::other_led_zone_ids;
use super::history::rename_is_noop;
use super::tab_model::{ReorderTarget, next_zone_name, zone_move_index};

/// Create a zone under a fresh default name, open its tab, and put its
/// name into rename mode so the user can type over it. The pane opens
/// every other zone so the next move, bringing devices in, is in view,
/// until the new zone gets its first device or its tab is left.
/// `pending` holds true while the request is in flight.
pub fn create_zone(studio: StudioContext, pending: RwSignal<bool>) {
    let Some(scene) = studio.active_scene.get_untracked() else {
        toasts::toast_error("No active scene is available");
        return;
    };
    let names: Vec<&str> = scene.zones.iter().map(|zone| zone.name.as_str()).collect();
    let name = next_zone_name(&names);
    let others = other_led_zone_ids(&scene.zones);
    pending.set(true);
    studio.history.create_zone(
        name,
        None,
        Box::new(move |zone| {
            pending.try_set(false);
            let Some(zone) = zone else {
                return;
            };
            let id = zone.id.to_string();
            // Open the rows that were closed, and remember them as opened
            // for this zone alone.
            let opened: HashSet<String> = studio
                .pane_expanded
                .with_untracked(|expanded| others.difference(expanded).cloned().collect());
            studio
                .pane_expanded
                .update(|expanded| expanded.extend(opened.iter().cloned()));
            studio.pane_auto_expanded.set(Some((id.clone(), opened)));
            studio
                .zone_rename_draft
                .set(Some((id.clone(), zone.name.clone())));
            studio.selected_surface_id.set(Some(id));
        }),
    );
}

/// Rename a zone. A blank or unchanged name sends nothing.
pub fn commit_zone_rename(studio: StudioContext, zone_id: &str, name: &str) {
    let name = name.trim().to_owned();
    let unchanged = studio.active_scene.with_untracked(|scene| {
        scene.as_ref().is_some_and(|scene| {
            scene
                .zones
                .iter()
                .any(|zone| zone.id.to_string() == zone_id && rename_is_noop(zone, &name))
        })
    });
    if name.is_empty() || unchanged {
        return;
    }
    let request = api::zones::PatchZoneRequest {
        name: Some(name),
        ..Default::default()
    };
    apply_zone_update(studio, zone_id, request, "Zone update failed");
}

/// Set or clear a zone's identity color.
pub fn commit_zone_color(studio: StudioContext, zone_id: &str, color: Option<String>) {
    let request = api::zones::PatchZoneRequest {
        color: Some(color),
        ..Default::default()
    };
    apply_zone_update(studio, zone_id, request, "Zone update failed");
}

/// Pause (`enabled = false`) or resume a zone.
pub fn commit_zone_enabled(studio: StudioContext, zone_id: &str, enabled: bool) {
    let request = api::zones::PatchZoneRequest {
        enabled: Some(enabled),
        ..Default::default()
    };
    apply_zone_update(studio, zone_id, request, "Zone update failed");
}

/// Move a zone to `target` in the tab order. The daemon keeps one zone
/// order for LED zones and Screens together, so the index is computed
/// against the full list.
pub fn commit_zone_move(studio: StudioContext, zone_id: &str, target: &ReorderTarget) {
    let index = studio.active_scene.with_untracked(|scene| {
        scene
            .as_ref()
            .and_then(|scene| zone_move_index(&scene.zones, zone_id, target))
    });
    let Some(index) = index else {
        return;
    };
    let request = api::zones::PatchZoneRequest {
        index: Some(index),
        ..Default::default()
    };
    apply_zone_update(studio, zone_id, request, "Couldn't reorder zones");
}

fn apply_zone_update(
    studio: StudioContext,
    zone_id: &str,
    request: api::zones::PatchZoneRequest,
    failure: &'static str,
) {
    studio
        .history
        .patch_zone(zone_id.to_owned(), request, failure);
}
