use std::collections::BTreeMap;
use std::path::Path;

use hypercolor_core::attachment::TemplateFilter;
use hypercolor_core::effect::{bundled_effects_root, html_effect_bundle};
use hypercolor_types::attachment::ComponentOrigin;
use hypercolor_types::effect::EffectSource;
use hypercolor_types::portable_setup::{
    PortableAsset, PortableBindingClass, PortableBlob, PortableDevice, PortableDeviceOutput,
    PortableDisplayFace, PortableEffect, PortableLayout, PortableLogicalDevice,
    PortableOutputState, PortableScene, PortableSelection, PortableSetupV1, SetupBindings,
    is_portable_bundle_path,
};

use super::{PortableSetupService, PortableSetupSnapshot, SetupIdentity, internal};
use crate::domain::DomainError;
use crate::domain::device_binding::CurrentBinding;
use crate::logical_devices::LogicalDeviceKind;
use crate::output_power::OutputOverride;

impl PortableSetupService {
    /// Project authored state through domain snapshots. The returned fingerprint
    /// is the expected-current fence for a later restore.
    pub async fn export(
        &self,
        identity: SetupIdentity,
        bindings: &SetupBindings,
    ) -> Result<PortableSetupSnapshot, DomainError> {
        let manifest = self.project_once(identity.clone(), bindings).await?;
        let observed = self.project_once(identity, bindings).await?;
        if manifest != observed {
            return Err(DomainError::conflict(
                "lighting changed while the setup was exported",
            ));
        }
        let bytes = serde_json::to_vec(&serde_json::to_value(&manifest).map_err(internal)?)
            .map_err(internal)?;
        let revision = hypercolor_core::asset::AssetLibrary::hash_bytes(&bytes);
        Ok(PortableSetupSnapshot { manifest, revision })
    }

