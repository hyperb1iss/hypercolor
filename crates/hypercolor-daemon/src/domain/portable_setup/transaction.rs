//! One owned transaction keeps every normal writer fenced until its receipt.

use hypercolor_core::config::PreparedConfigMutation;
use hypercolor_types::portable_setup::{PortableReferenceCatalog, PortableSetupV1, SetupBindings};
use std::sync::{Arc, atomic::Ordering};

use super::domains::{library_snapshot, recovery_map};
use super::journal::{BackupAncillary, RestoreJournal, RestoreRecovery, RuntimeRestoreProof};
use super::{
    PortableSetupService, PortableSetupSnapshot, RestoreOptions, RestoreReceipt, RestoreReport,
    SetupIdentity, fingerprint, internal,
};
use crate::domain::DomainError;
use crate::persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};

impl PortableSetupService {
    pub(super) async fn apply_candidate(
        &self,
        before: PortableSetupV1,
        mut candidate: PortableSetupV1,
        requested_revision: String,
        bindings: SetupBindings,
        options: RestoreOptions,
        shutdown: &PersistenceShutdown,
        recovery: Option<RestoreRecovery>,
        mut ancillary: Option<BackupAncillary>,
    ) -> Result<RestoreReport, DomainError> {
        candidate
            .validate()
            .map_err(|error| DomainError::validation(error.to_string()))?;
        let recovering = recovery.is_some();
        if let Some(proof) = &recovery {
            let proof = &proof.runtime;
            let persisted =
                crate::runtime_state::load(&self.state.runtime_state_path).map_err(internal)?;
            if persisted != proof.before && persisted.as_ref() != Some(&proof.candidate) {
                return Err(DomainError::conflict(
                    "runtime session changed outside interrupted setup restoration",
                ));
            }
        }
        let source = if recovering {
            let mut current = self
                .project_once(
                    SetupIdentity {
                        setup_id: before.setup_id,
                        name: before.name.clone(),
                    },
                    &bindings,
                    false,
                )
                .await?;
            macro_rules! verify {
                ($($field:ident),+) => {$({ recovery_map(stringify!($field), &current.$field, &before.$field, &candidate.$field)?; })+};
            }
            verify!(
                layouts,
                presets,
                playlists,
                logical_devices,
                display_faces,
                attachments
            );
            macro_rules! verify_output {
                ($($field:ident),+) => {$({
                    recovery_value(concat!("output.", stringify!($field)),
                        &current.output.$field, &before.output.$field, &candidate.output.$field)?;
                })+};
            }
            verify_output!(brightness, canvas_width, canvas_height);
            if let Some(recovery) = &recovery {
                for (id, actual) in &current.device_output {
                    let Some(local) = bindings.local_device(*id) else {
                        continue;
                    };
                    let default = hypercolor_types::portable_setup::PortableDeviceOutput {
                        disabled: false,
                        brightness: 1.0,
                        rotation: hypercolor_types::scene::DisplayRotation::default(),
                    };
                    let expected = if recovery.bindings.local_device(*id) == Some(local) {
                        before.device_output.get(id).unwrap_or(&default)
                    } else {
                        &default
                    };
                    recovery_value(
                        &format!("device_output.{id}"),
                        actual,
                        expected,
                        candidate.device_output.get(id).unwrap_or(&default),
                    )?;
                }
            }
            recovery_value(
                "selection.default_scene_id",
                &current.selection.default_scene_id,
                &before.selection.default_scene_id,
                &candidate.selection.default_scene_id,
            )?;
            let named = |scenes: &std::collections::BTreeMap<
                uuid::Uuid,
                hypercolor_types::portable_setup::PortableScene,
            >| {
                scenes
                    .iter()
                    .filter(|(id, _)| !id.is_nil())
                    .map(|(id, scene)| {
                        let mut scene = scene.clone();
                        // Offline display definitions have no native row. Their
                        // exact authoring remains in the durable intent; compare
                        // only the materialized portion against partial stores.
                        scene.zones.retain(|zone| {
                            zone.display_target
                                .as_ref()
                                .is_none_or(|target| bindings.local_device(target.device).is_some())
                        });
                        (*id, scene)
                    })
                    .collect()
            };
            recovery_map(
                "scenes",
                &named(&current.scenes),
                &named(&before.scenes),
                &named(&candidate.scenes),
            )?;
            // These participants validate file-by-file progress against their
            // operation-owned plans, including deliberate retirement gaps.
            current.effects.clone_from(&before.effects);
            current.assets.clone_from(&before.assets);
            current
                .attachment_templates
                .clone_from(&before.attachment_templates);
            current
        } else {
            before.clone()
        };
        let config = self.state.config_manager.clone();
        let config_source = config.as_ref().map(|manager| Arc::clone(&manager.get()));
        if let Some(config) = &config_source
            && (config.daemon.canvas_width != source.output.canvas_width
                || config.daemon.canvas_height != source.output.canvas_height
                || !configured_selection_matches(config, &source))
        {
            return Err(DomainError::conflict(
                "configured output changed before setup restoration",
            ));
        }
        if let Some(choices) = &mut ancillary
            && let Some(preferences) = &choices.preferences
        {
            let current = config_source.as_deref().ok_or_else(|| {
                DomainError::validation("lighting preferences require a configuration store")
            })?;
            let actual =
                hypercolor_types::portable_setup::PortablePreferences::from_config(current);
            if !recovering && choices.expected_preferences.is_none() {
                choices.expected_preferences = Some(actual.clone());
            }
            let expected = choices.expected_preferences.as_ref().ok_or_else(|| {
                DomainError::conflict("backup recovery lacks its preference authority")
            })?;
            let mut desired = current.clone();
            expected
                .apply_to_config(&mut desired)
                .map_err(|error| DomainError::validation(error.to_string()))?;
            preferences
                .apply_to_config(&mut desired)
                .map_err(|error| DomainError::validation(error.to_string()))?;
            let desired =
                hypercolor_types::portable_setup::PortablePreferences::from_config(&desired);
            if &actual != expected && !(recovering && actual == desired) {
                return Err(DomainError::conflict(
                    "lighting preferences changed outside backup restoration",
                ));
            }
        }
        let mut config_candidate = config_source.as_deref().cloned();
        if let Some(preferences) = ancillary
            .as_ref()
            .and_then(|choices| choices.preferences.as_ref())
        {
            let config = config_candidate.as_mut().ok_or_else(|| {
                DomainError::validation("lighting preferences require a configuration store")
            })?;
            preferences
                .apply_to_config(config)
                .map_err(|error| DomainError::validation(error.to_string()))?;
        }
        if let Some(config) = &mut config_candidate {
            config.daemon.canvas_width = candidate.output.canvas_width;
            config.daemon.canvas_height = candidate.output.canvas_height;
            if options.apply_selection {
                config.daemon.start_scene = candidate
                    .selection
                    .default_scene_id
                    .map_or_else(|| "last".to_owned(), |id| id.to_string());
            }
        }
        let mut tui = None;
        if let Some(choices) = &mut ancillary
            && let Some(desired) = choices.tui_preferences.clone()
        {
            let store = self.tui_preference_store();
            let expected = choices.expected_tui_preferences.clone();
            let shutdown = shutdown.clone();
            let (prepared, expected) = tokio::task::spawn_blocking(move || {
                let expected = match expected {
                    Some(value) => value,
                    None if !recovering => store
                        .load_while(|| !shutdown.is_shutdown())
                        .map_err(internal)?,
                    None => {
                        return Err(DomainError::conflict(
                            "backup recovery lacks terminal preference authority",
                        ));
                    }
                };
                let prepared = store
                    .prepare(&expected, &desired, recovering, &shutdown)
                    .map_err(|error| {
                        if error.is::<hypercolor_core::client_preferences::TuiPreferenceConflict>()
                        {
                            DomainError::conflict(error.to_string())
                        } else {
                            internal(error)
                        }
                    })?;
                Ok((prepared, expected))
            })
            .await
            .map_err(internal)??;
            choices.expected_tui_preferences = Some(expected);
            tui = Some(prepared);
        }
        let device_admission = self.state.device_settings.begin_portable_restore().await;
        let devices = self
            .prepare_device_sources(&source, &candidate, &bindings, recovering)
            .await?;
        // Ordinary power changes hold this admission through runtime-session
        // persistence and renderer publication. Preserve that lock order.
        let power = self
            .state
            .output_power
            .prepare_portable_output(&source.output, candidate.output.clone())
            .await?;
        let barrier = self.state.domains.layout.begin_portable_restore().await?;
        let staging = self
            .root
            .join("staging")
            .join(options.operation_id.to_string());
        let mut effects = self
            .state
            .domains
            .effects
            .prepare_portable_effects(crate::domain::effect::portable::PortableEffectsRestore {
                before: &before.effects,
                desired: &candidate.effects,
                blobs: &self.blobs,
                managed_root: &self.state.data_dir.join("effects/user"),
                staging_root: &staging,
                operation_id: options.operation_id,
            })
            .await?;
        let mut templates = if recovering {
            super::templates::prepare_templates_recovery(
                Arc::clone(&self.state.attachment_registry),
                &before.attachment_templates,
                &candidate.attachment_templates,
                &self.state.data_dir.join("attachments"),
                options.operation_id,
                &staging,
            )
            .await?
        } else {
            super::templates::prepare_templates(
                Arc::clone(&self.state.attachment_registry),
                &before.attachment_templates,
                &candidate.attachment_templates,
                &self.state.data_dir.join("attachments"),
                options.operation_id,
                &staging,
            )
            .await?
        };
        let mut catalog = PortableReferenceCatalog::default();
        for (id, entry) in effects.candidate().iter() {
            catalog.effects.insert(*id);
            for preset in &entry.metadata.presets {
                catalog.presets.insert(preset.id, *id);
            }
        }
        for template in templates
            .candidate_registry()
            .list(&hypercolor_core::attachment::TemplateFilter::default())
        {
            catalog.attachment_templates.insert(template.id.clone());
        }
        candidate
            .validate_with_catalog(&catalog)
            .map_err(|error| DomainError::validation(error.to_string()))?;
        let dimensions = (
            candidate.output.canvas_width,
            candidate.output.canvas_height,
        );
        let projection_bindings = self.projection_bindings(candidate.setup_id, &bindings)?;
        let mut layouts = self
            .state
            .domains
            .layout
            .prepare_portable_replace(
                barrier.guard(),
                &source.layouts,
                &candidate.layouts,
                &bindings,
                &projection_bindings,
                dimensions,
            )
            .await?;
        let runtime_admission = self
            .state
            .domains
            .runtime_session
            .begin_portable_restore()
            .await;
        let mut scenes = self
            .state
            .scene_manager
            .prepare_portable_replace(
                &source.scenes,
                &candidate.scenes,
                &bindings,
                &projection_bindings,
                dimensions,
                &devices.infos,
            )
            .await?;
        let mut faces =
            crate::display_preferences::DisplayPreferencesStore::prepare_portable_replace(
                Arc::clone(self.state.domains.display.preferences()),
                &source.display_faces,
                &candidate.display_faces,
                &bindings,
            )
            .await?;
        scenes.prepare_runtime(
            source.selection.active_scene_id,
            if options.apply_selection {
                crate::domain::scene::portable::PortableSceneSelection::Apply(
                    candidate.selection.active_scene_id,
                )
            } else {
                crate::domain::scene::portable::PortableSceneSelection::Preserve
            },
            faces.candidate(),
            &devices.infos,
        )?;
        let library_store = self.state.library_store();
        let mut library = match &ancillary {
            Some(choices) => {
                library_store
                    .prepare_portable_backup_replace(
                        &library_snapshot(&source),
                        library_snapshot(&candidate),
                        &choices.favorites,
                    )
                    .await
            }
            None => {
                library_store
                    .prepare_portable_replace(
                        &library_snapshot(&source),
                        library_snapshot(&candidate),
                    )
                    .await
            }
        }
        .map_err(internal)?;
        let mut media = if recovering {
            super::media::prepare_media_recovery(
                Arc::clone(&self.state.asset_library),
                &before.assets,
                &candidate.assets,
                &self.blobs,
                options.operation_id,
            )
            .await?
        } else {
            super::media::prepare_media(
                Arc::clone(&self.state.asset_library),
                &before.assets,
                &candidate.assets,
                &self.blobs,
                options.operation_id,
            )
            .await?
        };
        let mut attachments = self
            .prepare_attachments(
                &source,
                &candidate,
                &bindings,
                &devices.infos,
                templates.candidate_registry(),
                recovering,
            )
            .await?;
        for (local, portable) in &bindings.devices {
            if let Some(profile) = attachments.candidate().get(&local.to_string()) {
                candidate.attachments.insert(*portable, profile.into());
            }
        }
        let mut logical = crate::logical_devices::portable::prepare_portable_replace(
            Arc::clone(&self.state.logical_devices),
            &self.state.logical_devices_path,
            &source.logical_devices,
            &candidate.logical_devices,
            &bindings,
        )
        .await?;
        let desired_device_outputs = devices.live_after.clone();
        let mut registry = self
            .state
            .device_registry
            .prepare_output_settings(&devices.live_before, devices.live_after)
            .await
            .map_err(internal)?;
        let mut settings = self
            .state
            .device_settings
            .prepare_portable_output(
                &devices.stored_before,
                &devices.stored_after,
                Some((source.output.brightness, candidate.output.brightness)),
            )
            .await?;
        let mut runtime_snapshot =
            crate::runtime_state::snapshot_from_scene_manager(scenes.candidate());
        let mut active_layout = scenes
            .candidate()
            .active_scene()
            .and_then(|scene| {
                scene
                    .zones
                    .iter()
                    .find(|zone| zone.display_target.is_none())
            })
            .map(|zone| zone.layout.clone())
            .or_else(|| {
                let current_id = self.state.domains.layout.current().id;
                candidate
                    .layouts
                    .get(&current_id)
                    .or_else(|| candidate.layouts.get("default"))
                    .or_else(|| candidate.layouts.values().next())
                    .map(|layout| layout.materialize(&bindings, dimensions))
            })
            .ok_or_else(|| DomainError::validation("restored setup has no active canvas layout"))?;
        active_layout.canvas_width = dimensions.0;
        active_layout.canvas_height = dimensions.1;
        runtime_snapshot.active_layout_id = Some(active_layout.id.clone());
        runtime_snapshot.manual_paused = candidate.output.paused;
        runtime_snapshot.output_stopped = !candidate.output.enabled;
        let face_fps_cap = config_source.as_ref().map_or_else(
            || hypercolor_types::config::DisplayConfig::default().effective_face_fps_cap(),
            |config| config.display.effective_face_fps_cap(),
        );
        let mut renderer = self.state.domains.layout.prepare_portable_renderer(
            &barrier,
            active_layout,
            scenes.candidate(),
            effects.candidate(),
            &devices.infos,
            face_fps_cap,
        )?;
        let persisted_runtime =
            crate::runtime_state::load(&self.state.runtime_state_path).map_err(internal)?;
        if let Some(proof) = &recovery
            && persisted_runtime != proof.runtime.before
            && persisted_runtime.as_ref() != Some(&proof.runtime.candidate)
        {
            return Err(DomainError::conflict(
                "runtime session changed before recovery acquired its writer",
            ));
        }
        // Startup may regenerate Default-zone identities before a runtime file
        // exists. The persisted transition, not that derived view, owns recovery.
        // Advance the proof under the writer gate for another interrupted retry.
        let runtime_proof = RuntimeRestoreProof {
            before: persisted_runtime,
            candidate: runtime_snapshot.clone(),
        };
        let mut runtime = runtime_admission.prepare(&runtime_snapshot)?;
        let unresolved_bindings = candidate
            .devices
            .keys()
            .filter(|id| bindings.local_device(**id).is_none())
            .copied()
            .collect::<Vec<_>>();
        let receipt = RestoreReceipt {
            operation_id: options.operation_id,
            setup_id: candidate.setup_id,
            requested_revision: requested_revision.clone(),
            applied: PortableSetupSnapshot {
                revision: fingerprint(&candidate)?,
                manifest: candidate.clone(),
            },
            unresolved_bindings: unresolved_bindings.clone(),
            materialized_bindings: bindings.clone(),
        };
        let committed_ancillary = ancillary.clone();
        let intent = RestoreJournal::Pending {
            schema_version: 1,
            before: Box::new(before),
            candidate: Box::new(candidate.clone()),
            requested_revision,
            bindings,
            options: options.clone(),
            runtime: runtime_proof,
            ancillary,
        };
        let service = self.clone();
        let finish_shutdown = shutdown.clone();
        let shutdown = shutdown.clone();
        let runtime_handle = tokio::runtime::Handle::current();
        let report = tokio::task::spawn_blocking(move || {
            let _device_admission = device_admission;
            let mut commit = |mut config: Option<&mut PreparedConfigMutation<'_>>| -> Result<RestoreReport,DomainError> {
                service.restore_runtime.pending.store(true, Ordering::Release);
                retry_step(&shutdown, || service.write_journal(candidate.setup_id, &intent, &shutdown))?;
                retry_step(&shutdown, || effects.persist(&shutdown))?;
                retry_step(&shutdown, || templates.persist(&shutdown))?;
                retry_step(&shutdown, || media.persist(&shutdown))?;
                written(library.persist(&shutdown))?;
                written(attachments.persist(&shutdown))?;
                written(logical.persist(&shutdown))?;
                written(faces.persist(&shutdown))?;
                written(settings.persist(&shutdown))?;
                written(layouts.persist(&shutdown))?;
                written(scenes.persist(&shutdown))?;
                written(runtime.persist(&shutdown))?;
                if let Some(tui) = tui.as_mut() {
                    retry_step(&shutdown, || tui.persist(&shutdown).map_err(internal))?;
                }
                if let Some(config) = config.as_mut() { written(config.persist(&shutdown))?; }
                retry_step(&shutdown, || runtime_handle.block_on(renderer.activate(&barrier)).map(|()| AtomicWriteWaitOutcome::Written))?;
                effects.publish();
                templates.publish();
                media.publish();
                library.publish();
                attachments.publish();
                logical.publish();
                faces.publish();
                settings.publish();
                registry.publish();
                layouts.publish();
                scenes.publish();
                renderer.publish();
                power.publish(&service.state.event_bus);
                runtime_handle.block_on(async {
                    let mut render_loop = service.state.render_loop.write().await;
                    if service.state.output_power.snapshot().sleeping() { render_loop.pause(); }
                    else { render_loop.resume(); }
                    let mut backends = service.state.backend_manager.lock().await;
                    for (id, desired) in &desired_device_outputs {
                        backends.set_device_output_brightness(*id, desired.brightness);
                    }
                });
                if let Some(config) = config.as_mut() { config.publish(); }
                retry_step(&shutdown, || service.write_journal(candidate.setup_id, &RestoreJournal::Committed { schema_version:1, receipt: Box::new(receipt.clone()), finalized:false, ancillary: committed_ancillary.clone() }, &shutdown))?;
                Ok(RestoreReport { applied: receipt.applied.clone(), unresolved_bindings: unresolved_bindings.clone() })
            };
            match (config, config_source, config_candidate) {
                (Some(manager), Some(source), Some(desired)) => manager.with_prepared_mutation(&source, |config| {
                    *config = desired;
                }, |prepared| Ok(commit(Some(prepared))))
                .map_err(internal)?.ok_or_else(|| DomainError::conflict("configuration changed before setup restoration"))?,
                _ => commit(None),
            }
        }).await.map_err(internal)??;
        self.finish_committed(report.applied.manifest.setup_id, &finish_shutdown)
            .await?;
        Ok(report)
    }
}

