use super::super::{
    InstallPlatform, InstallPlatformError, InstallationState, PlatformCheckpoint,
    PlatformOwnerReceipt, PlatformState, PlatformTransactionRecord, PlatformTransitionStates,
    PreparedPlatformTransaction, UnitId, UnitRecord,
};
use super::effects::{
    candidate_launcher_entry, directory_create_effect, launcher_effect, layout_direction,
    layout_effect, require_exact_entry, validate_layout_unit,
};
use super::executor::LinuxInstallExecutor;
use super::model::{
    LINUX_DIRECTORY_ITEMS, LINUX_LAYOUT_ITEMS, LINUX_RECEIPT_SCHEMA_VERSION,
    LINUX_RECORD_SCHEMA_VERSION, LinuxDirectoryState, LinuxExactEntry, LinuxFilePublication,
    LinuxLayoutEffect, LinuxLayoutOperation, LinuxRecord, MAX_HTTP_RESPONSE_BYTES,
    MAX_LAUNCHER_BYTES, error,
};
use super::proof::{candidate_systemd, prior_systemd, require_notify_launcher, systemd_equivalent};
use super::state::consistent_platform_unit;
use super::systemd::canonical_executable;
use super::{LinuxInspection, LinuxInstallPlatform};

impl<E: LinuxInstallExecutor> LinuxInstallPlatform<E> {
    fn expected_layout_matches(
        inspection: &LinuxInspection,
        record: &LinuxRecord,
        index: u16,
        checkpoint: PlatformCheckpoint,
    ) -> bool {
        record
            .layout
            .iter()
            .enumerate()
            .all(|(operation_index, operation)| match &operation.effect {
                LinuxLayoutEffect::Directory { item } => {
                    if matches!(
                        checkpoint,
                        PlatformCheckpoint::PriorOriginal
                            | PlatformCheckpoint::PriorActiveRestored
                            | PlatformCheckpoint::PriorLauncherRestored
                            | PlatformCheckpoint::PriorLayoutRestored
                            | PlatformCheckpoint::PriorManager
                            | PlatformCheckpoint::PriorAutostart
                            | PlatformCheckpoint::PriorRestored
                    ) {
                        return inspection.directories.get(item).is_some_and(|actual| {
                            matches!(
                                actual,
                                LinuxDirectoryState::Absent | LinuxDirectoryState::Present
                            )
                        });
                    }
                    let expected = if operation_index < usize::from(index) {
                        LinuxDirectoryState::Present
                    } else {
                        LinuxDirectoryState::Absent
                    };
                    inspection.directories.get(item) == Some(&expected)
                }
                LinuxLayoutEffect::Entry {
                    item,
                    prior,
                    candidate_target,
                } => {
                    let expected = if operation_index < usize::from(index) {
                        LinuxExactEntry::Symlink {
                            target: candidate_target.clone(),
                        }
                    } else {
                        prior.clone()
                    };
                    inspection
                        .layout
                        .get(item)
                        .is_some_and(|actual| super::model::entries_match(actual, &expected))
                }
            })
    }
}

impl<E: LinuxInstallExecutor> InstallPlatform for LinuxInstallPlatform<E> {
    fn inspect(&mut self) -> Result<PlatformState, InstallPlatformError> {
        let inspection = self.inspect_exact()?;
        let state = self.state_from(&inspection)?;
        self.last_inspection = Some(inspection);
        Ok(state)
    }

