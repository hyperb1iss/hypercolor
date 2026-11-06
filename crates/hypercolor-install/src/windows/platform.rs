use std::time::Duration;

use super::executor::WindowsInstallExecutor;
use super::layout::{self, LAYOUT_OPERATION_COUNT, LayoutStep, position_at};
use super::model::{
    WINDOWS_APP_ARGUMENTS, WINDOWS_APP_MEMBER, WINDOWS_BROKER_MEMBER, WINDOWS_DAEMON_MEMBER,
    WindowsAppStart, WindowsBrokerObservation, WindowsFileIdentity, WindowsFirewallObservation,
    WindowsInstallConfig, WindowsProcessId, WindowsRunningProcess, WindowsRuntime,
    WindowsRuntimeObservation, WindowsUnitBinding, error, same_windows_path, service_program,
};
use super::record::{WindowsBrokerBaseline, WindowsRecord, decode_receipt, encode_receipt};
use super::session::WindowsSessionHost;
use crate::local_http::{MAX_HTTP_RESPONSE_BYTES, verify_http};
use crate::{
    InstallPlatform, InstallPlatformError, InstallationState, PlatformCheckpoint,
    PlatformOwnerReceipt, PlatformState, PlatformTransactionRecord, PlatformTransitionStates,
    PreparedPlatformTransaction, RestoredRelease, UnitId, UnitRecord,
};

/// How long the kernel gets to finish ending a process.
const TERMINATE_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Inspection {
    active: Option<UnitId>,
    broker: Option<WindowsBrokerObservation>,
    firewall: Option<WindowsFirewallObservation>,
    runtime: WindowsRuntimeObservation,
}

/// The install platform of a per-machine Windows installation in
/// app-supervised mode.
///
/// A transaction names a new unit from outside the store: `active.json`,
/// which the coordinator switches, then the broker's `ImagePath` and the
/// firewall's program rules, which [`super::layout`] moves one recorded
/// value at a time. The runtime is the app in the recorded user's session
/// and the daemon it supervises.
///
/// The executor owns every effect and observation on the machine, the
/// session host owns what only the update service can reach, and this type
/// decides every step from what they report.
pub struct WindowsInstallPlatform<E, S> {
    executor: E,
    session: S,
    config: WindowsInstallConfig,
    units: Vec<WindowsUnitBinding>,
    last: Option<Inspection>,
    restored: Option<RestoredRelease>,
    forced_stop: Option<UnitId>,
    waiting_for_owner: bool,
}

impl<E: WindowsInstallExecutor, S: WindowsSessionHost> WindowsInstallPlatform<E, S> {
    /// Bind a platform to its roots and to the retained units a journal may
    /// name: the active unit, and a pending transaction's candidate.
    ///
    /// # Errors
    /// Returns an error for an inexact root or a unit that is not an exact
    /// Windows unit.
    pub fn new(
        executor: E,
        session: S,
        config: WindowsInstallConfig,
        known_units: impl IntoIterator<Item = UnitRecord>,
    ) -> Result<Self, InstallPlatformError> {
        config.validate()?;
        let mut platform = Self {
            executor,
            session,
            config,
            units: Vec::new(),
            last: None,
            restored: None,
            forced_stop: None,
            waiting_for_owner: false,
        };
        for unit in known_units {
            platform.bind(&unit)?;
        }
        Ok(platform)
    }

    #[must_use]
    pub fn into_parts(self) -> (E, S) {
        (self.executor, self.session)
    }

    /// Whether the transaction stopped because the recorded user has no
    /// session to start a release in.
    ///
    /// The journal then rests at its runtime step, so a recovery run on the
    /// user's next logon finishes it, and the stable launcher keeps a manual
    /// launch on the prior until it does.
    #[must_use]
    pub fn waiting_for_owner(&self) -> bool {
        self.waiting_for_owner
    }

    fn bind(&mut self, unit: &UnitRecord) -> Result<(), InstallPlatformError> {
        if self.units.iter().any(|known| &known.unit == unit.id()) {
            return Ok(());
        }
        let binding = self.executor.bind_unit(unit)?;
        if &binding.unit != unit.id() {
            return Err(error("a Windows unit bound under another digest"));
        }
        binding.validate()?;
        self.units.push(binding);
        Ok(())
    }

