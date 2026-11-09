use crate::windows::BootstrapSlot;
use crate::{InstallPlatformError, UnitId};

use super::{PendingTransaction, WindowsSetupHost, run_windows_setup};

fn unit(seed: char) -> UnitId {
    UnitId::new(seed.to_string().repeat(64)).expect("unit")
}

/// A machine that loses its process after a chosen number of steps.
#[derive(Debug, Default)]
struct Machine {
    pending: Option<PendingTransaction>,
    staged: bool,
    active: Option<UnitId>,
    recorded: bool,
    coordinator: bool,
    service: Option<BootstrapSlot>,
    registrations: Vec<BootstrapSlot>,
    configured: bool,
    slots_written: bool,
    bad_slot: Option<BootstrapSlot>,
    collected: bool,
    transaction_fails: bool,
    /// Steps left before the run dies.
    steps_left: Option<usize>,
    log: Vec<&'static str>,
}

impl Machine {
    fn step(&mut self, name: &'static str) -> Result<(), InstallPlatformError> {
        if let Some(left) = &mut self.steps_left {
            if *left == 0 {
                return Err(InstallPlatformError::new("killed"));
            }
            *left -= 1;
        }
        self.log.push(name);
        Ok(())
    }
}

impl WindowsSetupHost for Machine {
    fn pending_transaction(&mut self) -> Result<Option<PendingTransaction>, InstallPlatformError> {
        Ok(self.pending)
    }

    fn finish_transaction(&mut self) -> Result<(), InstallPlatformError> {
        self.step("finish")?;
        self.pending = None;
        Ok(())
    }

    fn stage_candidate(&mut self) -> Result<(), InstallPlatformError> {
        self.step("stage")?;
        self.staged = true;
        Ok(())
    }

    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError> {
        Ok(self.active.clone())
    }

    fn install_candidate(&mut self) -> Result<(), InstallPlatformError> {
        self.step("install")?;
        assert!(self.staged, "a transaction installs a staged unit");
        if self.transaction_fails {
            return Err(InstallPlatformError::new("rolled back"));
        }
        self.active = Some(unit('c'));
        Ok(())
    }

    fn installation_recorded(&mut self) -> Result<bool, InstallPlatformError> {
        Ok(self.recorded)
    }

    fn record_installation(&mut self) -> Result<(), InstallPlatformError> {
        self.step("record")?;
        self.recorded = true;
        Ok(())
    }

    fn update_service_slot(&mut self) -> Result<Option<BootstrapSlot>, InstallPlatformError> {
        Ok(self.service)
    }

    fn prepare_coordinator_directory(&mut self) -> Result<(), InstallPlatformError> {
        self.step("coordinator")?;
        assert!(self.recorded, "the directory is the recorded user's");
        self.coordinator = true;
        Ok(())
    }

    fn write_bootstrap_slots(&mut self) -> Result<(), InstallPlatformError> {
        self.step("write-slots")?;
        self.slots_written = true;
        Ok(())
    }

    fn probe_bootstrap(&mut self, slot: BootstrapSlot) -> Result<(), InstallPlatformError> {
        self.step("probe")?;
        assert!(self.slots_written, "a probe needs its slot");
        if self.bad_slot == Some(slot) {
            return Err(InstallPlatformError::new("did not load"));
        }
        Ok(())
    }

    fn register_update_service(&mut self, slot: BootstrapSlot) -> Result<(), InstallPlatformError> {
        self.step("register")?;
        assert!(self.recorded, "the bootstrap reads the recorded user");
        self.service = Some(slot);
        self.step("configure")?;
        self.configured = true;
        self.registrations.push(slot);
        Ok(())
    }

    fn remove_update_service(&mut self) -> Result<(), InstallPlatformError> {
        self.step("remove-service")?;
        self.service = None;
        Ok(())
    }

    fn collect_units(&mut self) -> Result<Vec<String>, InstallPlatformError> {
        self.step("collect")?;
        self.collected = true;
        Ok(Vec::new())
    }
}

fn run(machine: &mut Machine, managed: bool) -> Result<Vec<String>, InstallPlatformError> {
    run_windows_setup(machine, &unit('c'), managed)
}

#[test]
fn a_first_install_ends_active_recorded_and_clean() {
    let mut machine = Machine::default();
    run(&mut machine, false).expect("setup");

    assert_eq!(machine.active, Some(unit('c')));
    assert!(machine.recorded);
    assert_eq!(machine.service, None);
    assert!(!machine.coordinator);
    assert!(machine.collected);
    assert_eq!(machine.log, ["stage", "install", "record", "collect"]);
}