    fn prepare_transaction(
        &mut self,
        candidate: &UnitRecord,
        prior: &InstallationState,
        target: &PlatformState,
    ) -> Result<PreparedPlatformTransaction, InstallPlatformError> {
        let inspection = self
            .last_inspection
            .clone()
            .ok_or_else(|| error("prepare requires an exact prior inspection"))?;
        if self.state_from(&inspection)? != prior.platform {
            return Err(error("prior Linux inspection changed before preparation"));
        }
        if inspection.systemd.load_state == "loaded"
            && matches!(inspection.launcher, LinuxExactEntry::Absent)
        {
            return Err(error(
                "loaded direct service lacks restorable launcher metadata",
            ));
        }
        if !self
            .known_units
            .iter()
            .any(|known| known.id() == candidate.id())
        {
            self.executor.validate_unit_authority(candidate)?;
            self.known_units.push(candidate.clone());
        }
        let candidate_binding = self.unit_binding(candidate)?;
        let prior_platform_unit = consistent_platform_unit(&prior.platform)?;
        let synthetic_prior = if prior.active_unit.is_none()
            && prior_platform_unit
                .as_ref()
                .is_some_and(|unit| unit.as_str().starts_with("legacy-"))
        {
            let version = if prior.platform.running_unit.is_some() {
                super::proof::read_health_version(
                    self.executor.http_get("/health", MAX_HTTP_RESPONSE_BYTES)?,
                )?
            } else {
                "legacy-inactive".to_owned()
            };
            let unit = prior_platform_unit
                .as_ref()
                .expect("checked synthetic legacy unit")
                .clone();
            let launcher = match inspection.launcher {
                LinuxExactEntry::RegularFile { mode, .. } => Some(LinuxFilePublication {
                    mode,
                    contents: inspection.launcher_bytes.clone(),
                }),
                LinuxExactEntry::Absent => None,
                LinuxExactEntry::Symlink { .. } => {
                    return Err(error("first conversion launcher is not a regular file"));
                }
            };
            let snapshot = super::model::LinuxLegacySnapshot {
                unit,
                version,
                launcher,
                layout: inspection
                    .layout
                    .iter()
                    .map(|(item, entry)| (*item, entry.clone()))
                    .collect(),
                inventory: inspection.legacy_inventory.clone(),
            };
            Some(self.executor.snapshot_legacy_unit(&snapshot)?)
        } else {
            None
        };
        if let Some(unit) = &synthetic_prior {
            self.executor.validate_unit_authority(unit)?;
            self.legacy_unit = Some(unit.id().clone());
            self.known_units.push(unit.clone());
        }
        let mut prior_binding = prior_platform_unit
            .as_ref()
            .map(|unit| {
                self.known_units
                    .iter()
                    .find(|known| known.id() == unit)
                    .ok_or_else(|| error("loaded direct service lacks a retained synthetic legacy or immutable prior UnitRecord"))
                    .and_then(|record| self.unit_binding(record))
            })
            .transpose()?;
        if prior
            .platform
            .running_unit
            .as_ref()
            .is_some_and(|unit| unit.as_str().starts_with("legacy-"))
        {
            let exec_start = require_notify_launcher(&inspection.launcher_bytes)?;
            let executable = canonical_executable(&exec_start)?;
            super::model::require_absolute(&executable, "legacy ExecStart executable")?;
            executable.clone_into(
                &mut prior_binding
                    .as_mut()
                    .ok_or_else(|| error("running legacy owner lacks a synthetic binding"))?
                    .daemon_path,
            );
        }
        if prior.platform.loaded && prior_binding.is_none() {
            return Err(error(
                "loaded direct service lacks restorable immutable launcher metadata",
            ));
        }
        let candidate_launcher = target
            .launcher_unit
            .as_ref()
            .map(|_| self.candidate_launcher())
            .transpose()?;
        let mut layout = LINUX_DIRECTORY_ITEMS
            .into_iter()
            .filter(|item| inspection.directories[item] == LinuxDirectoryState::Absent)
            .map(|item| LinuxLayoutOperation {
                effect: LinuxLayoutEffect::Directory { item },
            })
            .collect::<Vec<_>>();
        layout.extend(LINUX_LAYOUT_ITEMS.into_iter().map(|item| {
            LinuxLayoutOperation {
                effect: LinuxLayoutEffect::Entry {
                    item,
                    prior: match &inspection.layout[&item] {
                        LinuxExactEntry::RegularFile { mode, sha256, .. } => {
                            LinuxExactEntry::RegularFile {
                                mode: *mode,
                                sha256: sha256.clone(),
                                snapshot_unit: synthetic_prior
                                    .as_ref()
                                    .map(|unit| unit.id().clone()),
                                snapshot_path: synthetic_prior
                                    .as_ref()
                                    .map(|_| item.unit_path().to_owned()),
                            }
                        }
                        entry => entry.clone(),
                    },
                    candidate_target: self.layout_target(candidate.id(), item),
                },
            }
        }));
        let layout_operation_count = u16::try_from(layout.len()).expect("bounded layout count");
        let record = LinuxRecord {
            candidate: candidate_binding,
            prior: prior_binding,
            baseline_systemd: inspection.systemd,
            prior_launcher: inspection.launcher,
            prior_launcher_bytes: inspection.launcher_bytes,
            candidate_launcher,
            prior_directories: inspection.directories,
            layout,
            first_conversion: synthetic_prior.is_some(),
        };
        let payload = serde_json::to_vec(&record)
            .map_err(|source| error(format!("encode Linux platform record: {source}")))?;
        let record = PlatformTransactionRecord::linux(LINUX_RECORD_SCHEMA_VERSION, payload)
            .map_err(|source| error(source.to_string()))?;
        let prior_unloaded = PlatformState {
            loaded: false,
            running_unit: None,
            ..prior.platform.clone()
        };
        let candidate_manager = PlatformState {
            loaded: false,
            running_unit: None,
            autostart_enabled: prior.platform.autostart_enabled,
            ..target.clone()
        };
        let candidate_autostart = PlatformState {
            autostart_enabled: target.autostart_enabled,
            ..candidate_manager.clone()
        };
        let prior_manager = PlatformState {
            loaded: false,
            running_unit: None,
            autostart_enabled: prior.platform.autostart_enabled,
            ..prior.platform.clone()
        };
        let prior_autostart = PlatformState {
            autostart_enabled: prior.platform.autostart_enabled,
            ..prior_manager.clone()
        };
        Ok(PreparedPlatformTransaction {
            record,
            transitions: PlatformTransitionStates {
                prior_unloaded,
                candidate_manager,
                candidate_autostart,
                prior_manager,
                prior_autostart,
            },
            layout_operation_count,
        })
    }

