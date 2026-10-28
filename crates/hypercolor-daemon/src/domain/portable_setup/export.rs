use hypercolor_core::attachment::TemplateFilter;
use hypercolor_types::attachment::ComponentOrigin;
use hypercolor_types::portable_setup::{
    PortableBindingClass, PortableDevice, PortableDeviceOutput, PortableDisplayFace,
    PortableLayout, PortableLogicalDevice, PortableOutputState, PortableScene, PortableSelection,
    PortableSetupV1, SetupBindings,
};

use super::{PortableSetupService, PortableSetupSnapshot, RestoreReceipt, SetupIdentity, internal};
use crate::domain::DomainError;
use crate::domain::device_binding::CurrentBinding;
use crate::logical_devices::LogicalDeviceKind;
use crate::output_power::OutputOverride;

/// Cloud callers own their selected setup's name. Manual projections instead
/// retain the accepted native name, resolving it under the restore gate.
pub(super) enum ProjectionIdentity {
    Explicit(SetupIdentity),
    CurrentNative(SetupIdentity),
}

impl ProjectionIdentity {
    pub(super) fn setup_id(&self) -> uuid::Uuid {
        match self {
            Self::Explicit(identity) | Self::CurrentNative(identity) => identity.setup_id,
        }
    }

    pub(super) fn resolve(self, accepted: Option<&RestoreReceipt>) -> SetupIdentity {
        match self {
            Self::Explicit(identity) => identity,
            Self::CurrentNative(mut identity) => {
                if let Some(receipt) = accepted {
                    identity.name.clone_from(&receipt.applied.manifest.name);
                }
                identity
            }
        }
    }
}

impl PortableSetupService {
    /// Project authored state through domain snapshots. The returned fingerprint
    /// is the expected-current fence for a later restore.
    pub async fn export(
        &self,
        identity: SetupIdentity,
        bindings: &SetupBindings,
    ) -> Result<PortableSetupSnapshot, DomainError> {
        self.export_projection(ProjectionIdentity::Explicit(identity), bindings)
            .await
    }

