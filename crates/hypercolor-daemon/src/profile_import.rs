//! One-time import of legacy profiles into the named scene store.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::identity::LayoutId;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{
    ColorInterpolation, DisplayFaceTarget, EasingFunction, Scene, SceneId, SceneKind,
    SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior, Zone, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use serde::Deserialize;
use tracing::warn;
use uuid::{Uuid, uuid};

use crate::persistence::AtomicWriteCommitResult;
use crate::scene_store::SceneStore;

const PROFILE_IMPORT_NAMESPACE: Uuid = uuid!("2a937b6a-4ba1-5eb8-b02e-d3ca6eeaf3bd");

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProfileImportOutcome {
    NoSource,
    Imported { profiles: usize, backup: PathBuf },
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyProfile {
    id: String,
    name: String,
    description: Option<String>,
    #[serde(default)]
    primary: Option<LegacyProfilePrimary>,
    #[serde(default)]
    displays: Vec<LegacyProfileDisplay>,
    brightness: Option<u8>,
    layout_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProfilePrimary {
    effect_id: EffectId,
    #[serde(default)]
    controls: HashMap<String, ControlValue>,
    #[serde(default)]
    active_preset_id: Option<PresetId>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProfileDisplay {
    device_id: DeviceId,
    effect_id: EffectId,
    #[serde(default)]
    controls: HashMap<String, ControlValue>,
}

impl LegacyProfile {
    fn normalized(mut self) -> Self {
        let trimmed_name = self.name.trim().to_owned();
        self.name.clone_from(&trimmed_name);
        self.description = self
            .description
            .map(|description| description.trim().to_owned())
            .filter(|description| !description.is_empty());
        self.brightness = self.brightness.map(|brightness| brightness.min(100));
        let mut seen_displays = HashSet::new();
        self.displays
            .retain(|display| seen_displays.insert(display.device_id));
        self
    }
}

pub(crate) fn import_profiles(
    profiles_path: &Path,
    scenes_path: &Path,
    layouts: &HashMap<String, SpatialLayout>,
    default_layout: &SpatialLayout,
) -> anyhow::Result<ProfileImportOutcome> {
    if !profiles_path.exists() {
        return Ok(ProfileImportOutcome::NoSource);
    }

    let mut scene_store = SceneStore::load(scenes_path)
        .with_context(|| format!("failed to load scenes from {}", scenes_path.display()))?;

    let bytes = fs::read(profiles_path)
        .with_context(|| format!("failed to read profiles at {}", profiles_path.display()))?;
    let profiles = serde_json::from_slice::<HashMap<String, LegacyProfile>>(&bytes)
        .with_context(|| format!("failed to parse profiles at {}", profiles_path.display()))?;
    let profile_count = profiles.len();
    let merged = merge_profiles(&scene_store, profiles, layouts, default_layout)?;
    let pending = scene_store
        .reserve_save(merged.into_values())
        .context("failed to prepare imported scene snapshot")?;

    match scene_store.save_reserved_stage_aware(pending) {
        AtomicWriteCommitResult::DurableWritten => {}
        AtomicWriteCommitResult::Superseded => {
            bail!("profile import was superseded before the scene store became durable");
        }
        AtomicWriteCommitResult::FailedBeforeReplacement(error) => {
            return Err(error).context("profile import did not replace the scene store");
        }
        AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
            return Err(error).context("profile import scene replacement is not durable");
        }
    }

    let backup = retire_profiles(profiles_path)?;
    Ok(ProfileImportOutcome::Imported {
        profiles: profile_count,
        backup,
    })
}

fn merge_profiles(
    scene_store: &SceneStore,
    profiles: HashMap<String, LegacyProfile>,
    layouts: &HashMap<String, SpatialLayout>,
    default_layout: &SpatialLayout,
) -> anyhow::Result<HashMap<SceneId, Scene>> {
    let mut merged = scene_store
        .list()
        .cloned()
        .map(|scene| (scene.id, scene))
        .collect::<HashMap<_, _>>();
    let mut ordered = profiles.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|(left_key, left), (right_key, right)| {
        left.id.cmp(&right.id).then_with(|| left_key.cmp(right_key))
    });

    let mut seen_profile_ids = HashSet::new();
    let mut occupied_names = merged
        .values()
        .map(|scene| scene.name.to_lowercase())
        .collect::<HashSet<_>>();
    occupied_names.insert("default".to_owned());

    for (_, profile) in ordered {
        let profile = profile.normalized();
        if profile.id.trim().is_empty() {
            bail!("legacy profile id must not be empty");
        }
        if profile.name.trim().is_empty() {
            bail!("legacy profile name must not be empty");
        }
        if !seen_profile_ids.insert(profile.id.clone()) {
            bail!("duplicate legacy profile id: {}", profile.id);
        }
        let scene_id = imported_scene_id(&profile.id);
        let name = if let Some(existing) = merged.get(&scene_id) {
            existing.name.clone()
        } else {
            allocate_import_name(&profile.name, &occupied_names)?
        };
        occupied_names.insert(name.to_lowercase());
        let scene = convert_profile(profile, scene_id, name, layouts, default_layout)?;
        merged.insert(scene_id, scene);
    }

    Ok(merged)
}

fn convert_profile(
    profile: LegacyProfile,
    scene_id: SceneId,
    name: String,
    layouts: &HashMap<String, SpatialLayout>,
    default_layout: &SpatialLayout,
) -> anyhow::Result<Scene> {
    let layout_id = profile.layout_id.as_deref().map(LayoutId::from_persisted);
    let primary_layout = profile
        .layout_id
        .as_ref()
        .and_then(|id| layouts.get(id))
        .unwrap_or(default_layout);

    let mut groups =
        Vec::with_capacity(usize::from(profile.primary.is_some()) + profile.displays.len());
    if let Some(primary) = profile.primary {
        groups.push(primary_zone(scene_id, primary, primary_layout.clone()));
    }
    for display in profile.displays {
        groups.push(display_zone(scene_id, display));
    }

    let scene = Scene {
        id: scene_id,
        name,
        description: profile.description,
        zones: groups,
        zones_revision: 0,
        transition: TransitionSpec {
            duration_ms: 0,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
        layout_id,
        activation_brightness: profile.brightness.map(|value| f32::from(value) / 100.0),
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Snapshot,
    };
    if let Err(errors) = scene.validate() {
        bail!(
            "legacy profile '{}' cannot be represented as a scene: {}",
            profile.id,
            errors.join("; ")
        );
    }
    Ok(scene)
}

fn primary_zone(scene_id: SceneId, primary: LegacyProfilePrimary, layout: SpatialLayout) -> Zone {
    let zone_id = derived_zone_id(scene_id, "zone:primary");
    let layer_id = derived_layer_id(scene_id, "layer:primary");
    let controls = primary.controls;
    let layer = SceneLayer::from_effect(
        layer_id,
        primary.effect_id,
        controls.clone(),
        HashMap::new(),
        primary.active_preset_id,
    );
    Zone {
        id: zone_id,
        name: "Default".to_owned(),
        description: Some("Default zone.".to_owned()),
        layers: vec![layer],
        layout,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Primary,
        controls_version: 0,
        layers_version: 0,
    }
}

fn display_zone(scene_id: SceneId, display: LegacyProfileDisplay) -> Zone {
    let device_id = display.device_id;
    let zone_key = format!("zone:display:{device_id}");
    let layer_key = format!("layer:display:{device_id}");
    let controls = display.controls;
    let layer = SceneLayer::from_effect(
        derived_layer_id(scene_id, &layer_key),
        display.effect_id,
        controls.clone(),
        HashMap::new(),
        None,
    );
    Zone {
        id: derived_zone_id(scene_id, &zone_key),
        name: format!("{device_id} Face"),
        description: Some(format!("Display face for {device_id}")),
        layers: vec![layer],
        layout: deferred_display_layout(device_id),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: Some(DisplayFaceTarget::new(device_id)),
        role: ZoneRole::Display,
        controls_version: 0,
        layers_version: 0,
    }
}

fn deferred_display_layout(device_id: DeviceId) -> SpatialLayout {
    SpatialLayout {
        id: format!("display-face:{device_id}"),
        name: format!("{device_id} Display Face"),
        description: None,
        canvas_width: 1,
        canvas_height: 1,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn imported_scene_id(profile_id: &str) -> SceneId {
    SceneId(Uuid::new_v5(
        &PROFILE_IMPORT_NAMESPACE,
        profile_id.as_bytes(),
    ))
}

fn derived_zone_id(scene_id: SceneId, key: &str) -> ZoneId {
    ZoneId(Uuid::new_v5(&scene_id.0, key.as_bytes()))
}

fn derived_layer_id(scene_id: SceneId, key: &str) -> SceneLayerId {
    SceneLayerId::from_uuid(Uuid::new_v5(&scene_id.0, key.as_bytes()))
}

fn allocate_import_name(name: &str, occupied: &HashSet<String>) -> anyhow::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("legacy profile name must not be empty");
    }
    if !occupied.contains(&name.to_lowercase()) {
        return Ok(name.to_owned());
    }

    let first = format!("{name} (imported)");
    if !occupied.contains(&first.to_lowercase()) {
        return Ok(first);
    }
    for suffix in 2_u64.. {
        let candidate = format!("{name} (imported {suffix})");
        if !occupied.contains(&candidate.to_lowercase()) {
            return Ok(candidate);
        }
    }
    unreachable!("an unbounded numeric suffix must eventually be free")
}

fn retire_profiles(path: &Path) -> anyhow::Result<PathBuf> {
    let backup = backup_path(path);
    fs::rename(path, &backup).with_context(|| {
        format!(
            "failed to retire imported profiles from {} to {}",
            path.display(),
            backup.display()
        )
    })?;

    #[cfg(unix)]
    if let Err(error) = sync_parent_directory(path) {
        warn!(
            backup = %backup.display(),
            %error,
            "Profile backup is visible but its directory entry is not proven durable"
        );
    }
    Ok(backup)
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::File::open(parent).and_then(|directory| directory.sync_all())
}

fn backup_path(path: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let mut name = path
        .file_name()
        .map_or_else(|| OsString::from("profiles.json"), ToOwned::to_owned);
    name.push(format!(".migrated-{timestamp}"));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests;
