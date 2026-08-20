use std::collections::{BTreeMap, BTreeSet};

use super::super::{
    InstallPlatform, InstallPlatformError, InstallationState, PlatformCheckpoint,
    PlatformOwnerReceipt, PlatformState, PlatformTransactionRecord, PlatformTransitionStates,
    PreparedPlatformTransaction, UnitId, UnitRecord,
};
use super::effects::{
    candidate_launcher_entry, candidate_layout_entry, directory_effect, launcher_effect,
    layout_direction, layout_effect, require_exact_entry, validate_layout_unit,
};
use super::executor::MacosInstallExecutor;
use super::legacy::build_legacy_snapshot;
use super::model::{
    MACOS_RECEIPT_SCHEMA_VERSION, MACOS_RECORD_SCHEMA_VERSION, MAX_LAUNCHER_BYTES,
    MacosDirectoryState, MacosExactEntry, MacosFilePublication, MacosLayoutEffect,
    MacosLayoutOperation, MacosRecord, MacosStopAuthority, compare_directory_paths, entries_match,
    error,
};
use super::state::consistent_platform_unit;
use super::{MacosInspection, MacosInstallPlatform};

impl<E: MacosInstallExecutor> MacosInstallPlatform<E> {
    fn expected_layout_matches(
        inspection: &MacosInspection,
        record: &MacosRecord,
        index: u16,
        checkpoint: PlatformCheckpoint,
    ) -> bool {
        let entry_paths = record
            .layout
            .iter()
            .filter_map(|operation| match &operation.effect {
                MacosLayoutEffect::Entry { path, .. } => Some(path),
                MacosLayoutEffect::Directory { .. } => None,
            })
            .collect::<BTreeSet<_>>();
        if inspection.public.entries.len() != entry_paths.len()
            || inspection
                .public
                .entries
                .keys()
                .any(|path| !entry_paths.contains(path))
            || inspection.public.directories.len() != record.prior_directories.len()
            || inspection
                .public
                .directories
                .keys()
                .any(|path| !record.prior_directories.contains_key(path))
            || record.prior_directories.iter().any(|(path, prior)| {
                *prior == MacosDirectoryState::Present
                    && inspection.public.directories.get(path)
                        != Some(&MacosDirectoryState::Present)
            })
        {
            return false;
        }
        record
            .layout
            .iter()
            .enumerate()
            .all(|(operation_index, operation)| match &operation.effect {
                MacosLayoutEffect::Directory { path } => {
                    let actual = inspection.public.directories.get(path);
                    if is_prior_checkpoint(checkpoint) {
                        return matches!(
                            actual,
                            Some(MacosDirectoryState::Absent | MacosDirectoryState::Present)
                        );
                    }
                    let expected = if operation_index < usize::from(index) {
                        MacosDirectoryState::Present
                    } else {
                        MacosDirectoryState::Absent
                    };
                    actual == Some(&expected)
                }
                MacosLayoutEffect::Entry {
                    path,
                    prior,
                    candidate_target,
                } => {
                    let expected = if operation_index < usize::from(index) {
                        candidate_layout_entry(candidate_target.as_deref())
                    } else {
                        prior.clone()
                    };
                    inspection
                        .public
                        .entries
                        .get(path)
                        .is_some_and(|actual| entries_match(actual, &expected))
                }
            })
    }

    fn transitions(prior: &PlatformState, target: &PlatformState) -> PlatformTransitionStates {
        let prior_unloaded = PlatformState {
            loaded: false,
            running_unit: None,
            ..prior.clone()
        };
        let candidate_manager = PlatformState {
            loaded: false,
            running_unit: None,
            autostart_enabled: prior.autostart_enabled,
            ..target.clone()
        };
        let candidate_autostart = PlatformState {
            autostart_enabled: target.autostart_enabled,
            ..candidate_manager.clone()
        };
        let prior_manager = PlatformState {
            loaded: false,
            running_unit: None,
            autostart_enabled: prior.autostart_enabled,
            ..prior.clone()
        };
        let prior_autostart = prior_manager.clone();
        PlatformTransitionStates {
            prior_unloaded,
            candidate_manager,
            candidate_autostart,
            prior_manager,
            prior_autostart,
        }
    }