    pub(super) async fn export_projection(
        &self,
        identity: ProjectionIdentity,
        bindings: &SetupBindings,
    ) -> Result<PortableSetupSnapshot, DomainError> {
        let _gate = self.restore_runtime.gate.lock().await;
        if self
            .restore_runtime
            .pending
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(DomainError::conflict("setup restoration remains pending"));
        }
        let accepted = self.active_projection_receipt(identity.setup_id(), bindings)?;
        let identity = identity.resolve(accepted.as_ref());
        if let Some(receipt) = accepted
            && receipt.applied.manifest.devices.keys().any(|id| {
                bindings.local_device(*id).is_some()
                    && (receipt.unresolved_bindings.contains(id)
                        || bindings.local_device(*id)
                            != receipt.materialized_bindings.local_device(*id))
            })
        {
            return Err(DomainError::conflict(
                "newly connected devices need saved setup reconciliation",
            ));
        }
        self.export_raw(identity, bindings).await
    }

    pub(super) async fn export_raw(
        &self,
        identity: SetupIdentity,
        bindings: &SetupBindings,
    ) -> Result<PortableSetupSnapshot, DomainError> {
        let manifest = self.project_once(identity.clone(), bindings, true).await?;
        let observed = self.project_once(identity, bindings, true).await?;
        if manifest != observed {
            return Err(DomainError::conflict(
                "lighting changed while the setup was exported",
            ));
        }
        let revision = super::fingerprint(&manifest)?;
        Ok(PortableSetupSnapshot { manifest, revision })
    }

    pub(super) async fn project_once(
        &self,
        identity: SetupIdentity,
        bindings: &SetupBindings,
        validate_projection: bool,
    ) -> Result<PortableSetupV1, DomainError> {
        let projection_bindings = self.projection_bindings(identity.setup_id, bindings)?;
        let canvas = self.state.domains.layout.current();
        let dimensions = self.state.config_manager.as_ref().map_or(
            (canvas.canvas_width, canvas.canvas_height),
            |manager| {
                let config = manager.get();
                (config.daemon.canvas_width, config.daemon.canvas_height)
            },
        );
        let power = self.state.output_power.snapshot();
        let mut manifest = PortableSetupV1::new(
            identity.setup_id,
            identity.name,
            PortableOutputState {
                canvas_width: dimensions.0,
                canvas_height: dimensions.1,
                brightness: power.global_brightness,
                enabled: power.output_override != OutputOverride::Stopped,
                paused: power.manually_paused(),
            },
        );
        let prior = match self.active_projection_receipt(identity.setup_id, bindings)? {
            Some(receipt) => Some(receipt.applied.manifest),
            None => self.prior_projection(identity.setup_id)?,
        };
        let claims = self.state.device_registry.claims_snapshot().await;
        for device in self.state.device_registry.list().await {
            let Some(portable) = projection_bindings.devices.get(&device.info.id).copied() else {
                continue;
            };
            let fingerprint = self
                .state
                .device_registry
                .fingerprint_for_id(&device.info.id)
                .await;
            let hint = fingerprint
                .as_ref()
                .and_then(|fingerprint| {
                    CurrentBinding::from_discovery(
                        &device.info,
                        fingerprint,
                        String::new(),
                        claims.contains_key(&device.info.id),
                    )
                })
                .map(|binding| binding.portable_hint());
            let (vendor_id, product_id) = match hint.as_ref().map(|hint| &hint.class) {
                Some(PortableBindingClass::ClaimlessUsb {
                    vendor_id,
                    product_id,
                    ..
                }) => (Some(*vendor_id), Some(*product_id)),
                _ => (None, None),
            };
            let accepted_device = if bindings.devices.contains_key(&device.info.id) {
                None
            } else {
                prior
                    .as_ref()
                    .and_then(|prior| prior.devices.get(&portable))
                    .cloned()
            };
            manifest.devices.insert(
                portable,
                accepted_device.unwrap_or(PortableDevice {
                    name: device.observed_info().name,
                    portable_key: claims.get(&device.info.id).map(|claim| claim.key().clone()),
                    driver_id: device.info.driver_id().to_owned(),
                    vendor_id,
                    product_id,
                    model: device.info.model.clone(),
                    led_count: device.info.total_led_count(),
                    binding_hint: hint,
                }),
            );
            let output = if bindings.devices.contains_key(&device.info.id) {
                None
            } else {
                prior
                    .as_ref()
                    .and_then(|prior| prior.device_output.get(&portable))
                    .cloned()
            };
            manifest.device_output.insert(
                portable,
                output.unwrap_or(PortableDeviceOutput {
                    disabled: !device.user_settings.enabled,
                    brightness: device.user_settings.brightness,
                    rotation: device.user_settings.display_rotation,
                }),
            );
        }
        for logical in self.state.logical_devices.read().await.values() {
            if logical.kind == LogicalDeviceKind::Default {
                continue;
            }
            let Some(id) = projection_bindings.outputs.get(&logical.id) else {
                return Err(DomainError::conflict(
                    "logical device has no setup association",
                ));
            };
            let Some(physical) = projection_bindings.devices.get(&logical.physical_device_id)
            else {
                continue;
            };
            manifest.logical_devices.insert(
                *id,
                PortableLogicalDevice {
                    device: *physical,
                    name: logical.name.clone(),
                    led_start: logical.led_start,
                    led_count: logical.led_count,
                    enabled: logical.enabled,
                },
            );
        }
        let scenes = self.state.scene_manager.snapshot().await;
        for scene in scenes.list() {
            manifest.scenes.insert(
                scene.id.0,
                PortableScene::project(scene, &projection_bindings).map_err(internal)?,
            );
        }
        manifest.selection = PortableSelection {
            active_scene_id: scenes.active_scene_id().copied(),
            default_scene_id: configured_start_scene(&self.state, &scenes)?,
        };
        drop(scenes);
        let mut layouts = self.state.domains.layout.portable_snapshot().await;
        layouts.entry(canvas.id.clone()).or_insert(canvas);
        for (id, layout) in layouts {
            manifest.layouts.insert(
                id,
                PortableLayout::project(&layout, &projection_bindings).map_err(internal)?,
            );
        }
        manifest.presets = self
            .state
            .library_store()
            .list_presets()
            .await
            .into_iter()
            .map(|preset| (preset.id.0, preset))
            .collect();
        manifest.playlists = self
            .state
            .library_store()
            .list_playlists()
            .await
            .into_iter()
            .map(|playlist| (playlist.id.0, playlist))
            .collect();
        for (local, profile) in self
            .state
            .attachment_profiles
            .read()
            .await
            .portable_snapshot()
        {
            let portable = projection_bindings
                .outputs
                .get(&local)
                .copied()
                .or_else(|| {
                    local
                        .parse()
                        .ok()
                        .and_then(|id| projection_bindings.devices.get(&id).copied())
                });
            if let Some(portable) = portable {
                manifest.attachments.insert(portable, (&profile).into());
            }
        }
        let templates = self.state.attachment_registry.read().await;
        for template in templates.list(&TemplateFilter {
            origin: Some(ComponentOrigin::User),
            ..Default::default()
        }) {
            manifest
                .attachment_templates
                .insert(template.id.clone(), template.clone());
        }
        drop(templates);
        for (id, face) in self.state.domains.display.preferences().read().await.iter() {
            if let Some(portable) = projection_bindings.devices.get(&id) {
                manifest.display_faces.insert(
                    *portable,
                    PortableDisplayFace {
                        effect_id: face.effect_id,
                        controls: face.controls.clone(),
                        blend_mode: face.blend_mode,
                        opacity: face.opacity,
                    },
                );
            }
        }
        self.authored_references().await.retain_definitions(
            &mut manifest,
            prior.as_ref(),
            &projection_bindings,
        );
        if let Some(prior) = prior {
            retain_unresolved(&mut manifest, &prior, bindings);
        }
        if validate_projection {
            manifest
                .validate()
                .map_err(|error| DomainError::validation(error.to_string()))?;
        }
        Ok(manifest)
    }
}

