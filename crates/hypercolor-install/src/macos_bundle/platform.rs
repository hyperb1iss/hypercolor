use std::path::Path;
use std::time::Duration;

use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;

use super::super::{
    InstallDisposition, InstallLock, InstallPlatform, InstallPlatformError, InstallationState,
    PlatformCheckpoint, PlatformOwnerReceipt, PlatformState, PlatformTransactionRecord,
    PlatformTransitionStates, PreparedPlatformTransaction, RestoredRelease, UnitId, UnitRecord,
};
use super::executor::MacosBundleExecutor;
use super::launcher::unit_of_active_link;
use super::model::{
    MACOS_BUNDLE_STAGING_MARGIN_BYTES, MacosBundleConfig, MacosBundleObservation,
    MacosBundleRunningDaemon, MacosBundleServiceObservation, MacosBundleUnit, error,
};
use super::record::{BundleBinding, BundleRecord, decode_receipt, encode_receipt};
use super::service::MacosBundleService;
use super::swap::{SWAP_OPERATION_COUNT, SwapPosition, SwapStep};
use crate::InstallStore;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Inspection {
    active: Option<UnitId>,
    bundles: MacosBundleObservation,
    service: MacosBundleServiceObservation,
}

/// The install platform of a macOS app bundle: one signed `Hypercolor.app`
/// players launch, replaced by an atomic swap with a staged copy of the
/// candidate unit.
///
/// The executor owns the filesystem effects and the service the daemon
/// agent; this type decides every step from their observations.
pub struct MacosBundleInstallPlatform<E, S> {
    executor: E,
    service: S,
    config: MacosBundleConfig,
    units: Vec<(UnitRecord, MacosBundleUnit)>,
    last: Option<Inspection>,
    restored: Option<RestoredRelease>,
}

impl<E: MacosBundleExecutor, S: MacosBundleService> MacosBundleInstallPlatform<E, S> {
    /// Bind a platform to its roots and to the retained units a journal may
    /// name: the active unit, and a pending transaction's candidate.
    ///
    /// # Errors
    /// Returns an error for inexact roots or a unit that is not an app
    /// bundle unit.
    pub fn new(
        executor: E,
        service: S,
        config: MacosBundleConfig,
        known_units: impl IntoIterator<Item = UnitRecord>,
    ) -> Result<Self, InstallPlatformError> {
        config.validate()?;
        let mut platform = Self {
            executor,
            service,
            config,
            units: Vec::new(),
            last: None,
            restored: None,
        };
        for unit in known_units {
            platform.bind(&unit)?;
        }
        Ok(platform)
    }

    #[must_use]
    pub fn into_parts(self) -> (E, S) {
        (self.executor, self.service)
    }

    /// Remove what the store's terminal transaction left in the staging
    /// root: the displaced prior after a commit, the retired candidate after
    /// a rollback, and every interrupted leftover of either.
    ///
    /// It runs under `lock`, the store's transaction lock, and reads the
    /// journal itself. Returns whether anything was removed; with no journal
    /// there is nothing to settle, and repeating it is harmless. A later
    /// transaction cannot be prepared until this has run.
    ///
    /// # Errors
    /// Returns an error for a foreign lock, a nonterminal journal, a journal
    /// of another platform, or bundles that differ from the transaction's
    /// outcome; in the last case nothing is removed.
    pub fn settle(
        &mut self,
        store: &InstallStore,
        lock: &InstallLock,
    ) -> Result<bool, InstallPlatformError> {
        let Some(journal) = store
            .load_journal(lock)
            .map_err(|source| error(source.to_string()))?
        else {
            return Ok(false);
        };
        let committed = match journal.disposition {
            InstallDisposition::Committed => true,
            InstallDisposition::RolledBack => false,
            InstallDisposition::Forward | InstallDisposition::Rollback => {
                return Err(error(
                    "a macOS bundle transaction settles only once it is terminal",
                ));
            }
        };
        let record = self.record(&journal.platform_record)?;
        let observation = self.executor.observe()?;
        let remove = record
            .plan()
            .settle(committed, &observation, &record.staging_name)?;
        if remove {
            self.executor.discard(&record.staging_name)?;
        }
        self.last = None;
        Ok(remove)
    }

