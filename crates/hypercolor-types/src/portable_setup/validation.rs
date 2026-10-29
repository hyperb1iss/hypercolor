use super::{PortableBlob, PortableLayout, PortableSetupV1, SetupBindings};
use crate::control::ControlValue;
use crate::effect::EffectId;
use crate::layer::LayerSource;
use crate::library::PlaylistItemTarget;
use crate::library::PresetId;
use std::collections::{BTreeMap, HashSet};
use uuid::Uuid;

/// Invalid manifest data, reported before any destination store is changed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {message}")]
pub struct PortableSetupError {
    pub path: String,
    pub message: String,
}

/// Built-in and already-installed references accepted by a restore target.
#[derive(Debug, Default)]
pub struct PortableReferenceCatalog {
    pub effects: HashSet<EffectId>,
    pub presets: std::collections::HashMap<PresetId, EffectId>,
    pub attachment_templates: HashSet<String>,
}

fn error(path: impl Into<String>, message: impl Into<String>) -> PortableSetupError {
    PortableSetupError {
        path: path.into(),
        message: message.into(),
    }
}

/// Accept only paths that mean the same thing on Unix and Windows.
#[must_use]
pub fn is_portable_bundle_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.ends_with(['.', ' '])
                && !component.chars().any(|c| {
                    c.is_control() || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
                })
                && !matches!(
                    component
                        .split('.')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
}

impl PortableBlob {
    pub fn validate(&self) -> Result<(), PortableSetupError> {
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error("sha256", "expected a lowercase SHA-256 digest"));
        }
        Ok(())
    }
}

impl PortableSetupV1 {
    /// Check references against both the incoming manifest and the target's
    /// catalog. Built-in presets/effects/templates do not need bundled copies.
    pub fn validate_with_catalog(
        &self,
        catalog: &PortableReferenceCatalog,
    ) -> Result<(), PortableSetupError> {
        self.validate_references(catalog, |_| true, false)
    }

    /// Validate resources required by the current materialization. An unbound
    /// default face remains authored state until its target can be resolved;
    /// validate again with the new bindings before publishing that face.
    /// Scene, preset, playlist, and attachment references remain strict.
    pub fn validate_materializable_references(
        &self,
        catalog: &PortableReferenceCatalog,
        live_bindings: &SetupBindings,
    ) -> Result<(), PortableSetupError> {
        self.validate_references(
            catalog,
            |id| live_bindings.local_device(id).is_some(),
            false,
        )
    }

    /// Validate authored graph links while installed effects and media remain local.
    pub fn validate_configuration_references(
        &self,
        catalog: &PortableReferenceCatalog,
    ) -> Result<(), PortableSetupError> {
        self.validate_references(catalog, |_| true, true)
    }