fn configured_selection_matches(
    config: &hypercolor_types::config::HypercolorConfig,
    source: &PortableSetupV1,
) -> bool {
    let selector = config.daemon.start_scene.trim();
    match source.selection.default_scene_id {
        None => selector.eq_ignore_ascii_case("last"),
        Some(id)
            if id.is_default()
                && (selector.is_empty() || selector.eq_ignore_ascii_case("default")) =>
        {
            true
        }
        Some(id) => {
            selector == id.to_string()
                || source
                    .scenes
                    .get(&id.0)
                    .is_some_and(|scene| scene.name.eq_ignore_ascii_case(selector))
        }
    }
}

fn recovery_value<T: PartialEq>(
    path: &str,
    current: &T,
    before: &T,
    desired: &T,
) -> Result<(), DomainError> {
    if current != before && current != desired {
        return Err(DomainError::conflict(format!(
            "local data at {path} diverged from interrupted setup restoration"
        )));
    }
    Ok(())
}

pub(super) fn written(outcome: AtomicWriteWaitOutcome) -> Result<(), DomainError> {
    match outcome {
        AtomicWriteWaitOutcome::Written => Ok(()),
        AtomicWriteWaitOutcome::ShutdownPending => Err(DomainError::conflict(
            "setup restoration is saved for the next start",
        )),
        AtomicWriteWaitOutcome::Superseded => Err(DomainError::conflict(
            "setup restoration lost its persistence authority",
        )),
    }
}

fn retry_step(
    shutdown: &PersistenceShutdown,
    mut step: impl FnMut() -> Result<AtomicWriteWaitOutcome, DomainError>,
) -> Result<(), DomainError> {
    let mut reported = false;
    loop {
        if shutdown.is_shutdown() {
            return written(AtomicWriteWaitOutcome::ShutdownPending);
        }
        match step() {
            Ok(outcome) => return written(outcome),
            Err(error) => {
                if !reported {
                    tracing::error!(%error, "Portable restore retains its writers until persistence recovers");
                    reported = true;
                }
                std::thread::park_timeout(std::time::Duration::from_millis(100));
            }
        }
    }
}