    fn add_candidate(&mut self, candidate: &UnitRecord) -> Result<(), InstallPlatformError> {
        if self
            .known_units
            .iter()
            .any(|known| known.id() == candidate.id())
        {
            return Ok(());
        }
        self.executor.validate_unit_authority(candidate)?;
        let projection = self.executor.candidate_layout(candidate)?;
        super::model::validate_candidate_layout(&projection)?;
        self.projections.push((candidate.id().clone(), projection));
        self.known_units.push(candidate.clone());
        Ok(())
    }

    fn prior_entries(
        inspection: &MacosInspection,
        synthetic: Option<&UnitRecord>,
    ) -> BTreeMap<String, MacosExactEntry> {
        inspection
            .public
            .entries
            .iter()
            .map(|(path, entry)| {
                let entry = match entry {
                    MacosExactEntry::RegularFile { mode, sha256, .. } => {
                        MacosExactEntry::RegularFile {
                            mode: *mode,
                            sha256: sha256.clone(),
                            snapshot_unit: synthetic.map(|unit| unit.id().clone()),
                            snapshot_path: synthetic.map(|_| path.clone()),
                        }
                    }
                    other => other.clone(),
                };
                (path.clone(), entry)
            })
            .collect()
    }

    fn launcher_matches_checkpoint(
        inspection: &MacosInspection,
        record: &MacosRecord,
        checkpoint: PlatformCheckpoint,
    ) -> bool {
        let candidate = candidate_launcher_entry(record.candidate_launcher.as_ref());
        let expected = match checkpoint {
            PlatformCheckpoint::CandidateLauncher
            | PlatformCheckpoint::CandidateActive
            | PlatformCheckpoint::CandidateManager
            | PlatformCheckpoint::CandidateAutostart
            | PlatformCheckpoint::CandidateRuntime
            | PlatformCheckpoint::PriorActiveRestored => candidate,
            _ => record.prior_launcher.clone(),
        };
        entries_match(&inspection.launcher, &expected)
    }
}