    fn bind(&mut self, unit: &UnitRecord) -> Result<(), InstallPlatformError> {
        if let Some((known, _)) = self.units.iter().find(|(known, _)| known.id() == unit.id()) {
            return if known == unit {
                Ok(())
            } else {
                Err(error(
                    "two retained authorities claim one macOS bundle unit",
                ))
            };
        }
        let bound = crate::payload::bind_macos_bundle_unit(unit)
            .map_err(|source| error(source.to_string()))?;
        self.units.push((unit.clone(), bound));
        Ok(())
    }

    fn unit(&self, id: &UnitId) -> Result<&(UnitRecord, MacosBundleUnit), InstallPlatformError> {
        self.units
            .iter()
            .find(|(record, _)| record.id() == id)
            .ok_or_else(|| error(format!("macOS bundle unit {} is not retained", id.as_str())))
    }

    fn record(
        &self,
        record: &PlatformTransactionRecord,
    ) -> Result<BundleRecord, InstallPlatformError> {
        let record = BundleRecord::decode(record)?;
        if record.installed_bundle != self.config.installed_bundle {
            return Err(error(
                "the macOS bundle record was prepared for another installed bundle",
            ));
        }
        Ok(record)
    }

    fn inspect_exact(&mut self) -> Result<Inspection, InstallPlatformError> {
        let active = read_active_unit(&self.config.release_root)?;
        let bundles = self.executor.observe()?;
        let service = self.service.observe()?;
        if let Some(running) = &service.running {
            running.validate()?;
            if !service.registered {
                return Err(error("a macOS daemon runs without its registered agent"));
            }
        }
        for slot in std::iter::once(&bundles.installed).chain(bundles.staged.values()) {
            if let super::MacosBundleSlot::Bundle(identity) = slot {
                identity.validate()?;
            }
        }
        Ok(Inspection {
            active,
            bundles,
            service,
        })
    }

    /// The coordinator's coarse view: the layout and the agent follow the
    /// active pointer, as on every platform, while the exact checkpoints
    /// below judge which bundle sits where.
    ///
    /// Autostart reads as the agent's registration, which no transaction
    /// changes. launchd's own gate is closed in the guard step before each
    /// swap, opened by the start that follows it, and left as it was found
    /// by the final proof, so it moves inside journaled steps and never
    /// changes a checkpoint.
    fn state_from(inspection: &Inspection) -> Result<PlatformState, InstallPlatformError> {
        let service = &inspection.service;
        let running_unit = match &service.running {
            None => None,
            Some(_) => Some(
                inspection
                    .active
                    .clone()
                    .ok_or_else(|| error("a macOS daemon runs while no unit is active"))?,
            ),
        };
        Ok(PlatformState {
            layout_unit: (!inspection.bundles.installed.is_absent())
                .then(|| inspection.active.clone())
                .flatten(),
            launcher_unit: service
                .registered
                .then(|| inspection.active.clone())
                .flatten(),
            loaded: service.running.is_some(),
            running_unit,
            autostart_enabled: service.registered,
        })
    }

    fn reserve(&mut self, unit: &MacosBundleUnit) -> Result<(), InstallPlatformError> {
        let needed = unit
            .installed_bytes()
            .checked_add(MACOS_BUNDLE_STAGING_MARGIN_BYTES)
            .ok_or_else(|| error("macOS bundle reservation overflows"))?;
        let available = self.executor.available_bytes()?;
        if available < needed {
            return Err(error(format!(
                "insufficient disk: the bundle filesystem has {available} bytes free and \
                 staging the candidate needs {needed}"
            )));
        }
        Ok(())
    }

    /// Whether `running` is the daemon of `binding` inside the installed
    /// bundle.
    fn runs_installed(&self, running: &MacosBundleRunningDaemon, binding: &BundleBinding) -> bool {
        running.cdhash == binding.identity.daemon
            && running.executable == self.config.installed_daemon()
    }

    /// Whether `running` is either side's daemon, inside the installed
    /// bundle or inside the staged copy where the displaced bundle lives
    /// while the transaction is open.
    fn runs_transaction_daemon(
        &self,
        record: &BundleRecord,
        running: &MacosBundleRunningDaemon,
    ) -> bool {
        std::iter::once(&record.candidate)
            .chain(record.prior.as_ref())
            .any(|binding| {
                running.cdhash == binding.identity.daemon
                    && (running.executable == self.config.installed_daemon()
                        || running.executable == self.config.staged_daemon(&record.staging_name))
            })
    }