fn configured_start_scene(
    state: &crate::app_state::AppState,
    scenes: &hypercolor_core::scene::SceneManager,
) -> Result<Option<hypercolor_types::scene::SceneId>, DomainError> {
    use hypercolor_types::scene::SceneId;
    let Some(manager) = &state.config_manager else {
        return Ok(None);
    };
    let config = manager.get();
    let selector = config.daemon.start_scene.trim();
    if selector.eq_ignore_ascii_case("last") {
        return Ok(None);
    }
    if selector.is_empty() || selector.eq_ignore_ascii_case("default") {
        return Ok(Some(SceneId::DEFAULT));
    }
    let matches = scenes
        .list()
        .into_iter()
        .filter(|scene| {
            scene.name.eq_ignore_ascii_case(selector) || scene.id.to_string() == selector
        })
        .map(|scene| scene.id)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [id] => Ok(Some(*id)),
        _ => Err(DomainError::conflict(
            "configured startup scene has no unique portable identity",
        )),
    }
}

fn retain_unresolved(
    current: &mut PortableSetupV1,
    prior: &PortableSetupV1,
    bindings: &SetupBindings,
) {
    for (id, device) in &prior.devices {
        if bindings.local_device(*id).is_none() {
            current.devices.entry(*id).or_insert_with(|| device.clone());
            if let Some(value) = prior.device_output.get(id) {
                current
                    .device_output
                    .entry(*id)
                    .or_insert_with(|| value.clone());
            }
            if let Some(value) = prior.display_faces.get(id) {
                current
                    .display_faces
                    .entry(*id)
                    .or_insert_with(|| value.clone());
            }
            if let Some(value) = prior.attachments.get(id) {
                current
                    .attachments
                    .entry(*id)
                    .or_insert_with(|| value.clone());
            }
        }
    }
    for (id, logical) in &prior.logical_devices {
        if bindings.local_device(logical.device).is_none() {
            current
                .logical_devices
                .entry(*id)
                .or_insert_with(|| logical.clone());
        }
    }
    for (id, scene) in &mut current.scenes {
        let Some(previous) = prior.scenes.get(id) else {
            continue;
        };
        for (index, old) in previous.zones.iter().enumerate() {
            if old
                .display_target
                .as_ref()
                .is_some_and(|target| bindings.local_device(target.device).is_none())
                && !scene.zones.iter().any(|zone| zone.id == old.id)
            {
                scene
                    .zones
                    .insert(index.min(scene.zones.len()), old.clone());
            }
        }
        for zone in &mut scene.zones {
            if zone.display_target.is_none()
                && let Some(target) = previous
                    .zones
                    .iter()
                    .find(|old| old.id == zone.id)
                    .and_then(|old| old.display_target.as_ref())
                && bindings.local_device(target.device).is_none()
            {
                zone.display_target = Some(target.clone());
            }
        }
    }
}