    fn unit(&self, id: &UnitId) -> Result<&WindowsUnitBinding, InstallPlatformError> {
        self.units
            .iter()
            .find(|binding| &binding.unit == id)
            .ok_or_else(|| error(format!("Windows unit {} is not retained", id.as_str())))
    }

    fn record(
        &self,
        record: &PlatformTransactionRecord,
    ) -> Result<WindowsRecord, InstallPlatformError> {
        let record = WindowsRecord::decode(record)?;
        if record.units_root != self.config.units_root {
            return Err(error(
                "the Windows record was prepared for another units root",
            ));
        }
        Ok(record)
    }

    fn inspect_exact(&mut self) -> Result<Inspection, InstallPlatformError> {
        Ok(Inspection {
            active: self.executor.active_unit()?,
            broker: self.executor.observe_broker()?,
            firewall: self.executor.observe_firewall()?,
            runtime: self.executor.observe_runtime()?,
        })
    }

    /// The coordinator's coarse view: the layout and the launcher follow
    /// the active pointer, while the exact checkpoints judge what names
    /// which unit. The app's own `Run` value is the user's choice, names
    /// the stable launcher and no transaction changes it, so autostart is
    /// never part of a transaction's state.
    fn state_from(inspection: &Inspection) -> PlatformState {
        let running = !inspection.runtime.is_stopped();
        PlatformState {
            layout_unit: inspection.active.clone(),
            launcher_unit: inspection.active.clone(),
            loaded: running,
            running_unit: running.then(|| inspection.active.clone()).flatten(),
            autostart_enabled: false,
        }
    }

    fn app_path(&self, unit: &UnitId) -> String {
        self.config.member_path(unit, WINDOWS_APP_MEMBER)
    }

    fn daemon_path(&self, unit: &UnitId) -> String {
        self.config.member_path(unit, WINDOWS_DAEMON_MEMBER)
    }

    /// The running app and daemon, when exactly they run and from `unit`.
    fn runtime_of(
        &self,
        runtime: &WindowsRuntimeObservation,
        unit: &UnitId,
    ) -> Option<WindowsRuntime> {
        let (app, daemon) = runtime.pair()?;
        (same_windows_path(&app.image, &self.app_path(unit))
            && same_windows_path(&daemon.image, &self.daemon_path(unit)))
        .then_some(WindowsRuntime {
            app: app.id,
            daemon: daemon.id,
        })
    }

    /// The side of this transaction `process` runs from, if either.
    fn transaction_unit_of(
        &self,
        record: &WindowsRecord,
        process: &WindowsRunningProcess,
    ) -> Option<UnitId> {
        std::iter::once(&record.candidate)
            .chain(record.prior.as_ref())
            .find(|binding| {
                [
                    self.app_path(&binding.unit),
                    self.daemon_path(&binding.unit),
                ]
                .iter()
                .any(|path| same_windows_path(&process.image, path))
            })
            .map(|binding| binding.unit.clone())
    }

    fn runtime_matches(
        &self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        record: &WindowsRecord,
        receipt: Option<&WindowsRuntime>,
        runtime: &WindowsRuntimeObservation,
    ) -> bool {
        let Some(unit) = &expected.running_unit else {
            return runtime.is_stopped();
        };
        let Some(running) = self.runtime_of(runtime, unit) else {
            return false;
        };
        let is_prior = record
            .prior
            .as_ref()
            .is_some_and(|prior| &prior.unit == unit);
        match checkpoint {
            PlatformCheckpoint::PriorOriginal => is_prior && record.baseline == Some(running),
            PlatformCheckpoint::CandidateRuntime => {
                unit == &record.candidate.unit && receipt.is_none_or(|receipt| receipt == &running)
            }
            PlatformCheckpoint::PriorRestored => {
                is_prior
                    && record
                        .baseline
                        .is_none_or(|baseline| !baseline.shares_a_process(&running))
            }
            _ => false,
        }
    }