    async fn project_once(
        &self,
        identity: SetupIdentity,
        bindings: &SetupBindings,
    ) -> Result<PortableSetupV1, DomainError> {
        let canvas = self.state.domains.layout.current();
        let power = self.state.output_power.snapshot();
        let mut manifest = PortableSetupV1::new(
            identity.setup_id,
            identity.name,
            PortableOutputState {
                canvas_width: canvas.canvas_width,
                canvas_height: canvas.canvas_height,
                brightness: power.global_brightness,
                enabled: power.output_override != OutputOverride::Stopped,
                paused: power.manually_paused(),
            },
        );
        let prior = self.prior_projection(identity.setup_id)?;
        let claims = self.state.device_registry.claims_snapshot().await;
        for device in self.state.device_registry.list().await {
            let Some(portable) = bindings.devices.get(&device.info.id).copied() else {
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
            manifest.devices.insert(
                portable,
                PortableDevice {
                    name: device.observed_info().name,
                    portable_key: claims.get(&device.info.id).map(|claim| claim.key().clone()),
                    driver_id: device.info.driver_id().to_owned(),
                    vendor_id,
                    product_id,
                    model: device.info.model.clone(),
                    led_count: device.info.total_led_count(),
                    binding_hint: hint,
                },
            );
            manifest.device_output.insert(
                portable,
                PortableDeviceOutput {
                    disabled: !device.user_settings.enabled,
                    brightness: device.user_settings.brightness,
                    rotation: device.user_settings.display_rotation,
                },
            );
        }
        for logical in self.state.logical_devices.read().await.values() {
            if logical.kind == LogicalDeviceKind::Default {
                continue;
            }
            let Some(id) = bindings.outputs.get(&logical.id) else {
                return Err(DomainError::conflict(
                    "logical device has no setup association",
                ));
            };
            let Some(physical) = bindings.devices.get(&logical.physical_device_id) else {
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
                PortableScene::project(scene, bindings).map_err(internal)?,
            );
        }
        manifest.selection = PortableSelection {
            active_scene_id: scenes.active_scene_id().copied(),
            default_scene_id: scenes
                .list()
                .iter()
                .find(|scene| scene.id.is_default())
                .map(|scene| scene.id),
        };
        let mut layouts = self.state.domains.layout.portable_snapshot().await;
        layouts.entry(canvas.id.clone()).or_insert(canvas);
        for (id, layout) in layouts {
            manifest.layouts.insert(
                id,
                PortableLayout::project(&layout, bindings).map_err(internal)?,
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
            let portable = bindings.outputs.get(&local).copied().or_else(|| {
                local
                    .parse()
                    .ok()
                    .and_then(|id| bindings.devices.get(&id).copied())
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
            if let Some(portable) = bindings.devices.get(&id) {
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
        let asset_sources = {
            let assets = self.state.asset_library.read().await;
            assets
                .records()
                .iter()
                .map(|asset| {
                    assets
                        .object_path_for_hash(&asset.hash_sha256)
                        .map(|path| (asset.clone(), path))
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(internal)?
        };
        let blobs = self.blobs.clone();
        manifest.assets = tokio::task::spawn_blocking(move || {
            asset_sources
                .into_iter()
                .map(|(asset, path)| {
                    let content = blobs.capture(&path)?;
                    if content.sha256 != asset.hash_sha256 || content.byte_len != asset.byte_len {
                        return Err(
                            hypercolor_core::portable_setup::SetupBlobError::ContentMismatch,
                        );
                    }
                    Ok((
                        asset.id.0,
                        PortableAsset {
                            name: asset.name,
                            mime_type: asset.mime_type,
                            tags: asset.tags,
                            content,
                        },
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
        })
        .await
        .map_err(internal)?
        .map_err(internal)?;
        let effect_sources = {
            let bundled = bundled_effects_root();
            self.state
                .domains
                .effects
                .portable_sources()
                .await
                .into_iter()
                .filter(|entry| {
                    matches!(entry.metadata.source, EffectSource::Html { .. })
                        && !entry.source_path.starts_with(&bundled)
                })
                .map(|entry| (entry.metadata.id.0, entry.metadata.name, entry.source_path))
                .collect::<Vec<_>>()
        };
        let blobs = self.blobs.clone();
        manifest.effects = tokio::task::spawn_blocking(move || {
            effect_sources
                .into_iter()
                .map(|(id, name, source)| {
                    let (root, entrypoint) = effect_resource_root(&source)?;
                    let mut files = BTreeMap::new();
                    capture_files(&root, &root, &blobs, &mut files)?;
                    Ok((
                        id,
                        PortableEffect {
                            name,
                            entrypoint,
                            files,
                        },
                    ))
                })
                .collect::<anyhow::Result<BTreeMap<_, _>>>()
        })
        .await
        .map_err(internal)?
        .map_err(internal)?;
        if let Some(prior) = prior {
            retain_unresolved(&mut manifest, &prior, bindings);
        }
        manifest
            .validate()
            .map_err(|error| DomainError::validation(error.to_string()))?;
        Ok(manifest)
    }
}

fn effect_resource_root(source: &Path) -> anyhow::Result<(std::path::PathBuf, String)> {
    if let Some(bundle) =
        html_effect_bundle(source).map_err(|error| anyhow::anyhow!(error.message))?
    {
        return Ok((bundle.root, bundle.manifest.entrypoint));
    }
    let root = source
        .parent()
        .ok_or_else(|| anyhow::anyhow!("effect has no bundle directory"))?;
    let entrypoint = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("effect entrypoint is not UTF-8"))?
        .to_owned();
    Ok((root.to_owned(), entrypoint))
}

fn capture_files(
    root: &Path,
    directory: &Path,
    blobs: &hypercolor_core::portable_setup::SetupBlobStore,
    files: &mut BTreeMap<String, PortableBlob>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "effect resources must not contain symlinks"
        );
        if kind.is_dir() {
            capture_files(root, &path, blobs, files)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("effect resource path is not UTF-8"))?
                .replace('\\', "/");
            if relative
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("hypercolor-effect.json"))
            {
                continue;
            }
            anyhow::ensure!(
                is_portable_bundle_path(&relative),
                "effect resource path is not portable: {relative}"
            );
            files.insert(relative, blobs.capture(&path)?);
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{capture_files, effect_resource_root};
    use hypercolor_core::effect::{HTML_EFFECT_BUNDLE_MANIFEST, HtmlEffectBundleManifest};
    use hypercolor_core::portable_setup::SetupBlobStore;
    use hypercolor_types::effect::EffectId;
    use std::collections::BTreeMap;

    #[test]
    fn nested_entrypoint_keeps_resources_at_bundle_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("effect");
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::create_dir_all(root.join("shared")).unwrap();
        std::fs::write(root.join("content/main.html"), "<html></html>").unwrap();
        std::fs::write(root.join("shared/script.js"), "const color = 'violet'").unwrap();
        let manifest = HtmlEffectBundleManifest {
            schema_version: 1,
            id: EffectId::new(uuid::Uuid::now_v7()),
            entrypoint: "content/main.html".into(),
        };
        std::fs::write(
            root.join(HTML_EFFECT_BUNDLE_MANIFEST),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let (actual_root, entrypoint) =
            effect_resource_root(&root.join("content/main.html")).unwrap();
        assert_eq!(actual_root, root.canonicalize().unwrap());
        assert_eq!(entrypoint, "content/main.html");
        let blobs = SetupBlobStore::open(directory.path().join("blobs")).unwrap();
        let mut files = BTreeMap::new();
        capture_files(&actual_root, &actual_root, &blobs, &mut files).unwrap();
        assert!(files.contains_key("content/main.html"));
        assert!(files.contains_key("shared/script.js"));
        assert!(!files.contains_key(HTML_EFFECT_BUNDLE_MANIFEST));
    }
}