#[test]
fn a_managed_install_proves_both_slots_before_it_registers_one() {
    let mut machine = Machine::default();
    run(&mut machine, true).expect("setup");

    assert_eq!(machine.service, Some(BootstrapSlot::A));
    assert_eq!(
        machine.log,
        [
            "stage",
            "install",
            "record",
            "coordinator",
            "write-slots",
            "probe",
            "probe",
            "register",
            "configure",
            "collect"
        ]
    );
}

#[test]
fn a_slot_that_does_not_load_is_never_registered() {
    for slot in [BootstrapSlot::A, BootstrapSlot::B] {
        let mut machine = Machine {
            bad_slot: Some(slot),
            ..Machine::default()
        };
        run(&mut machine, true).expect_err("a bootstrap that does not load");

        assert_eq!(machine.service, None);
        assert_eq!(machine.active, Some(unit('c')), "the release stays in");
    }
}

#[test]
fn a_run_killed_after_any_step_is_finished_by_the_next() {
    for managed in [false, true] {
        for steps in 0..16 {
            let mut machine = Machine {
                steps_left: Some(steps),
                ..Machine::default()
            };
            let first = run(&mut machine, managed);
            machine.steps_left = None;
            machine.log.clear();

            run(&mut machine, managed).expect("the next run finishes");

            assert!(first.is_err() || steps >= 4, "{steps}");
            assert_eq!(machine.active, Some(unit('c')), "{steps}");
            assert!(machine.recorded, "{steps}");
            assert!(machine.collected, "{steps}");
            assert_eq!(machine.service.is_some(), managed, "{steps}");
            assert_eq!(machine.coordinator, managed, "{steps}");
            assert_eq!(machine.configured, managed, "{steps}");
        }
    }
}

#[test]
fn a_run_of_an_installed_unit_touches_no_transaction() {
    let mut machine = Machine {
        active: Some(unit('c')),
        recorded: true,
        ..Machine::default()
    };
    run(&mut machine, false).expect("setup");

    assert_eq!(machine.log, ["stage", "collect"]);
}

#[test]
fn an_open_installer_transaction_ends_before_this_one_begins() {
    let mut machine = Machine {
        pending: Some(PendingTransaction::StartsNothing),
        active: Some(unit('a')),
        ..Machine::default()
    };
    run(&mut machine, false).expect("setup");

    assert_eq!(machine.log[..3], ["finish", "stage", "install"]);
    assert_eq!(machine.active, Some(unit('c')));
}

#[test]
fn an_update_that_needs_its_owner_is_never_overrun() {
    let mut machine = Machine {
        pending: Some(PendingTransaction::StartsARelease),
        ..Machine::default()
    };
    let failure = run(&mut machine, true).expect_err("the update comes first");

    assert!(failure.to_string().contains("run this installer again"));
    assert!(machine.log.is_empty(), "{:?}", machine.log);
}

#[test]
fn a_transaction_that_rolled_back_stops_the_run() {
    let mut machine = Machine {
        transaction_fails: true,
        ..Machine::default()
    };
    run(&mut machine, true).expect_err("rolled back");

    assert!(!machine.recorded, "nothing claims a release that is not in");
    assert_eq!(machine.service, None);
}

#[test]
fn a_record_is_written_once_and_kept_after_that() {
    let mut machine = Machine {
        recorded: true,
        ..Machine::default()
    };
    run(&mut machine, false).expect("setup");

    assert!(!machine.log.contains(&"record"));
}

#[test]
fn a_registered_service_stays_on_the_slot_an_update_moved_it_to() {
    let mut machine = Machine {
        active: Some(unit('a')),
        recorded: true,
        service: Some(BootstrapSlot::B),
        ..Machine::default()
    };
    run(&mut machine, true).expect("setup");

    assert_eq!(machine.service, Some(BootstrapSlot::B));
    assert_eq!(machine.registrations, [BootstrapSlot::B]);
    assert!(!machine.log.contains(&"write-slots"), "{:?}", machine.log);
}

#[test]
fn a_build_without_the_updater_takes_the_service_away() {
    let mut machine = Machine {
        active: Some(unit('a')),
        recorded: true,
        service: Some(BootstrapSlot::A),
        ..Machine::default()
    };
    run(&mut machine, false).expect("setup");

    assert_eq!(machine.service, None);
}
