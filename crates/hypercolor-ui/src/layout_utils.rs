//! Shared layout zone CRUD helpers — used by layout palette, zone properties,
//! and anywhere zones are added, removed, or restored from cache.

use std::collections::HashSet;

use leptos::prelude::*;

use crate::api::{self, ZoneSummary};
use crate::channel_names;
use crate::layout_geometry;
use crate::style_utils::uuid_v4_hex;
use crate::toasts;
use hypercolor_types::spatial::{DeviceZone, NormalizedPosition, SpatialLayout};

/// Type alias for the removed-zone stash, keyed by (device_id, zone_name).
pub type ZoneCache = std::collections::HashMap<(String, Option<String>), DeviceZone>;

/// Compute the next `display_order` value for a new zone added to the layout.
pub fn next_display_order(layout: &Signal<Option<SpatialLayout>>) -> i32 {
    layout.with_untracked(|current| {
        current
            .as_ref()
            .and_then(|l| l.zones.iter().map(|z| z.display_order).max())
            .unwrap_or(-1)
            + 1
    })
}

/// Read canvas pixel dimensions from the layout signal.
pub fn current_canvas_dimensions(layout: &Signal<Option<SpatialLayout>>) -> (u32, u32) {
    layout.with_untracked(|current| {
        current
            .as_ref()
            .map(|layout| (layout.canvas_width.max(1), layout.canvas_height.max(1)))
            .unwrap_or((320, 200))
    })
}

/// Create a default `DeviceZone` placed at canvas center.
#[allow(clippy::too_many_arguments)]
pub fn create_default_zone(
    device_id: &str,
    device_name: &str,
    zone: Option<&ZoneSummary>,
    total_leds: usize,
    canvas_width: u32,
    canvas_height: u32,
    display_order: i32,
) -> DeviceZone {
    let defaults = layout_geometry::default_zone_visuals(
        device_name,
        zone,
        total_leds,
        canvas_width,
        canvas_height,
    );
    let zone_name = zone.map(|z| z.name.clone());
    let display_name = zone.map_or_else(
        || device_name.to_owned(),
        |z| {
            let channel_name = channel_names::effective_channel_name(device_id, &z.id, &z.name);
            prefixed_channel_display_name(device_name, &channel_name)
        },
    );

    DeviceZone {
        id: format!("zone_{}", uuid_v4_hex()),
        name: display_name,
        device_id: device_id.to_string(),
        zone_name,
        position: NormalizedPosition::new(0.5, 0.5),
        size: layout_geometry::normalize_zone_size_for_editor(
            NormalizedPosition::new(0.5, 0.5),
            defaults.size,
            &defaults.topology,
        ),
        rotation: 0.0,
        scale: 1.0,
        orientation: defaults.orientation,
        topology: defaults.topology,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: defaults.shape,
        shape_preset: defaults.shape_preset,
        attachment: None,
        display_order,
    }
}

/// Remove a device zone from the layout by device_id + zone_name,
/// stashing it in the cache so re-adding restores its settings.
pub fn remove_device_zone(
    device_id: &str,
    zone_name: Option<&str>,
    set_layout: &WriteSignal<Option<SpatialLayout>>,
    set_selected_zone_id: &WriteSignal<Option<String>>,
    set_is_dirty: &WriteSignal<bool>,
    set_removed_zone_cache: &WriteSignal<ZoneCache>,
) {
    set_layout.update(|l| {
        if let Some(layout) = l {
            if let Some(pos) = layout
                .zones
                .iter()
                .position(|z| z.device_id == device_id && z.zone_name.as_deref() == zone_name)
            {
                let removed = layout.zones.remove(pos);
                let key = (removed.device_id.clone(), removed.zone_name.clone());
                set_removed_zone_cache.update(|cache| {
                    cache.insert(key, removed);
                });
            }
        }
    });
    set_selected_zone_id.set(None);
    set_is_dirty.set(true);
}