    fn runtime_matches(
        &self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        record: &BundleRecord,
        receipt: Option<&MacosBundleRunningDaemon>,
        service: &MacosBundleServiceObservation,
    ) -> bool {
        let (Some(unit), Some(running)) = (&expected.running_unit, &service.running) else {
            return expected.running_unit.is_none() && service.running.is_none();
        };
        let Some(binding) = record.binding(unit) else {
            return false;
        };
        if !self.runs_installed(running, binding) {
            return false;
        }
        let is_prior = record
            .prior
            .as_ref()
            .is_some_and(|prior| &prior.unit == unit);
        let fresh = record
            .baseline
            .as_ref()
            .is_none_or(|baseline| baseline.pid != running.pid);
        match checkpoint {
            PlatformCheckpoint::PriorOriginal => {
                is_prior && record.baseline.as_ref() == Some(running)
            }
            PlatformCheckpoint::CandidateRuntime => {
                unit == &record.candidate.unit
                    && fresh
                    && receipt.is_none_or(|receipt| receipt == running)
            }
            PlatformCheckpoint::PriorRestored => {
                is_prior && fresh && receipt.is_none_or(|receipt| receipt.pid != running.pid)
            }
            _ => false,
        }
    }

    /// End a transaction with launchd's gate where it found it. A disabled
    /// agent can run, so this holds after a start that had to open the gate
    /// and after a transaction with nothing to start.
    fn leave_gate_as_found(
        &mut self,
        record: &BundleRecord,
        service: &MacosBundleServiceObservation,
    ) -> Result<(), InstallPlatformError> {
        if service.registered && service.enabled != record.autostart {
            self.service.set_enabled(record.autostart)?;
        }
        Ok(())
    }

    fn require_quiescent(expected: &PlatformState) -> Result<(), InstallPlatformError> {
        if expected.loaded || expected.running_unit.is_some() {
            return Err(error("macOS bundle checkpoint is not quiescent"));
        }
        Ok(())
    }
}