    fn layout_holds(record: &WindowsRecord, position: u16, inspection: &Inspection) -> bool {
        record.layout_holds(
            position,
            inspection.broker.as_ref(),
            inspection.firewall.as_ref(),
        )
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
        PlatformTransitionStates {
            prior_unloaded,
            candidate_manager,
            candidate_autostart,
            prior_autostart: prior_manager.clone(),
            prior_manager,
        }
    }

    fn require_quiescent(expected: &PlatformState) -> Result<(), InstallPlatformError> {
        if expected.loaded || expected.running_unit.is_some() {
            return Err(error("Windows checkpoint is not quiescent"));
        }
        Ok(())
    }

    /// The broker as a transaction records it. It must already run from
    /// the prior unit when there is one; only a first install may meet a
    /// broker registered from elsewhere.
    fn broker_baseline(
        &self,
        broker: Option<&WindowsBrokerObservation>,
        prior: Option<&WindowsUnitBinding>,
    ) -> Result<Option<WindowsBrokerBaseline>, InstallPlatformError> {
        let Some(broker) = broker else {
            return Ok(None);
        };
        let program = service_program(&broker.image).ok_or_else(|| {
            error(format!(
                "the SMBus broker's registered command {:?} is not one program",
                broker.image
            ))
        })?;
        if let Some(prior) = prior
            && !same_windows_path(
                program,
                &self.config.member_path(&prior.unit, WINDOWS_BROKER_MEMBER),
            )
        {
            return Err(error(
                "the SMBus broker is not registered from the active unit; adopt the \
                 installation before updating it",
            ));
        }
        Ok(Some(WindowsBrokerBaseline {
            program: program.to_owned(),
            running: broker.running,
        }))
    }

    fn prove_image(
        &mut self,
        process: WindowsProcessId,
        path: &str,
        expected: &WindowsFileIdentity,
    ) -> Result<(), InstallPlatformError> {
        let image = self
            .executor
            .process_image(process)?
            .ok_or_else(|| error(format!("process {} exited during its proof", process.pid)))?;
        if !same_windows_path(&image.path, path) {
            return Err(error(format!(
                "process {} runs {}, not {path}",
                process.pid, image.path
            )));
        }
        if image.identity != *expected {
            return Err(error(format!(
                "the image of process {} is not the unit's {path}",
                process.pid
            )));
        }
        Ok(())
    }

    /// Prove `running` is `binding`'s release: the exact image files of
    /// both processes and the broker, and a local API that reports its
    /// version.
    ///
    /// Bringing the broker up here as well covers a release this step did
    /// not start: at sign-in the launcher can start the prior before
    /// recovery reaches its runtime step, which then has nothing to start.
    fn prove_runtime(
        &mut self,
        record: &WindowsRecord,
        binding: &WindowsUnitBinding,
        running: &WindowsRuntime,
    ) -> Result<(), InstallPlatformError> {
        self.start_broker(record, binding)?;
        let daemon = self.daemon_path(&binding.unit);
        self.prove_image(running.daemon, &daemon, &binding.daemon)?;
        let app = self.app_path(&binding.unit);
        self.prove_image(running.app, &app, &binding.app)?;
        verify_http(
            self.executor.http_get("/health", MAX_HTTP_RESPONSE_BYTES)?,
            self.executor
                .http_get("/api/v1/system", MAX_HTTP_RESPONSE_BYTES)?,
            &binding.version,
        )
    }

    /// Start the broker `binding` ships when it ran before the transaction,
    /// and prove it runs that exact image.
    fn start_broker(
        &mut self,
        record: &WindowsRecord,
        binding: &WindowsUnitBinding,
    ) -> Result<(), InstallPlatformError> {
        if !record.broker.as_ref().is_some_and(|broker| broker.running) {
            return Ok(());
        }
        let process = self.executor.start_broker()?;
        let path = self
            .config
            .member_path(&binding.unit, WINDOWS_BROKER_MEMBER);
        self.prove_image(process, &path, &binding.broker)
    }