/// Remove ALL zones for a device from the layout,
/// stashing each in the cache so re-adding restores settings.
pub fn remove_all_device_zones(
    device_id: &str,
    set_layout: &WriteSignal<Option<SpatialLayout>>,
    set_selected_zone_id: &WriteSignal<Option<String>>,
    set_is_dirty: &WriteSignal<bool>,
    set_removed_zone_cache: &WriteSignal<ZoneCache>,
) {
    set_layout.update(|l| {
        if let Some(layout) = l {
            set_removed_zone_cache.update(|cache| {
                for zone in layout.zones.iter().filter(|z| z.device_id == device_id) {
                    let key = (zone.device_id.clone(), zone.zone_name.clone());
                    cache.insert(key, zone.clone());
                }
            });
            layout.zones.retain(|z| z.device_id != device_id);
        }
    });
    set_selected_zone_id.set(None);
    set_is_dirty.set(true);
}

/// Add ALL zones for a device to the layout, checking the cache first.
#[allow(clippy::too_many_arguments)]
pub fn add_all_device_zones(
    device_id: &str,
    device_name: &str,
    zones: &[ZoneSummary],
    total_leds: usize,
    layout: &Signal<Option<SpatialLayout>>,
    set_layout: &WriteSignal<Option<SpatialLayout>>,
    set_selected_zone_id: &WriteSignal<Option<String>>,
    set_is_dirty: &WriteSignal<bool>,
    removed_zone_cache: &Signal<ZoneCache>,
    set_removed_zone_cache: &WriteSignal<ZoneCache>,
) {
    let (canvas_width, canvas_height) = current_canvas_dimensions(layout);
    let existing_zone_names: std::collections::HashSet<Option<String>> =
        layout.with_untracked(|current| {
            current
                .as_ref()
                .map(|l| {
                    l.zones
                        .iter()
                        .filter(|z| z.device_id == device_id)
                        .map(|z| z.zone_name.clone())
                        .collect()
                })
                .unwrap_or_default()
        });

    let has_cached =
        removed_zone_cache.with_untracked(|c| c.keys().any(|(did, _)| did == device_id));

    if existing_zone_names.is_empty() && !has_cached {
        let display_order = next_display_order(layout);
        if let Some(seed) = layout_geometry::seeded_device_layout(
            device_id,
            device_name,
            zones,
            canvas_width,
            canvas_height,
            display_order,
        ) {
            let selected_zone_id = seed.zones.first().map(|zone| zone.id.clone());
            set_layout.update(|l| {
                if let Some(current_layout) = l {
                    current_layout.zones.extend(seed.zones.clone());
                }
            });

            if let Some(zone_id) = selected_zone_id {
                set_selected_zone_id.set(Some(zone_id));
            }
            set_is_dirty.set(true);
            return;
        }
    }

    let mut first_new_id = None;
    set_layout.update(|l| {
        if let Some(current_layout) = l {
            let mut order = current_layout
                .zones
                .iter()
                .map(|z| z.display_order)
                .max()
                .unwrap_or(-1)
                + 1;
            for zone in zones {
                let zn = Some(zone.name.clone());
                if existing_zone_names.contains(&zn) {
                    continue;
                }
                let cache_key = (device_id.to_string(), Some(zone.name.clone()));
                let cached = removed_zone_cache.with_untracked(|c| c.get(&cache_key).cloned());
                let new_zone = if let Some(mut restored) = cached {
                    restored.id = format!("zone_{}", uuid_v4_hex());
                    set_removed_zone_cache.update(|c| {
                        c.remove(&cache_key);
                    });
                    restored
                } else {
                    create_default_zone(
                        device_id,
                        device_name,
                        Some(zone),
                        total_leds,
                        canvas_width,
                        canvas_height,
                        order,
                    )
                };
                order += 1;
                if first_new_id.is_none() {
                    first_new_id = Some(new_zone.id.clone());
                }
                current_layout.zones.push(new_zone);
            }
        }
    });

    if let Some(id) = first_new_id {
        set_selected_zone_id.set(Some(id));
    }
    set_is_dirty.set(true);
}

pub fn prefixed_channel_display_name(device_name: &str, channel_name: &str) -> String {
    if channel_name.eq_ignore_ascii_case(device_name) {
        device_name.to_owned()
    } else {
        format!("{device_name} · {channel_name}")
    }
}