impl<E: MacosInstallExecutor> InstallPlatform for MacosInstallPlatform<E> {
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
            .ok_or_else(|| error("prepare requires an exact prior macOS inspection"))?;
        if self.state_from(&inspection)? != prior.platform {
            return Err(error("prior macOS inspection changed before preparation"));
        }
        self.add_candidate(candidate)?;
        let candidate_binding = self.unit_binding(candidate)?;
        let prior_platform_unit = consistent_platform_unit(&prior.platform)?;
        let first_conversion = prior.active_unit.is_none()
            && prior_platform_unit
                .as_ref()
                .is_some_and(|unit| unit.as_str().starts_with("legacy-"));
        let synthetic = if first_conversion {
            let unit = prior_platform_unit
                .clone()
                .ok_or_else(|| error("macOS first conversion lacks a legacy unit ID"))?;
            let snapshot = build_legacy_snapshot(&inspection, unit)?;
            let record = self.executor.snapshot_legacy_unit(&snapshot)?;
            self.executor.validate_unit_authority(&record)?;
            self.legacy_unit = Some(record.id().clone());
            self.known_units.push(record.clone());
            Some((record, snapshot.executable))
        } else {
            None
        };
        let prior_binding = if let Some((unit, executable)) = &synthetic {
            Some(self.legacy_binding(unit, executable))
        } else {
            prior_platform_unit
                .as_ref()
                .map(|unit| {
                    self.retained_unit(unit)
                        .and_then(|unit| self.unit_binding(unit))
                })
                .transpose()?
        };
        if prior.platform.loaded && prior_binding.is_none() {
            return Err(error(
                "loaded direct launchd owner lacks retained immutable prior authority",
            ));
        }
        let candidate_launcher = target
            .launcher_unit
            .as_ref()
            .map(|_| self.candidate_launcher())
            .transpose()?;
        let candidate_launcher_snapshot = candidate_launcher
            .as_ref()
            .map(|launcher| {
                self.executor
                    .persist_launcher_snapshot(&MacosFilePublication {
                        mode: launcher.mode,
                        contents: launcher.bytes.as_bytes().to_vec(),
                    })
            })
            .transpose()?;
        let prior_launcher_snapshot = match (&inspection.launcher, &prior_binding) {
            (MacosExactEntry::RegularFile { mode, .. }, Some(_)) => Some(
                self.executor
                    .persist_launcher_snapshot(&MacosFilePublication {
                        mode: *mode,
                        contents: inspection.launcher_bytes.clone(),
                    })?,
            ),
            (MacosExactEntry::RegularFile { .. }, None) => {
                return Err(error(
                    "prior macOS launcher lacks retained private snapshot authority",
                ));
            }
            (MacosExactEntry::Absent | MacosExactEntry::Symlink { .. }, _) => None,
        };
        let synthetic_unit = synthetic.as_ref().map(|(unit, _)| unit);
        let prior_entries = Self::prior_entries(&inspection, synthetic_unit);
        let projection = self.projection(candidate.id())?;
        let targets = projection
            .entries
            .iter()
            .cloned()
            .collect::<BTreeMap<_, _>>();
        let mut layout = inspection
            .public
            .directories
            .iter()
            .filter(|(_, state)| **state == MacosDirectoryState::Absent)
            .map(|(path, _)| MacosLayoutOperation {
                effect: MacosLayoutEffect::Directory { path: path.clone() },
            })
            .collect::<Vec<_>>();
        layout.sort_by(|left, right| match (&left.effect, &right.effect) {
            (
                MacosLayoutEffect::Directory { path: left },
                MacosLayoutEffect::Directory { path: right },
            ) => compare_directory_paths(left, right),
            _ => std::cmp::Ordering::Equal,
        });
        let entry_paths = prior_entries
            .keys()
            .chain(targets.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        layout.extend(entry_paths.into_iter().map(|path| {
            MacosLayoutOperation {
                effect: MacosLayoutEffect::Entry {
                    prior: prior_entries
                        .get(&path)
                        .cloned()
                        .unwrap_or(MacosExactEntry::Absent),
                    candidate_target: targets.get(&path).cloned(),
                    path,
                },
            }
        }));
        let baseline_owner_epoch = inspection.launchd.pid.map(|_| {
            inspection
                .owner_record
                .as_ref()
                .expect("loaded inspection validated its owner record")
                .owner_epoch
        });
        let baseline_stop_authority = inspection
            .launchd
            .pid
            .and(inspection.owner_record.as_ref())
            .map(|owner| {
                let prior = prior_binding
                    .as_ref()
                    .expect("loaded inspection validated its prior binding");
                MacosStopAuthority {
                    owner_epoch: owner.owner_epoch,
                    audit_token_identity: owner.active_identity.audit_token_identity.clone(),
                    executable_path: owner.active_identity.executable_path.clone(),
                    designated_requirement_hash: owner
                        .active_identity
                        .designated_requirement_hash
                        .clone(),
                    pid: owner.active_identity.pid,
                    unit: prior.unit.clone(),
                }
            });
        let record = MacosRecord {
            candidate: candidate_binding,
            prior: prior_binding,
            baseline_launchd: inspection.launchd,
            baseline_owner_epoch,
            baseline_stop_authority,
            prior_launcher: inspection.launcher,
            prior_launcher_bytes: String::from_utf8(inspection.launcher_bytes)
                .map_err(|_| error("prior macOS launchd plist is not exact UTF-8"))?,
            candidate_launcher,
            prior_launcher_snapshot,
            candidate_launcher_snapshot,
            prior_directories: inspection.public.directories,
            prior_entries,
            layout,
            first_conversion,
        };
        let layout_operation_count = u16::try_from(record.layout.len())
            .map_err(|_| error("macOS layout operation count exceeds u16"))?;
        let payload = serde_json::to_vec(&record)
            .map_err(|source| error(format!("encode macOS platform record: {source}")))?;
        let record = PlatformTransactionRecord::macos(MACOS_RECORD_SCHEMA_VERSION, payload)
            .map_err(|source| error(source.to_string()))?;
        Ok(PreparedPlatformTransaction {
            record,
            transitions: Self::transitions(&prior.platform, target),
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
        if let Some(destination) = super::runtime::effect_destination(checkpoint) {
            self.pending_effect_checkpoint = Some(destination);
        }
        let record = self.validated_record(platform_record)?;
        let receipt = self.validated_receipt(&record, candidate_owner_receipt)?;
        let inspection = self
            .last_inspection
            .as_ref()
            .ok_or_else(|| error("exact macOS checkpoint requires a preceding inspection"))?;
        if !Self::expected_layout_matches(inspection, &record, layout_operation_index, checkpoint)
            || !Self::launcher_matches_checkpoint(inspection, &record, checkpoint)
            || inspection.launchd.autostart_enabled != expected.autostart_enabled
            || inspection.launchd.pid.is_some() != expected.running_unit.is_some()
        {
            return Ok(false);
        }
        let Some(running) = &expected.running_unit else {
            return Ok(true);
        };
        let owner = inspection
            .owner_record
            .as_ref()
            .ok_or_else(|| error("running macOS checkpoint lacks its owner record"))?;
        match checkpoint {
            PlatformCheckpoint::PriorOriginal => Ok(record.prior.as_ref().is_some_and(|prior| {
                &prior.unit == running
                    && record
                        .baseline_stop_authority
                        .as_ref()
                        .is_some_and(|authority| {
                            super::runtime::owner_matches_stop_authority(owner, authority)
                        })
            })),
            PlatformCheckpoint::CandidateRuntime => Ok(running == &record.candidate.unit
                && super::runtime::owner_matches_binding(owner, &record.candidate)
                && owner.owner_epoch > record.baseline_owner_epoch.unwrap_or_default()
                && receipt
                    .as_ref()
                    .is_none_or(|receipt| super::runtime::owner_matches_receipt(owner, receipt))),
            PlatformCheckpoint::PriorRestored => Ok(record.prior.as_ref().is_some_and(|prior| {
                &prior.unit == running
                    && super::runtime::owner_matches_binding(owner, prior)
                    && owner.owner_epoch > record.baseline_owner_epoch.unwrap_or_default()
                    && receipt
                        .as_ref()
                        .is_none_or(|receipt| owner.owner_epoch > receipt.owner_epoch)
            })),
            _ => Ok(false),
        }
    }

    fn capture_candidate_owner_receipt(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<PlatformOwnerReceipt, InstallPlatformError> {
        let record = self.validated_record(platform_record)?;
        let receipt = self.capture_stop_authority(expected, &record)?;
        let payload = serde_json::to_vec(&receipt)
            .map_err(|source| error(format!("encode macOS owner receipt: {source}")))?;
        PlatformOwnerReceipt::macos(MACOS_RECEIPT_SCHEMA_VERSION, payload)
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
        if usize::from(layout_operation_count) != record.layout.len()
            || transitions != &Self::transitions(prior, target)
            || record.candidate.unit
                != target
                    .layout_unit
                    .clone()
                    .ok_or_else(|| error("macOS target lacks its candidate layout unit"))?
            || target.launcher_unit.is_some() != record.candidate_launcher.is_some()
            || prior.launcher_unit.is_some()
                != !matches!(record.prior_launcher, MacosExactEntry::Absent)
        {
            return Err(error(
                "macOS transaction plan hides compound platform effects",
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
                "candidate unit differs from prepared macOS authority",
            ));
        }
        let inspection = self.inspect_exact()?;
        if self.state_from(&inspection)? != prior.platform
            || inspection.active_unit != prior.active_unit
            || inspection.launchd != record.baseline_launchd
            || !entries_match(&inspection.launcher, &record.prior_launcher)
            || inspection.launcher_bytes != record.prior_launcher_bytes.as_bytes()
            || inspection.public.directories != record.prior_directories
            || inspection.public.entries.len() != record.prior_entries.len()
            || !record.prior_entries.iter().all(|(path, expected)| {
                inspection
                    .public
                    .entries
                    .get(path)
                    .is_some_and(|actual| entries_match(actual, expected))
            })
        {
            return Err(error("macOS direct authority drifted before unload"));
        }
        self.prove_baseline_owner(&prior.platform, &record)
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
            "macOS launchd plist drifted before exact mutation",
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
            .ok_or_else(|| error("macOS layout cursor exceeds its fixed plan"))?;
        match &operation.effect {
            MacosLayoutEffect::Directory { path } => {
                let direction = layout_direction(checkpoint)?;
                validate_layout_unit(direction, unit, &record)?;
                let current = self
                    .executor
                    .public_snapshot(&[self.projection(&record.candidate.unit)?.clone()])?
                    .directories
                    .get(path)
                    .copied()
                    .ok_or_else(|| error("macOS directory effect is not exactly observable"))?;
                let create = directory_effect(direction, current)?;
                if create {
                    self.executor
                        .replace_directory(path, MacosDirectoryState::Absent, true)?;
                }
            }
            MacosLayoutEffect::Entry {
                path,
                prior,
                candidate_target,
            } => {
                let current = self
                    .executor
                    .public_snapshot(&[self.projection(&record.candidate.unit)?.clone()])?
                    .entries
                    .get(path)
                    .cloned()
                    .ok_or_else(|| error("macOS layout entry is not exactly observable"))?;
                let (expected, replacement) = layout_effect(
                    &mut self.executor,
                    checkpoint,
                    unit,
                    &record,
                    prior,
                    candidate_target.as_deref(),
                    &self.known_units,
                )?;
                require_exact_entry(
                    &current,
                    &expected,
                    "macOS public layout drifted before exact mutation",
                )?;
                self.executor
                    .replace_layout(path, &expected, replacement.as_ref())?;
            }
        }
        self.last_inspection = None;
        Ok(())
    }

    fn reload_manager(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        self.validated_record(platform_record)?;
        if expected.loaded || expected.running_unit.is_some() {
            return Err(error("macOS manager checkpoint is not quiescent"));
        }
        self.last_inspection = None;
        Ok(())
    }

    fn restore_autostart(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        self.validated_record(platform_record)?;
        if expected.loaded || expected.running_unit.is_some() {
            return Err(error("macOS autostart checkpoint is not quiescent"));
        }
        let _ = self.executor.set_autostart(expected.autostart_enabled)?;
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
        let receipt = self.validated_receipt(&record, candidate_owner_receipt)?;
        let checkpoint = self
            .pending_effect_checkpoint
            .take()
            .ok_or_else(|| error("macOS runtime effect lacks its exact checkpoint direction"))?;
        if let Some(transition) =
            super::runtime::transition(checkpoint, expected, &record, receipt.as_ref())?
        {
            let _ = self.executor.transition_runtime(&transition)?;
        }
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

fn is_prior_checkpoint(checkpoint: PlatformCheckpoint) -> bool {
    matches!(
        checkpoint,
        PlatformCheckpoint::PriorOriginal
            | PlatformCheckpoint::PriorActiveRestored
            | PlatformCheckpoint::PriorLauncherRestored
            | PlatformCheckpoint::PriorLayoutRestored
            | PlatformCheckpoint::PriorManager
            | PlatformCheckpoint::PriorAutostart
            | PlatformCheckpoint::PriorRestored
    )
}