    fn matches_exact_state(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        layout_operation_index: u16,
        platform_record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<bool, InstallPlatformError> {
        let record = self.validated_record(platform_record)?;
        let receipt = self.validated_receipt(&record, candidate_owner_receipt)?;
        let inspection = self
            .last_inspection
            .as_ref()
            .ok_or_else(|| error("exact checkpoint requires a preceding inspection"))?;
        if !Self::expected_layout_matches(inspection, &record, layout_operation_index, checkpoint) {
            return Ok(false);
        }
        let candidate_launcher = candidate_launcher_entry(record.candidate_launcher.as_ref());
        let launcher_expected = match checkpoint {
            PlatformCheckpoint::CandidateLauncher
            | PlatformCheckpoint::CandidateActive
            | PlatformCheckpoint::CandidateManager
            | PlatformCheckpoint::CandidateAutostart
            | PlatformCheckpoint::CandidateRuntime
            | PlatformCheckpoint::PriorActiveRestored => candidate_launcher,
            _ => record.prior_launcher.clone(),
        };
        if inspection.launcher != launcher_expected {
            return Ok(false);
        }
        let candidate_side = matches!(
            checkpoint,
            PlatformCheckpoint::CandidateActive
                | PlatformCheckpoint::CandidateManager
                | PlatformCheckpoint::CandidateAutostart
                | PlatformCheckpoint::CandidateRuntime
                | PlatformCheckpoint::PriorActiveRestored
                | PlatformCheckpoint::PriorLauncherRestored
                | PlatformCheckpoint::PriorLayoutRestored
        );
        let expected_systemd = if candidate_side {
            candidate_systemd(
                &record,
                &self.config.direct_fragment_path,
                expected.running_unit.is_some(),
                expected.autostart_enabled,
            )
        } else {
            prior_systemd(
                &record,
                expected.running_unit.is_some(),
                expected.autostart_enabled,
            )?
        };
        let early_rollback_prior_manager = matches!(
            checkpoint,
            PlatformCheckpoint::CandidateActive
                | PlatformCheckpoint::PriorActiveRestored
                | PlatformCheckpoint::PriorLauncherRestored
                | PlatformCheckpoint::PriorLayoutRestored
        ) && prior_systemd(
            &record,
            expected.running_unit.is_some(),
            expected.autostart_enabled,
        )
        .is_ok_and(|prior| systemd_equivalent(&inspection.systemd, &prior));
        // Querying a newly published unit can load it before daemon-reload.
        // Its exact inactive candidate identity is valid only after publication.
        let discovered_candidate_launcher = checkpoint == PlatformCheckpoint::CandidateLauncher
            && expected.running_unit.is_none()
            && record.candidate_launcher.is_some()
            && systemd_equivalent(
                &inspection.systemd,
                &candidate_systemd(
                    &record,
                    &self.config.direct_fragment_path,
                    false,
                    expected.autostart_enabled,
                ),
            );
        if !systemd_equivalent(&inspection.systemd, &expected_systemd)
            && !early_rollback_prior_manager
            && !discovered_candidate_launcher
        {
            return Ok(false);
        }
        let Some(running_unit) = &expected.running_unit else {
            return Ok(true);
        };
        if checkpoint == PlatformCheckpoint::CandidateRuntime {
            if running_unit != &record.candidate.unit {
                return Ok(false);
            }
            if inspection.systemd.invocation_id == record.baseline_systemd.invocation_id {
                return Ok(false);
            }
            if let Some(receipt) = receipt {
                return Ok(receipt.unit == *running_unit
                    && receipt.invocation_id == inspection.systemd.invocation_id
                    && receipt.main_pid == inspection.systemd.main_pid);
            }
            return Ok(true);
        }
        if checkpoint == PlatformCheckpoint::PriorOriginal {
            return Ok(record
                .prior
                .as_ref()
                .is_some_and(|prior| &prior.unit == running_unit)
                && inspection.systemd.invocation_id == record.baseline_systemd.invocation_id
                && inspection.systemd.main_pid == record.baseline_systemd.main_pid);
        }
        if checkpoint != PlatformCheckpoint::PriorRestored
            || record
                .prior
                .as_ref()
                .is_none_or(|prior| &prior.unit != running_unit)
            || inspection.systemd.invocation_id == record.baseline_systemd.invocation_id
            || receipt
                .as_ref()
                .is_some_and(|receipt| receipt.invocation_id == inspection.systemd.invocation_id)
        {
            return Ok(false);
        }
        Ok(true)
    }

    fn capture_candidate_owner_receipt(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<PlatformOwnerReceipt, InstallPlatformError> {
        let record = self.validated_record(platform_record)?;
        let receipt = self.capture_stop_authority(expected, &record)?;
        let payload = serde_json::to_vec(&receipt).map_err(|source| error(source.to_string()))?;
        PlatformOwnerReceipt::linux(LINUX_RECEIPT_SCHEMA_VERSION, payload)
            .map_err(|source| error(source.to_string()))
    }

    fn validate_transaction_plan(
        &mut self,
        prior: &PlatformState,
        target: &PlatformState,
        transitions: &PlatformTransitionStates,
        layout_operation_count: u16,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let record = self.validated_record(platform_record)?;
        let record_has_prior_launcher = !matches!(record.prior_launcher, LinuxExactEntry::Absent);
        if usize::from(layout_operation_count) != record.layout.len()
            || transitions.prior_unloaded.loaded
            || transitions.candidate_manager.loaded
            || transitions.prior_manager.loaded
            || transitions.candidate_autostart.running_unit.is_some()
            || transitions.prior_autostart.running_unit.is_some()
            || record.candidate.unit
                != target
                    .layout_unit
                    .clone()
                    .ok_or_else(|| error("Linux target lacks candidate layout unit"))?
            || target.launcher_unit.is_some() != record.candidate_launcher.is_some()
            || prior.launcher_unit.is_some() != record_has_prior_launcher
        {
            return Err(error(
                "Linux transaction plan hides compound platform effects",
            ));
        }
        Ok(())
    }

    fn preflight_authority(
        &mut self,
        candidate: &UnitId,
        prior: &InstallationState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let record = self.validated_record(platform_record)?;
        if &record.candidate.unit != candidate {
            return Err(error(
                "candidate unit does not match the prepared Linux authority",
            ));
        }
        let inspection = self.inspect_exact()?;
        if self.state_from(&inspection)? != prior.platform
            || inspection.active_unit != prior.active_unit
            || inspection.systemd != record.baseline_systemd
            || inspection.launcher != record.prior_launcher
            || inspection.launcher_bytes != record.prior_launcher_bytes
        {
            return Err(error("Linux direct authority drifted before unload"));
        }
        for operation in &record.layout {
            if let LinuxLayoutEffect::Directory { item } = operation.effect
                && inspection.directories[&item] != LinuxDirectoryState::Absent
            {
                return Err(error(
                    "Linux public scaffolding appeared after transaction preparation",
                ));
            }
        }
        self.prove_baseline_owner(&prior.platform, &record)?;
        Ok(())
    }

    fn wait_for_guard_release(
        &mut self,
        unloaded: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let record = self.validated_record(platform_record)?;
        self.prove_owner(PlatformCheckpoint::PriorUnloaded, unloaded, &record, None)
            .map(|_| ())
    }

    fn install_launcher(
        &mut self,
        checkpoint: PlatformCheckpoint,
        unit: Option<&UnitId>,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let record = self.validated_record(platform_record)?;
        let current = self.executor.launcher_entry(MAX_LAUNCHER_BYTES)?.0;
        let (expected, publication) = launcher_effect(checkpoint, unit, &record)?;
        require_exact_entry(
            &current,
            &expected,
            "Linux launcher drifted before exact mutation",
        )?;
        self.executor
            .replace_launcher(&expected, publication.as_ref())?;
        self.last_inspection = None;
        Ok(())
    }

    fn install_layout_operation(
        &mut self,
        checkpoint: PlatformCheckpoint,
        unit: Option<&UnitId>,
        operation_index: u16,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let record = self.validated_record(platform_record)?;
        let operation = record
            .layout
            .get(usize::from(operation_index))
            .ok_or_else(|| error("layout operation cursor exceeds the fixed plan"))?;
        let LinuxLayoutEffect::Entry {
            item,
            prior,
            candidate_target,
        } = &operation.effect
        else {
            let item = match &operation.effect {
                LinuxLayoutEffect::Directory { item } => *item,
                LinuxLayoutEffect::Entry { .. } => unreachable!("matched entry effect"),
            };
            let current = self.executor.directory_state(item)?;
            let direction = layout_direction(checkpoint)?;
            validate_layout_unit(direction, unit, &record)?;
            let create = directory_create_effect(direction, current)?;
            if !create {
                self.last_inspection = None;
                return Ok(());
            }
            self.executor
                .replace_directory(item, LinuxDirectoryState::Absent, create)?;
            self.last_inspection = None;
            return Ok(());
        };
        let current = self.executor.layout_entry(*item)?;
        let (expected, replacement) = layout_effect(
            checkpoint,
            unit,
            &record,
            prior,
            candidate_target,
            &self.known_units,
        )?;
        require_exact_entry(
            &current,
            &expected,
            "Linux public layout drifted before exact mutation",
        )?;
        self.executor
            .replace_layout(*item, &expected, replacement.as_ref())?;
        self.last_inspection = None;
        Ok(())
    }

    fn reload_manager(
        &mut self,
        _expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        self.validated_record(platform_record)?;
        self.executor.reload_manager()?;
        self.last_inspection = None;
        Ok(())
    }

    fn restore_autostart(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        self.validated_record(platform_record)?;
        self.executor.set_autostart(expected.autostart_enabled)?;
        self.last_inspection = None;
        Ok(())
    }

    fn restore_runtime(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<(), InstallPlatformError> {
        let record = self.validated_record(platform_record)?;
        self.validated_receipt(&record, candidate_owner_receipt)?;
        self.executor.set_runtime(expected.running_unit.is_some())?;
        self.last_inspection = None;
        Ok(())
    }

    fn wait_for_newer_owner(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<(), InstallPlatformError> {
        let record = self.validated_record(platform_record)?;
        let receipt = self.validated_receipt(&record, candidate_owner_receipt)?;
        self.prove_owner(checkpoint, expected, &record, receipt.as_ref())
            .map(|_| ())
    }
}