pub fn apply_slot_display_names_to_seeded_attachment_layout(
    seeded: &mut layout_geometry::SeededAttachmentLayout,
    _device_name: &str,
    slot_display_names: &std::collections::HashMap<String, String>,
) {
    if slot_display_names.is_empty() {
        return;
    }

    for zone in &mut seeded.zones {
        let slot_id = zone
            .attachment
            .as_ref()
            .map(|attachment| attachment.slot_id.as_str())
            .or(zone.zone_name.as_deref());
        if let Some(display_name) = slot_id.and_then(|id| slot_display_names.get(id)) {
            zone.name.clone_from(display_name);
        }
    }
}

pub fn sync_channel_display_name_in_layout(
    layout: &mut SpatialLayout,
    layout_device_id: &str,
    device_name: &str,
    slot_id: &str,
    default_display_name: &str,
    previous_display_name: &str,
    new_display_name: &str,
) -> bool {
    let expected_old_names = [
        prefixed_channel_display_name(device_name, default_display_name),
        prefixed_channel_display_name(device_name, previous_display_name),
    ]
    .into_iter()
    .collect::<HashSet<_>>();
    let new_layout_name = prefixed_channel_display_name(device_name, new_display_name);

    let mut changed = false;

    for zone in &mut layout.zones {
        if zone.device_id != layout_device_id || zone.attachment.is_some() {
            continue;
        }
        if !zone_name_matches_slot_alias(zone.zone_name.as_deref(), Some(slot_id)) {
            continue;
        }
        if expected_old_names.contains(zone.name.as_str()) {
            zone.name.clone_from(&new_layout_name);
            changed = true;
        }
    }

    changed
}

pub fn replace_attachment_layout(
    layout: &mut SpatialLayout,
    device_id: &str,
    seeded: layout_geometry::SeededAttachmentLayout,
) {
    layout
        .zones
        .retain(|zone| !(zone.device_id == device_id && zone.attachment.is_some()));

    layout.zones.extend(seeded.zones);
}

/// Import a device's attachment zones into the active layout.
pub fn import_device_attachments(
    device_id: String,
    set_in_flight: WriteSignal<bool>,
    layouts_resource: leptos::prelude::LocalResource<Result<Vec<api::LayoutSummary>, String>>,
) {
    set_in_flight.set(true);
    leptos::task::spawn_local(async move {
        let result: Result<(usize, String), String> = async {
            let devices = api::fetch_devices().await?;
            let device = devices
                .iter()
                .find(|d| d.id == device_id)
                .ok_or_else(|| "Device not found".to_string())?
                .clone();
            let attachments = api::fetch_device_attachments(&device_id).await?;
            if attachments.suggested_zones.is_empty() {
                return Ok((0_usize, String::new()));
            }

            let mut layout = api::fetch_active_layout().await?;
            let layout_name = layout.name.clone();
            let layout_id = layout.id.clone();
            let mut seeded = layout_geometry::seeded_attachment_layout(
                &device.layout_device_id,
                &device.name,
                &attachments.suggested_zones,
                0,
            );
            let slot_display_names = attachments
                .slots
                .iter()
                .filter_map(|slot| {
                    channel_names::load_channel_name(&device.id, &slot.id)
                        .map(|display_name| (slot.id.clone(), display_name))
                })
                .collect::<std::collections::HashMap<_, _>>();
            apply_slot_display_names_to_seeded_attachment_layout(
                &mut seeded,
                &device.name,
                &slot_display_names,
            );
            let imported_count = seeded.zones.len();
            replace_attachment_layout(&mut layout, &device.layout_device_id, seeded);

            let req = api::UpdateLayoutApiRequest {
                name: None,
                description: None,
                canvas_width: None,
                canvas_height: None,
                zones: Some(layout.zones),
            };
            api::update_layout(&layout_id, &req).await?;
            api::apply_layout(&layout_id).await?;

            Ok((imported_count, layout_name))
        }
        .await;

        set_in_flight.set(false);
        match result {
            Ok((0, _)) => toasts::toast_info("No attachment zones ready to import"),
            Ok((count, layout_name)) => {
                layouts_resource.refetch();
                let noun = if count == 1 { "zone" } else { "zones" };
                toasts::toast_success(&format!(
                    "Imported {count} attachment {noun} into {layout_name}"
                ));
            }
            Err(error) => {
                toasts::toast_error(&format!("Attachment import failed: {error}"));
            }
        }
    });
}