    fn validate_references(
        &self,
        catalog: &PortableReferenceCatalog,
        materializes_face: impl Fn(Uuid) -> bool,
        allow_uninstalled_effects: bool,
    ) -> Result<(), PortableSetupError> {
        self.validate()?;
        let effect = |id: EffectId| {
            if id.0.is_nil() {
                return Err(error("effect reference", "effect identity must not be nil"));
            }
            if allow_uninstalled_effects
                || self.effects.contains_key(&id.0)
                || catalog.effects.contains(&id)
            {
                Ok(())
            } else {
                Err(error("effect reference", format!("effect {id} is absent")))
            }
        };
        let preset = |id: PresetId| {
            if id.0.is_nil() {
                return Err(error("preset reference", "preset identity must not be nil"));
            }
            let effect = self
                .presets
                .get(&id.0)
                .map(|preset| preset.effect_id)
                .or_else(|| catalog.presets.get(&id).copied());
            if effect.is_none() && !allow_uninstalled_effects {
                return Err(error("preset reference", format!("preset {id} is absent")));
            }
            Ok(effect)
        };
        let template = |id: &str| {
            if self.attachment_templates.contains_key(id)
                || catalog.attachment_templates.contains(id)
            {
                Ok(())
            } else {
                Err(error(
                    "attachment reference",
                    format!("template {id} is absent"),
                ))
            }
        };
        for scene in self.scenes.values() {
            for zone in &scene.zones {
                for layer in &zone.layers {
                    if let LayerSource::Effect {
                        effect_id,
                        preset_id,
                        ..
                    } = layer.source
                    {
                        effect(effect_id)?;
                        if let Some(id) = preset_id
                            && let Some(preset_effect) = preset(id)?
                            && preset_effect != effect_id
                        {
                            return Err(error(
                                "layer.preset_id",
                                "preset belongs to another effect",
                            ));
                        }
                    }
                }
            }
        }
        for saved in self.presets.values() {
            effect(saved.effect_id)?;
        }
        for playlist in self.playlists.values() {
            for item in &playlist.items {
                match item.target {
                    PlaylistItemTarget::Effect { effect_id } => effect(effect_id)?,
                    PlaylistItemTarget::Preset { preset_id } => {
                        if let Some(id) = preset(preset_id)? {
                            effect(id)?;
                        }
                    }
                }
            }
        }
        for (device, face) in &self.display_faces {
            if materializes_face(*device) {
                effect(face.effect_id)?;
            }
        }
        for profile in self.attachments.values() {
            for binding in &profile.bindings {
                template(&binding.template_id)?;
                if !profile.slots.iter().any(|slot| slot.id == binding.slot_id) {
                    return Err(error("attachment binding", "referenced slot is absent"));
                }
            }
        }
        for layout in self.layouts.values().chain(
            self.scenes
                .values()
                .flat_map(|scene| scene.zones.iter().map(|zone| &zone.layout)),
        ) {
            for output in &layout.zones {
                if let Some(attachment) = &output.attachment {
                    template(&attachment.template_id)?;
                }
            }
        }
        Ok(())
    }
    /// Validate the document graph and every file destination before restore.
    /// Missing physical devices are valid: presence belongs to local discovery.
    pub fn validate(&self) -> Result<(), PortableSetupError> {
        if self.schema_version != 1 {
            return Err(error(
                "schema_version",
                "unsupported portable setup version",
            ));
        }
        if self.setup_id.is_nil() {
            return Err(error("setup_id", "setup identity must not be nil"));
        }
        if self.name.trim().is_empty() {
            return Err(error("name", "setup name must not be empty"));
        }
        if self.output.canvas_width == 0 || self.output.canvas_height == 0 {
            return Err(error("output", "render dimensions must be nonzero"));
        }
        unit(self.output.brightness, "output.brightness")?;
        if !self.output.enabled && self.output.paused {
            return Err(error(
                "output",
                "stopped output cannot also be manually paused",
            ));
        }
        for id in self.devices.keys() {
            if id.is_nil() || self.logical_devices.contains_key(id) {
                return Err(error(
                    format!("devices.{id}"),
                    "device identities must be nonnil and distinct from logical devices",
                ));
            }
        }
        for (id, logical) in &self.logical_devices {
            if id.is_nil() {
                return Err(error(
                    format!("logical_devices.{id}"),
                    "logical identity must not be nil",
                ));
            }
            self.require_device(logical.device, &format!("logical_devices.{id}.device"))?;
            if logical.led_count == 0 || logical.led_start.checked_add(logical.led_count).is_none()
            {
                return Err(error(
                    format!("logical_devices.{id}"),
                    "invalid LED segment span",
                ));
            }
        }
        for (id, scene) in &self.scenes {
            if id.is_nil() && scene.kind != crate::scene::SceneKind::Ephemeral {
                return Err(error(
                    format!("scenes.{id}.kind"),
                    "the default identity requires the ephemeral scene kind",
                ));
            }
            if !id.is_nil() && scene.kind != crate::scene::SceneKind::Named {
                return Err(error(
                    format!("scenes.{id}.kind"),
                    "only named scenes have a portable persistence owner",
                ));
            }
            if let Some(layout_id) = &scene.layout_id
                && !self.layouts.contains_key(layout_id.as_str())
            {
                return Err(error(
                    format!("scenes.{id}.layout_id"),
                    "referenced layout is absent",
                ));
            }
            let mut zones = HashSet::new();
            for zone in &scene.zones {
                let path = format!("scenes.{id}.zones.{}", zone.id);
                if !zones.insert(zone.id) {
                    return Err(error(path, "duplicate zone identity"));
                }
                unit(zone.brightness, &format!("{path}.brightness"))?;
                self.validate_layout(&zone.layout, &path)?;
                if (zone.role == crate::scene::ZoneRole::Display) != zone.display_target.is_some() {
                    return Err(error(&path, "display role and display target must agree"));
                }
                if let Some(target) = &zone.display_target {
                    self.require_device(target.device, &path)?;
                    unit(target.opacity, &format!("{path}.display_target.opacity"))?;
                }
                let mut layers = HashSet::new();
                for layer in &zone.layers {
                    if !layers.insert(layer.id) {
                        return Err(error(&path, "duplicate layer identity"));
                    }
                    layer
                        .validate()
                        .map_err(|errors| error(&path, errors.join(", ")))?;
                    match &layer.source {
                        LayerSource::Media { asset_id, .. } if asset_id.0.is_nil() => {
                            return Err(error(&path, "referenced media identity must not be nil"));
                        }
                        LayerSource::Effect { controls, .. } => {
                            for value in controls.values() {
                                portable_control(value, &path)?;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        for (id, layout) in &self.layouts {
            if id != &layout.id {
                return Err(error(
                    format!("layouts.{id}"),
                    "map key does not match layout identity",
                ));
            }
            self.validate_layout(layout, &format!("layouts.{id}"))?;
        }
        for id in [
            self.selection.active_scene_id,
            self.selection.default_scene_id,
        ]
        .into_iter()
        .flatten()
        {
            if !self.scenes.contains_key(&id.0) {
                return Err(error("selection", "selected scene is absent"));
            }
        }
        for (id, preset) in &self.presets {
            if *id != preset.id.0 {
                return Err(error(
                    format!("presets.{id}"),
                    "map key does not match preset identity",
                ));
            }
            for value in preset.controls.values() {
                portable_control(value, "presets.controls")?;
            }
        }
        for (id, playlist) in &self.playlists {
            if *id != playlist.id.0 {
                return Err(error(
                    format!("playlists.{id}"),
                    "map key does not match playlist identity",
                ));
            }
        }
        for (id, template) in &self.attachment_templates {
            if id != &template.id {
                return Err(error(
                    format!("attachment_templates.{id}"),
                    "map key does not match template identity",
                ));
            }
        }
        for id in self
            .attachments
            .keys()
            .chain(self.device_output.keys())
            .chain(self.display_faces.keys())
        {
            self.require_device(*id, "device preference")?;
        }
        for output in self.device_output.values() {
            unit(output.brightness, "device_output.brightness")?;
        }
        for face in self.display_faces.values() {
            if face.effect_id.0.is_nil() {
                return Err(error("display_faces.effect_id", "effect identity is empty"));
            }
            unit(face.opacity, "display_faces.opacity")?;
            for value in face.controls.values() {
                portable_control(value, "display_faces.controls")?;
            }
        }
        self.blobs()?;
        Ok(())
    }

    /// Enumerate deduplicated blob requirements, rejecting conflicting lengths
    /// and bundle paths before a transport fetches or writes any bytes.
    pub fn blobs(&self) -> Result<BTreeMap<String, u64>, PortableSetupError> {
        let mut blobs = BTreeMap::new();
        let mut insert = |blob: &PortableBlob| -> Result<(), PortableSetupError> {
            blob.validate()?;
            if let Some(previous) = blobs.insert(blob.sha256.clone(), blob.byte_len)
                && previous != blob.byte_len
            {
                return Err(error("blobs", "one digest has conflicting byte lengths"));
            }
            Ok(())
        };
        for asset in self.assets.values() {
            insert(&asset.content)?;
        }
        for (id, effect) in &self.effects {
            if !effect.files.contains_key(&effect.entrypoint) {
                return Err(error(
                    format!("effects.{id}.entrypoint"),
                    "entrypoint is absent from effect files",
                ));
            }
            let mut paths = HashSet::new();
            for (path, blob) in &effect.files {
                let canonical = path.to_lowercase();
                let conflicts_with_directory = paths.iter().any(|existing: &String| {
                    canonical.starts_with(&format!("{existing}/"))
                        || existing.starts_with(&format!("{canonical}/"))
                });
                if !is_portable_bundle_path(path)
                    || path.rsplit('/').next().is_some_and(|name| {
                        name.eq_ignore_ascii_case("hypercolor-effect.json")
                            || name.eq_ignore_ascii_case(".hypercolor-effect-pending")
                    })
                    || conflicts_with_directory
                    || !paths.insert(canonical)
                {
                    return Err(error(
                        format!("effects.{id}.files.{path}"),
                        "invalid or case-colliding portable path",
                    ));
                }
                insert(blob)?;
            }
        }
        Ok(blobs)
    }

    fn require_device(&self, id: Uuid, path: &str) -> Result<(), PortableSetupError> {
        if self.devices.contains_key(&id) {
            Ok(())
        } else {
            Err(error(path, "physical device reference is absent"))
        }
    }

    fn validate_layout(
        &self,
        layout: &PortableLayout,
        path: &str,
    ) -> Result<(), PortableSetupError> {
        let mut ids = HashSet::new();
        for output in &layout.zones {
            if !ids.insert(&output.id) {
                return Err(error(path, "duplicate output identity"));
            }
            if !self.devices.contains_key(&output.device)
                && !self.logical_devices.contains_key(&output.device)
            {
                return Err(error(path, "output device reference is absent"));
            }
            if let Some(brightness) = output.brightness {
                unit(brightness, path)?;
            }
        }
        Ok(())
    }
}

fn unit(value: f32, path: &str) -> Result<(), PortableSetupError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(error(path, "expected a finite value from zero to one"))
    }
}

fn portable_control(value: &ControlValue, path: &str) -> Result<(), PortableSetupError> {
    value.validate().map_err(|e| error(path, e.to_string()))?;
    match value {
        ControlValue::SecretRef(_) => {
            return Err(error(
                path,
                "credential references stay on the installation",
            ));
        }
        ControlValue::List(values) => {
            for value in values {
                portable_control(value, path)?;
            }
        }
        ControlValue::Map(values) => {
            for value in values.values() {
                portable_control(value, path)?;
            }
        }
        _ => {}
    }
    Ok(())
}