impl<E: MacosBundleExecutor, S: MacosBundleService> InstallPlatform
    for MacosBundleInstallPlatform<E, S>
{
    fn inspect(&mut self) -> Result<PlatformState, InstallPlatformError> {
        let inspection = self.inspect_exact()?;
        let state = Self::state_from(&inspection)?;
        self.last = Some(inspection);
        Ok(state)
    }

    fn prepare_transaction(
        &mut self,
        candidate: &UnitRecord,
        prior: &InstallationState,
        target: &PlatformState,
    ) -> Result<PreparedPlatformTransaction, InstallPlatformError> {
        let inspection = self
            .last
            .clone()
            .ok_or_else(|| error("prepare requires an exact prior macOS bundle inspection"))?;
        if inspection.active != prior.active_unit
            || Self::state_from(&inspection)? != prior.platform
        {
            return Err(error(
                "the macOS bundle installation changed before preparation",
            ));
        }
        self.bind(candidate)?;
        let candidate_unit = self.unit(candidate.id())?.1.clone();
        let prior_unit = prior
            .active_unit
            .as_ref()
            .map(|id| self.unit(id).map(|(_, unit)| unit.clone()))
            .transpose()?;
        let installed_is_prior = match &prior_unit {
            Some(unit) => inspection.bundles.installed.holds(Some(unit.identity())),
            None => inspection.bundles.installed.is_absent(),
        };
        if !installed_is_prior {
            return Err(error(
                "the installed macOS bundle is not the active unit's; adopt it before updating",
            ));
        }
        if prior_unit
            .as_ref()
            .is_some_and(|prior| prior.identity().daemon == candidate_unit.identity().daemon)
        {
            return Err(error(
                "the candidate's daemon has the installed daemon's cdhash, so the two could \
                 not be told apart",
            ));
        }
        // Even a retry of this candidate settles first: a leftover would
        // count against the reservation and be removed while the prior is
        // stopped.
        if !inspection.bundles.staged.is_empty() || !inspection.bundles.interrupted.is_empty() {
            return Err(error(
                "the macOS staging root still holds an earlier transaction; settle it first",
            ));
        }
        let staging_name = candidate.id().as_str().to_owned();
        if inspection.service.registered && inspection.active.is_none() {
            return Err(error(
                "a daemon agent is registered but no unit is active; adopt the installation \
                 before updating it",
            ));
        }
        if target.launcher_unit.is_some() != prior.platform.launcher_unit.is_some()
            || target.autostart_enabled != prior.platform.autostart_enabled
        {
            return Err(error(
                "registering or removing the daemon agent belongs to the installer, not to an \
                 update",
            ));
        }
        let baseline = inspection.service.running.clone();
        if let Some(running) = &baseline {
            let prior_binding = prior_unit.as_ref().map(BundleBinding::of);
            if !prior_binding
                .as_ref()
                .is_some_and(|binding| self.runs_installed(running, binding))
            {
                return Err(error(
                    "the running macOS daemon is not the installed bundle's daemon",
                ));
            }
        }
        self.reserve(&candidate_unit)?;
        let record = BundleRecord {
            installed_bundle: self.config.installed_bundle.clone(),
            staging_name,
            candidate: BundleBinding::of(&candidate_unit),
            prior: prior_unit.as_ref().map(BundleBinding::of),
            autostart: inspection.service.enabled,
            baseline,
        };
        Ok(PreparedPlatformTransaction {
            record: record.encode()?,
            transitions: transition_states(&prior.platform, target),
            layout_operation_count: SWAP_OPERATION_COUNT,
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
        let record = self.record(platform_record)?;
        let receipt = decode_receipt(candidate_owner_receipt)?;
        let inspection = self
            .last
            .as_ref()
            .ok_or_else(|| error("an exact macOS bundle checkpoint needs an inspection"))?;
        let Some(position) = SwapPosition::at(checkpoint, layout_operation_index) else {
            return Ok(false);
        };
        Ok(record
            .plan()
            .holds(position, &inspection.bundles, &record.staging_name)
            && self.runtime_matches(
                checkpoint,
                expected,
                &record,
                receipt.as_ref(),
                &inspection.service,
            ))
    }

    fn capture_candidate_owner_receipt(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<PlatformOwnerReceipt, InstallPlatformError> {
        let record = self.record(platform_record)?;
        let inspection = self.inspect_exact()?;
        let running = inspection
            .service
            .running
            .clone()
            .ok_or_else(|| error("the macOS candidate daemon is not running"))?;
        if expected.running_unit.as_ref() != Some(&record.candidate.unit)
            || !self.runtime_matches(
                PlatformCheckpoint::CandidateRuntime,
                expected,
                &record,
                None,
                &inspection.service,
            )
        {
            return Err(error("the running macOS daemon is not the candidate's"));
        }
        self.last = Some(inspection);
        encode_receipt(&running)
    }

    fn validate_transaction_plan(
        &mut self,
        prior: &PlatformState,
        target: &PlatformState,
        transitions: &PlatformTransitionStates,
        layout_operation_count: u16,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let record = self.record(platform_record)?;
        if layout_operation_count != SWAP_OPERATION_COUNT
            || transitions != &transition_states(prior, target)
            || target.layout_unit.as_ref() != Some(&record.candidate.unit)
            || prior.layout_unit.is_some() != record.prior.is_some()
            || prior.launcher_unit.is_some() != target.launcher_unit.is_some()
        {
            return Err(error(
                "macOS bundle transaction plan differs from its exact swap",
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
        let record = self.record(platform_record)?;
        if &record.candidate.unit != candidate {
            return Err(error(
                "candidate unit differs from the prepared macOS bundle",
            ));
        }
        let inspection = self.inspect_exact()?;
        if inspection.active != prior.active_unit
            || Self::state_from(&inspection)? != prior.platform
            || !record.plan().holds(
                SwapPosition::Before,
                &inspection.bundles,
                &record.staging_name,
            )
            || inspection.service.running != record.baseline
        {
            return Err(error("the macOS bundle installation drifted before unload"));
        }
        let unit = self.unit(candidate)?.1.clone();
        self.reserve(&unit)?;
        self.last = Some(inspection);
        Ok(())
    }

    fn wait_for_guard_release(
        &mut self,
        unloaded: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        self.record(platform_record)?;
        Self::require_quiescent(unloaded)?;
        // Both swaps follow this step, so an interrupted one never starts a
        // half-finished change at the next login: launchd keeps the closed
        // gate across a restart. The gate does nothing to a loaded job, so
        // a crashed daemon's job goes too, before launchd can restart it
        // around the swap.
        let service = self.service.observe()?;
        if service.registered && service.enabled {
            self.service.set_enabled(false)?;
        }
        self.service.stop(None)?;
        self.service.wait_for_release()?;
        self.last = None;
        Ok(())
    }

    fn install_launcher(
        &mut self,
        _checkpoint: PlatformCheckpoint,
        _unit: Option<&UnitId>,
        _platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        // Every plan keeps the agent's registration, and the agent always
        // runs the installed bundle, so no transaction step changes it.
        Err(error(
            "the macOS daemon agent definition does not change within a bundle transaction",
        ))
    }

    fn install_layout_operation(
        &mut self,
        checkpoint: PlatformCheckpoint,
        unit: Option<&UnitId>,
        operation_index: u16,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let record = self.record(platform_record)?;
        let forward = match checkpoint {
            PlatformCheckpoint::CandidateLayout => true,
            PlatformCheckpoint::PriorLayoutRestored => false,
            _ => {
                return Err(error(
                    "macOS bundle layout effect lacks a layout checkpoint",
                ));
            }
        };
        let expected_unit = if forward {
            Some(&record.candidate.unit)
        } else {
            record.prior.as_ref().map(|prior| &prior.unit)
        };
        if unit != expected_unit {
            return Err(error("macOS bundle layout effect names the wrong unit"));
        }
        let plan = record.plan();
        let (from, _, step) = plan.step(forward, operation_index)?;
        let observation = self.executor.observe()?;
        if !plan.holds(from, &observation, &record.staging_name) {
            return Err(error(format!(
                "the macOS bundles drifted before the exact swap step: expected {from:?}, \
                 observed {:?}",
                plan.position_of(&observation, &record.staging_name)
            )));
        }
        let name = record.staging_name.as_str();
        match step {
            SwapStep::Materialize => {
                // The room was reserved before the prior stopped; reserving
                // again here would count a crashed attempt's own leftovers,
                // which materializing removes first.
                let (source, bound) = self.unit(&record.candidate.unit)?.clone();
                self.executor.materialize(&source, &bound, name)?;
            }
            SwapStep::Exchange => self.executor.exchange(name)?,
            SwapStep::InstallStaged => self.executor.install_staged(name)?,
            SwapStep::WithdrawInstalled => self.executor.withdraw_installed(name)?,
            SwapStep::Retire => self.executor.retire_staged(name)?,
        }
        self.last = None;
        Ok(())
    }

    fn reload_manager(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        // launchd reads an agent's definition when it bootstraps it; there
        // is no separate reload to run.
        self.record(platform_record)?;
        Self::require_quiescent(expected)?;
        self.last = None;
        Ok(())
    }

    fn restore_autostart(
        &mut self,
        _expected: &PlatformState,
        _platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        // Every plan keeps the agent's registration, which is all the
        // coordinator sees of autostart, so no transaction step changes it.
        Err(error(
            "the macOS daemon agent's autostart does not change within a bundle transaction",
        ))
    }

    fn restore_runtime(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
        _candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<(), InstallPlatformError> {
        let record = self.record(platform_record)?;
        let inspection = self.inspect_exact()?;
        match &expected.running_unit {
            None => {
                if let Some(running) = &inspection.service.running {
                    if !self.runs_transaction_daemon(&record, running) {
                        return Err(error(
                            "the running macOS daemon belongs to neither side of this transaction",
                        ));
                    }
                    self.service.stop(Some(running))?;
                }
            }
            Some(unit) => {
                let binding = record
                    .binding(unit)
                    .ok_or_else(|| error("macOS runtime target is outside this transaction"))?;
                if inspection.service.running.is_some()
                    || !inspection.bundles.installed.holds(Some(&binding.identity))
                {
                    return Err(error(
                        "the macOS daemon can start only from its own installed bundle, alone",
                    ));
                }
                // launchd refuses to load a disabled agent, and this start
                // is the only thing that needs it open.
                if !inspection.service.enabled {
                    self.service.set_enabled(true)?;
                }
                self.service.start(&binding.identity.daemon)?;
            }
        }
        self.last = None;
        Ok(())
    }

    fn wait_for_newer_owner(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<(), InstallPlatformError> {
        let record = self.record(platform_record)?;
        let receipt = decode_receipt(candidate_owner_receipt)?;
        let inspection = self.inspect_exact()?;
        let Some(unit) = &expected.running_unit else {
            if inspection.service.running.is_some() {
                return Err(error("a macOS daemon runs where none is expected"));
            }
            return self.leave_gate_as_found(&record, &inspection.service);
        };
        let running = inspection
            .service
            .running
            .clone()
            .ok_or_else(|| error("the macOS daemon is not running for its proof"))?;
        if !self.runtime_matches(
            checkpoint,
            expected,
            &record,
            receipt.as_ref(),
            &inspection.service,
        ) {
            return Err(error(
                "the running macOS daemon is not the one this proof expects",
            ));
        }
        let binding = record
            .binding(unit)
            .cloned()
            .ok_or_else(|| error("macOS proof target is outside this transaction"))?;
        let probation = if checkpoint == PlatformCheckpoint::CandidateRuntime {
            self.config.probation
        } else {
            Duration::ZERO
        };
        self.service.prove(&running, &binding.version, probation)?;
        let after = self.inspect_exact()?;
        if after.service.running.as_ref() != Some(&running) {
            return Err(error("the macOS daemon changed during its proof"));
        }
        self.leave_gate_as_found(&record, &after.service)?;
        if checkpoint == PlatformCheckpoint::PriorRestored {
            self.restored = Some(RestoredRelease {
                unit: binding.unit,
                version: binding.version,
                instance: format!("pid:{}", running.pid),
                process_id: running.pid,
                executable_sha256: binding.daemon_sha256,
            });
        }
        self.last = Some(after);
        Ok(())
    }

    fn stop_unjournaled_runtime(
        &mut self,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<bool, InstallPlatformError> {
        let record = self.record(platform_record)?;
        let inspection = self.inspect_exact()?;
        let Some(running) = &inspection.service.running else {
            return Ok(false);
        };
        if !self.runs_transaction_daemon(&record, running) {
            return Err(error(
                "the running macOS daemon is not provably this transaction's",
            ));
        }
        self.service.stop(Some(running))?;
        self.last = None;
        Ok(true)
    }

    fn matches_exact_state_except_runtime(
        &mut self,
        checkpoint: PlatformCheckpoint,
        _expected: &PlatformState,
        layout_operation_index: u16,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<bool, InstallPlatformError> {
        let record = self.record(platform_record)?;
        let inspection = self
            .last
            .as_ref()
            .ok_or_else(|| error("an exact macOS bundle checkpoint needs an inspection"))?;
        Ok(
            SwapPosition::at(checkpoint, layout_operation_index).is_some_and(|position| {
                record
                    .plan()
                    .holds(position, &inspection.bundles, &record.staging_name)
            }),
        )
    }

    fn matches_untouched_prior(
        &mut self,
        prior: &PlatformState,
        platform_record: &PlatformTransactionRecord,
        restarted: bool,
    ) -> Result<bool, InstallPlatformError> {
        let record = self.record(platform_record)?;
        let inspection = self.inspect_exact()?;
        let service = &inspection.service;
        let untouched = record.plan().holds(
            SwapPosition::Before,
            &inspection.bundles,
            &record.staging_name,
        ) && service.registered == prior.launcher_unit.is_some()
            && (!restarted
                || service.running.as_ref().is_some_and(|running| {
                    record
                        .prior
                        .as_ref()
                        .is_some_and(|binding| self.runs_installed(running, binding))
                        && record.baseline.as_ref() != Some(running)
                }));
        self.last = Some(inspection);
        Ok(untouched)
    }

    fn take_restored_release(&mut self) -> Option<RestoredRelease> {
        self.restored.take()
    }
}

/// The states a transaction from `prior` to `target` passes through.
pub(super) fn transition_states(
    prior: &PlatformState,
    target: &PlatformState,
) -> PlatformTransitionStates {
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
    PlatformTransitionStates {
        prior_unloaded,
        candidate_manager,
        candidate_autostart,
        prior_autostart: prior_manager.clone(),
        prior_manager,
    }
}

/// Read the release root's `active` pointer, `units/<digest>`.
fn read_active_unit(release_root: &Path) -> Result<Option<UnitId>, InstallPlatformError> {
    let root = ReadOnlyDirectoryAuthority::open(release_root)
        .map_err(|source| error(format!("open the macOS release root: {source}")))?;
    let Some(target) = root
        .read_symlink(Path::new("active"))
        .map_err(|source| error(format!("read the macOS active pointer: {source}")))?
    else {
        return Ok(None);
    };
    unit_of_active_link(&target).map(Some).map_err(error)
}