/// Check if a slot ID matches a zone name (case-insensitive or slugified).
pub fn slot_id_matches_zone_name(slot_id: &str, zone_name: &str) -> bool {
    slot_id.eq_ignore_ascii_case(zone_name) || slot_id == slugify_slot_name(zone_name)
}

pub fn zone_name_matches_slot_alias(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.eq_ignore_ascii_case(right)
                || slot_id_matches_zone_name(left, right)
                || slot_id_matches_zone_name(right, left)
        }
        (None, None) => true,
        _ => false,
    }
}

fn representative_zone_id_with_filter(
    layout: &SpatialLayout,
    mut filter: impl FnMut(&hypercolor_types::spatial::DeviceZone) -> bool,
) -> Option<String> {
    layout
        .zones
        .iter()
        .enumerate()
        .filter(|(_, zone)| filter(zone))
        .min_by(|(left_index, left_zone), (right_index, right_zone)| {
            left_zone
                .display_order
                .cmp(&right_zone.display_order)
                .then_with(|| left_index.cmp(right_index))
        })
        .map(|(_, zone)| zone.id.clone())
}

pub fn representative_zone_id_for_device(
    layout: &SpatialLayout,
    device_id: &str,
) -> Option<String> {
    let suppressed = suppressed_attachment_source_zone_ids(layout);
    representative_zone_id_with_filter(layout, |zone| {
        zone.device_id == device_id && !suppressed.contains(&zone.id)
    })
    .or_else(|| representative_zone_id_with_filter(layout, |zone| zone.device_id == device_id))
}

pub fn representative_zone_id_for_device_slot(
    layout: &SpatialLayout,
    device_id: &str,
    zone_name: Option<&str>,
) -> Option<String> {
    let suppressed = suppressed_attachment_source_zone_ids(layout);
    representative_zone_id_with_filter(layout, |zone| {
        zone.device_id == device_id
            && zone_name_matches_slot_alias(zone.zone_name.as_deref(), zone_name)
            && !suppressed.contains(&zone.id)
    })
    .or_else(|| {
        representative_zone_id_with_filter(layout, |zone| {
            zone.device_id == device_id
                && zone_name_matches_slot_alias(zone.zone_name.as_deref(), zone_name)
        })
    })
}

pub fn selected_zone_matches_device_slot(
    layout: &SpatialLayout,
    selected_zone_id: &str,
    device_id: &str,
    zone_name: Option<&str>,
) -> bool {
    layout
        .zones
        .iter()
        .find(|zone| zone.id == selected_zone_id)
        .is_some_and(|zone| {
            zone.device_id == device_id
                && zone_name_matches_slot_alias(zone.zone_name.as_deref(), zone_name)
        })
}

pub fn suppressed_attachment_source_zone_ids(layout: &SpatialLayout) -> HashSet<String> {
    let attached_slots = layout
        .zones
        .iter()
        .filter_map(|zone| {
            let attachment = zone.attachment.as_ref()?;
            Some((zone.device_id.as_str(), attachment.slot_id.as_str()))
        })
        .collect::<Vec<_>>();

    layout
        .zones
        .iter()
        .filter_map(|zone| {
            let zone_name = zone.zone_name.as_deref()?;
            (zone.attachment.is_none()
                && attached_slots.iter().any(|(device_id, slot_id)| {
                    zone.device_id == *device_id && slot_id_matches_zone_name(slot_id, zone_name)
                }))
            .then(|| zone.id.clone())
        })
        .collect()
}

fn slugify_slot_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut previous_dash = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            previous_dash = false;
            continue;
        }

        if !out.is_empty() && !previous_dash {
            out.push('-');
            previous_dash = true;
        }
    }

    out.trim_matches('-').to_owned()
}