    /// Stop whatever runs: ask the app to quit, wait on the process handles
    /// for the deadline, and only then end them. A process that belongs to
    /// neither side of the transaction is never touched.
    fn stop_runtime(
        &mut self,
        record: &WindowsRecord,
        runtime: &WindowsRuntimeObservation,
    ) -> Result<(), InstallPlatformError> {
        let mut side = None;
        let mut processes = Vec::new();
        for process in runtime.processes() {
            let unit = self.transaction_unit_of(record, process).ok_or_else(|| {
                error(format!(
                    "process {} runs {}, which belongs to neither side of this transaction",
                    process.id.pid, process.image
                ))
            })?;
            side.get_or_insert(unit);
            processes.push(process.id);
        }
        if processes.is_empty() {
            return Ok(());
        }
        let quit = self.session.ask_app_to_quit()
            && self
                .executor
                .wait_for_every_exit(&processes, self.config.stop_deadline)?;
        if !quit {
            self.executor.terminate(&processes)?;
            if !self
                .executor
                .wait_for_every_exit(&processes, TERMINATE_DEADLINE)?
            {
                return Err(error("a Windows process outlived its forced stop"));
            }
            self.forced_stop = side;
        }
        Ok(())
    }
}

impl<E: WindowsInstallExecutor, S: WindowsSessionHost> InstallPlatform
    for WindowsInstallPlatform<E, S>
{
    fn inspect(&mut self) -> Result<PlatformState, InstallPlatformError> {
        let inspection = self.inspect_exact()?;
        let state = Self::state_from(&inspection);
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
            .ok_or_else(|| error("prepare requires an exact prior Windows inspection"))?;
        if inspection.active != prior.active_unit || Self::state_from(&inspection) != prior.platform
        {
            return Err(error("the Windows installation changed before preparation"));
        }
        // A unit's programs have one path, so the same unit on both sides
        // could not be told apart from a release that never stopped.
        if prior.active_unit.as_ref() == Some(candidate.id()) {
            return Err(error("the candidate is already the active unit"));
        }
        self.bind(candidate)?;
        let candidate_binding = self.unit(candidate.id())?.clone();
        let prior_binding = prior
            .active_unit
            .as_ref()
            .map(|id| self.unit(id).cloned())
            .transpose()?;
        if target.autostart_enabled != prior.platform.autostart_enabled {
            return Err(error(
                "autostart belongs to the app and the installer, not to an update",
            ));
        }
        // The launcher reads the pointer, so every commit leaves it on the
        // candidate; a target with none could never be reached.
        if target.launcher_unit.is_none() {
            return Err(error("a Windows transaction always ends on its launcher"));
        }
        let baseline = if inspection.runtime.is_stopped() {
            None
        } else {
            let running = prior
                .active_unit
                .as_ref()
                .and_then(|unit| self.runtime_of(&inspection.runtime, unit));
            Some(running.ok_or_else(|| {
                error("the running app and daemon are not exactly the active unit's")
            })?)
        };
        let broker = self.broker_baseline(inspection.broker.as_ref(), prior_binding.as_ref())?;
        if broker.as_ref().is_some_and(|broker| broker.running) && target.running_unit.is_none() {
            return Err(error(
                "a transaction that starts no release has nothing to start the SMBus broker \
                 again",
            ));
        }
        let record = WindowsRecord {
            units_root: self.config.units_root.clone(),
            candidate: candidate_binding,
            prior: prior_binding,
            baseline,
            broker,
            firewall: inspection.firewall,
        };
        Ok(PreparedPlatformTransaction {
            record: record.encode()?,
            transitions: Self::transitions(&prior.platform, target),
            layout_operation_count: LAYOUT_OPERATION_COUNT,
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
            .ok_or_else(|| error("an exact Windows checkpoint needs an inspection"))?;
        let Some(position) = position_at(checkpoint, layout_operation_index) else {
            return Ok(false);
        };
        Ok(Self::layout_holds(&record, position, inspection)
            && self.runtime_matches(
                checkpoint,
                expected,
                &record,
                receipt.as_ref(),
                &inspection.runtime,
            ))
    }

    fn capture_candidate_owner_receipt(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<PlatformOwnerReceipt, InstallPlatformError> {
        let record = self.record(platform_record)?;
        let inspection = self.inspect_exact()?;
        let running = expected
            .running_unit
            .as_ref()
            .filter(|unit| *unit == &record.candidate.unit)
            .and_then(|unit| self.runtime_of(&inspection.runtime, unit))
            .filter(|_| {
                self.runtime_matches(
                    PlatformCheckpoint::CandidateRuntime,
                    expected,
                    &record,
                    None,
                    &inspection.runtime,
                )
            })
            .ok_or_else(|| error("the running app and daemon are not the candidate's"))?;
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
        if layout_operation_count != LAYOUT_OPERATION_COUNT
            || transitions != &Self::transitions(prior, target)
            || target.layout_unit.as_ref() != Some(&record.candidate.unit)
            || prior.layout_unit.is_some() != record.prior.is_some()
        {
            return Err(error(
                "Windows transaction plan differs from its exact layout",
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
                "candidate unit differs from the prepared Windows record",
            ));
        }
        let inspection = self.inspect_exact()?;
        let running = prior
            .active_unit
            .as_ref()
            .and_then(|unit| self.runtime_of(&inspection.runtime, unit));
        if inspection.active != prior.active_unit
            || Self::state_from(&inspection) != prior.platform
            || !Self::layout_holds(&record, 0, &inspection)
            || running != record.baseline
        {
            return Err(error("the Windows installation drifted before unload"));
        }
        self.last = Some(inspection);
        Ok(())
    }

    fn wait_for_guard_release(
        &mut self,
        unloaded: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        // The daemon's instance guard dies with its process, and the
        // coordinator proves the runtime stopped on either side of this.
        self.record(platform_record)?;
        Self::require_quiescent(unloaded)?;
        self.last = None;
        Ok(())
    }

    fn install_launcher(
        &mut self,
        _checkpoint: PlatformCheckpoint,
        _unit: Option<&UnitId>,
        _platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        // The stable launcher belongs to the installer and reads the active
        // pointer, so no transaction step changes it.
        Err(error(
            "the stable Windows launcher does not change within a transaction",
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
        let (from, to, expected_unit) = match checkpoint {
            PlatformCheckpoint::CandidateLayout => (
                operation_index,
                operation_index + 1,
                Some(&record.candidate.unit),
            ),
            PlatformCheckpoint::PriorLayoutRestored => (
                operation_index + 1,
                operation_index,
                record.prior.as_ref().map(|prior| &prior.unit),
            ),
            _ => return Err(error("Windows layout effect lacks a layout checkpoint")),
        };
        if unit != expected_unit {
            return Err(error("Windows layout effect names the wrong unit"));
        }
        let step = layout::step(operation_index)?;
        let broker = self.executor.observe_broker()?;
        let firewall = self.executor.observe_firewall()?;
        if !record.layout_holds(from, broker.as_ref(), firewall.as_ref()) {
            return Err(error(format!(
                "the Windows layout drifted before step {operation_index}"
            )));
        }
        match step {
            LayoutStep::Broker => {
                let program = record
                    .broker_program_at(to)
                    .ok_or_else(|| error("no SMBus broker is recorded for this step"))?;
                self.executor.switch_broker(&program)?;
            }
            LayoutStep::Rule(rule) => {
                let baseline = record
                    .firewall
                    .as_ref()
                    .ok_or_else(|| error("no firewall rule is recorded for this step"))?;
                let program = record.rule_program_at(baseline, rule, to);
                self.executor.set_firewall_rule(rule, program.as_deref())?;
            }
        }
        self.last = None;
        Ok(())
    }

    fn reload_manager(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        // The service control manager reads a service's image when it starts
        // it; there is no separate reload to run.
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
        Err(error("no Windows transaction changes autostart"))
    }

    fn restore_runtime(
        &mut self,
        expected: &PlatformState,
        platform_record: &PlatformTransactionRecord,
        _candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<(), InstallPlatformError> {
        let record = self.record(platform_record)?;
        match &expected.running_unit {
            None => {
                let runtime = self.executor.observe_runtime()?;
                self.stop_runtime(&record, &runtime)?;
            }
            Some(unit) => {
                let binding = record
                    .binding(unit)
                    .cloned()
                    .ok_or_else(|| error("Windows runtime target is outside this transaction"))?;
                self.start_broker(&record, &binding)?;
                let app = self.app_path(unit);
                match self.session.start_app(&app, &WINDOWS_APP_ARGUMENTS)? {
                    WindowsAppStart::NoSession => {
                        self.waiting_for_owner = true;
                        return Err(error(
                            "the installation's user has no session to start the release in",
                        ));
                    }
                    WindowsAppStart::Started(started) => {
                        self.waiting_for_owner = false;
                        self.executor
                            .wait_for_daemon(started.id, self.config.start_deadline)?;
                    }
                }
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
            return if inspection.runtime.is_stopped() {
                Ok(())
            } else {
                Err(error("a Windows process runs where none is expected"))
            };
        };
        if !self.runtime_matches(
            checkpoint,
            expected,
            &record,
            receipt.as_ref(),
            &inspection.runtime,
        ) {
            return Err(error(
                "the running app and daemon are not the ones this proof expects",
            ));
        }
        let running = self
            .runtime_of(&inspection.runtime, unit)
            .ok_or_else(|| error("the Windows runtime is not running for its proof"))?;
        let binding = record
            .binding(unit)
            .cloned()
            .ok_or_else(|| error("Windows proof target is outside this transaction"))?;
        self.prove_runtime(&record, &binding, &running)?;
        if checkpoint == PlatformCheckpoint::CandidateRuntime {
            let window = self.config.probation;
            if let Some(exited) = self
                .executor
                .wait_for_any_exit(&[running.app, running.daemon], window)?
            {
                return Err(error(format!(
                    "candidate failed probation: process {} exited inside its {} s window",
                    exited.pid,
                    window.as_secs()
                )));
            }
            self.prove_runtime(&record, &binding, &running)
                .map_err(|source| {
                    error(format!(
                        "candidate failed the proof at the end of its {} s probation window: \
                         {source}",
                        window.as_secs()
                    ))
                })?;
        }
        let after = self.inspect_exact()?;
        if self.runtime_of(&after.runtime, unit) != Some(running) {
            return Err(error("the Windows runtime changed during its proof"));
        }
        if checkpoint == PlatformCheckpoint::PriorRestored {
            self.restored = Some(RestoredRelease {
                unit: binding.unit,
                version: binding.version,
                instance: format!("pid:{}:{}", running.daemon.pid, running.daemon.created),
                process_id: running.daemon.pid,
                executable_sha256: binding.daemon.sha256,
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
        let runtime = self.executor.observe_runtime()?;
        if runtime.is_stopped() {
            return Ok(false);
        }
        self.stop_runtime(&record, &runtime)?;
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
            .ok_or_else(|| error("an exact Windows checkpoint needs an inspection"))?;
        Ok(position_at(checkpoint, layout_operation_index)
            .is_some_and(|position| Self::layout_holds(&record, position, inspection)))
    }

    fn matches_untouched_prior(
        &mut self,
        _prior: &PlatformState,
        platform_record: &PlatformTransactionRecord,
        restarted: bool,
    ) -> Result<bool, InstallPlatformError> {
        let record = self.record(platform_record)?;
        let inspection = self.inspect_exact()?;
        let running = record
            .prior
            .as_ref()
            .and_then(|prior| self.runtime_of(&inspection.runtime, &prior.unit));
        let untouched = Self::layout_holds(&record, 0, &inspection)
            && (!restarted || running.is_some_and(|running| record.baseline != Some(running)));
        self.last = Some(inspection);
        Ok(untouched)
    }

    fn take_restored_release(&mut self) -> Option<RestoredRelease> {
        self.restored.take()
    }

    fn take_forced_stop(&mut self) -> Option<UnitId> {
        self.forced_stop.take()
    }
}
