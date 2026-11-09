//! The layout steps of one Windows transaction.
//!
//! Units never move, and the coordinator switches `active.json` itself, so
//! the layout holds only what names a unit from outside it:
//!
//! | Position | Update service | Broker `ImagePath` | Daemon rule | App rule  |
//! | -------- | -------------- | ------------------ | ----------- | --------- |
//! | 0        | proven slot    | prior              | prior       | prior     |
//! | 1        | other slot     | prior              | prior       | prior     |
//! | 2        | other slot     | candidate          | prior       | prior     |
//! | 3        | other slot     | candidate          | candidate   | prior     |
//! | 4        | other slot     | candidate          | candidate   | candidate |
//!
//! Each step changes one recorded value, so a step repeated after a crash
//! is recognized as done. A compensation writes back the exact value the
//! record retained, or removes a rule that did not exist.
//!
//! The bootstrap goes first because it is the step a release can simply
//! refuse: a new one has to prove it loads before `ImagePath` moves, and
//! that refusal should cost nothing. A release that ships the bootstrap the
//! active slot already holds leaves the service alone.
//!
//! Positions never depend on whether the broker runs. Windows starts it at
//! boot from whatever `ImagePath` names, and its failure actions restart it
//! on their own, so its run state can change under an open transaction at
//! any moment. The switch stops the broker itself, and the runtime step
//! that starts the release starts the broker first.

use super::machine_layout::{BootstrapSlot, WINDOWS_BROKER_PROGRAM};
use super::model::{
    WindowsFirewallObservation, WindowsFirewallRule, WindowsLayoutObservation, error,
    same_windows_path, service_program,
};
use super::record::WindowsRecord;
use crate::{InstallPlatformError, PlatformCheckpoint};

/// The layout operations of one transaction.
pub(super) const LAYOUT_OPERATION_COUNT: u16 = 4;

/// The one effect that moves a transaction between two adjacent positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LayoutStep {
    Bootstrap,
    Broker,
    Rule(WindowsFirewallRule),
}

const STEPS: [LayoutStep; LAYOUT_OPERATION_COUNT as usize] = [
    LayoutStep::Bootstrap,
    LayoutStep::Broker,
    LayoutStep::Rule(WindowsFirewallRule::Daemon),
    LayoutStep::Rule(WindowsFirewallRule::App),
];

/// The position the coordinator expects at `checkpoint`.
///
/// Only the layout checkpoints move with the operation cursor; every
/// checkpoint before the layout holds the prior's values, and every one
/// after it the candidate's, whatever the cursor says.
pub(super) fn position_at(checkpoint: PlatformCheckpoint, index: u16) -> Option<u16> {
    match checkpoint {
        PlatformCheckpoint::PriorOriginal
        | PlatformCheckpoint::PriorUnloaded
        | PlatformCheckpoint::PriorManager
        | PlatformCheckpoint::PriorAutostart
        | PlatformCheckpoint::PriorRestored => Some(0),
        PlatformCheckpoint::CandidateLayout | PlatformCheckpoint::PriorLayoutRestored => {
            (index <= LAYOUT_OPERATION_COUNT).then_some(index)
        }
        PlatformCheckpoint::CandidateLauncher
        | PlatformCheckpoint::CandidateActive
        | PlatformCheckpoint::CandidateManager
        | PlatformCheckpoint::CandidateAutostart
        | PlatformCheckpoint::CandidateRuntime
        | PlatformCheckpoint::PriorActiveRestored
        | PlatformCheckpoint::PriorLauncherRestored => Some(LAYOUT_OPERATION_COUNT),
    }
}

/// The step layout operation `index` takes, in either direction.
pub(super) fn step(index: u16) -> Result<LayoutStep, InstallPlatformError> {
    STEPS
        .get(usize::from(index))
        .copied()
        .ok_or_else(|| error("Windows layout cursor exceeds its operations"))
}

fn applied(step: LayoutStep, position: u16) -> bool {
    STEPS
        .iter()
        .take(usize::from(position))
        .any(|done| *done == step)
}

impl WindowsRecord {
    /// The slot the update service runs from at `position`, when this
    /// transaction replaces its bootstrap.
    pub(super) fn bootstrap_slot_at(&self, position: u16) -> Option<BootstrapSlot> {
        self.bootstrap.map(|proven| {
            if applied(LayoutStep::Bootstrap, position) {
                proven.other()
            } else {
                proven
            }
        })
    }

    /// The broker program this transaction registers at `position`, when a
    /// broker is registered at all.
    pub(super) fn broker_program_at(&self, position: u16) -> Option<String> {
        let baseline = self.broker.as_ref()?;
        Some(if applied(LayoutStep::Broker, position) {
            self.candidate_path(WINDOWS_BROKER_PROGRAM)
        } else {
            baseline.program.clone()
        })
    }

    /// The program `rule` allows at `position`, given the rules the record
    /// retained; `None` is a rule that does not exist.
    pub(super) fn rule_program_at(
        &self,
        baseline: &WindowsFirewallObservation,
        rule: WindowsFirewallRule,
        position: u16,
    ) -> Option<String> {
        if applied(LayoutStep::Rule(rule), position) {
            Some(self.candidate_rule(rule))
        } else {
            baseline.program(rule).map(str::to_owned)
        }
    }

    /// Whether the observed broker registration and firewall rules are
    /// exactly `position`.
    ///
    /// A firewall that answered when the transaction was prepared and is
    /// gone now proves no position at all. Its stored rules outlive it, so
    /// treating it as matched would skip a compensation for good; the
    /// transaction rests until it answers again.
    pub(super) fn layout_holds(&self, position: u16, observed: &WindowsLayoutObservation) -> bool {
        let bootstrap_holds = self
            .bootstrap_slot_at(position)
            .is_none_or(|expected| observed.bootstrap == Some(expected));
        let broker_holds = self.broker_program_at(position).is_none_or(|expected| {
            observed.broker.as_ref().is_some_and(|broker| {
                service_program(&broker.image)
                    .is_some_and(|actual| same_windows_path(actual, &expected))
            })
        });
        let rule_holds = |rule| {
            let Some(baseline) = self.firewall.as_ref() else {
                return true;
            };
            let Some(firewall) = observed.firewall.as_ref() else {
                return false;
            };
            match (
                self.rule_program_at(baseline, rule, position),
                firewall.program(rule),
            ) {
                (None, None) => true,
                (Some(expected), Some(actual)) => same_windows_path(actual, &expected),
                _ => false,
            }
        };
        bootstrap_holds
            && broker_holds
            && rule_holds(WindowsFirewallRule::Daemon)
            && rule_holds(WindowsFirewallRule::App)
    }
}

#[cfg(test)]
mod tests {
    use super::{LAYOUT_OPERATION_COUNT, LayoutStep, position_at, step};
    use crate::PlatformCheckpoint;
    use crate::windows::model::{
        WindowsBrokerObservation, WindowsFileIdentity, WindowsFirewallObservation,
        WindowsLayoutObservation, WindowsUnitBinding,
    };
    use crate::windows::record::{WindowsBrokerBaseline, WindowsRecord};
    use crate::windows::{
        BootstrapSlot, WINDOWS_APP_PROGRAM, WINDOWS_BROKER_PROGRAM, WINDOWS_DAEMON_PROGRAM,
        WindowsFirewallRule,
    };

    const COUNT: u16 = LAYOUT_OPERATION_COUNT;
    const ROOT: &str = r"C:\H\units";

    /// Where the unit seeded `seed` keeps `member`.
    fn path(seed: char, member: &str) -> String {
        let unit = crate::UnitId::new(seed.to_string().repeat(64)).expect("unit");
        crate::windows::model::member_path(ROOT, &unit, member)
    }

    fn broker_of(seed: char) -> String {
        path(seed, WINDOWS_BROKER_PROGRAM)
    }

    /// The firewall when each rule allows the seeded unit's program.
    fn rules(daemon: char, app: char) -> WindowsFirewallObservation {
        WindowsFirewallObservation {
            daemon: Some(path(daemon, WINDOWS_DAEMON_PROGRAM)),
            app: Some(path(app, WINDOWS_APP_PROGRAM)),
        }
    }

    fn binding(seed: char) -> WindowsUnitBinding {
        let identity = WindowsFileIdentity {
            volume_serial: 1,
            file_id: "0".repeat(32),
            sha256: "0".repeat(64),
        };
        WindowsUnitBinding {
            unit: crate::UnitId::new(seed.to_string().repeat(64)).expect("unit"),
            version: "0.9.0".to_owned(),
            app: identity.clone(),
            daemon: identity.clone(),
            broker: identity.clone(),
            bootstrap: identity,
        }
    }

    fn record() -> WindowsRecord {
        WindowsRecord {
            units_root: ROOT.to_owned(),
            candidate: binding('b'),
            prior: Some(binding('a')),
            baseline: None,
            bootstrap: Some(BootstrapSlot::A),
            broker: Some(WindowsBrokerBaseline {
                program: broker_of('a'),
                running: true,
            }),
            firewall: Some(rules('a', 'a')),
        }
    }

    fn observed(
        slot: BootstrapSlot,
        broker: &str,
        firewall: WindowsFirewallObservation,
    ) -> WindowsLayoutObservation {
        WindowsLayoutObservation {
            bootstrap: Some(slot),
            broker: Some(WindowsBrokerObservation {
                image: format!("\"{broker}\""),
                running: false,
            }),
            firewall: Some(firewall),
        }
    }

    /// Every position's exact observation, in order.
    fn positions() -> [WindowsLayoutObservation; 5] {
        let (proven, other) = (BootstrapSlot::A, BootstrapSlot::B);
        [
            observed(proven, &broker_of('a'), rules('a', 'a')),
            observed(other, &broker_of('a'), rules('a', 'a')),
            observed(other, &broker_of('b'), rules('a', 'a')),
            observed(other, &broker_of('b'), rules('b', 'a')),
            observed(other, &broker_of('b'), rules('b', 'b')),
        ]
    }

    #[test]
    fn every_position_is_told_apart() {
        let record = record();
        for (position, observation) in (0_u16..).zip(positions()) {
            for candidate in 0..=COUNT {
                assert_eq!(
                    record.layout_holds(candidate, &observation),
                    candidate == position,
                    "observation of {position} judged at {candidate}"
                );
            }
        }
    }

    #[test]
    fn whether_the_broker_runs_never_changes_the_position() {
        let record = record();
        let mut observation = positions()[2].clone();
        for running in [false, true] {
            if let Some(broker) = &mut observation.broker {
                broker.running = running;
            }
            assert!(record.layout_holds(2, &observation));
        }
    }

    #[test]
    fn a_broker_that_vanished_is_drift() {
        let record = record();
        let mut observation = positions()[0].clone();
        observation.broker = None;
        assert!(!record.layout_holds(0, &observation));
    }

    #[test]
    fn an_update_service_that_vanished_is_drift() {
        let record = record();
        let mut observation = positions()[0].clone();
        observation.bootstrap = None;
        assert!(!record.layout_holds(0, &observation));
    }

    #[test]
    fn a_broker_line_that_is_not_one_program_is_drift() {
        let record = record();
        let mut observation = positions()[0].clone();
        if let Some(broker) = &mut observation.broker {
            broker.image = format!("\"{}\" --anything", broker_of('a'));
        }
        assert!(!record.layout_holds(0, &observation));
    }

    #[test]
    fn a_firewall_that_stopped_proves_no_position() {
        let record = record();
        for (position, mut observation) in (0_u16..).zip(positions()) {
            observation.firewall = None;
            assert!(!record.layout_holds(position, &observation));
        }
    }

    #[test]
    fn a_rule_that_did_not_exist_must_be_gone_at_its_prior_positions() {
        let mut record = record();
        let absent = WindowsFirewallObservation {
            daemon: None,
            ..rules('a', 'a')
        };
        record.firewall = Some(absent.clone());
        let mut observation = positions()[0].clone();
        observation.firewall = Some(absent);
        assert!(record.layout_holds(0, &observation));
        observation.firewall = Some(rules('a', 'a'));
        assert!(!record.layout_holds(0, &observation));
    }

    #[test]
    fn what_was_never_managed_holds_everywhere() {
        let mut record = record();
        record.bootstrap = None;
        record.broker = None;
        record.firewall = None;
        let nothing = WindowsLayoutObservation {
            bootstrap: None,
            broker: None,
            firewall: None,
        };
        for position in 0..=COUNT {
            assert!(record.layout_holds(position, &nothing));
            assert_eq!(record.bootstrap_slot_at(position), None);
            assert_eq!(record.broker_program_at(position), None);
        }
    }

    #[test]
    fn checkpoints_outside_the_layout_ignore_the_cursor() {
        assert_eq!(
            position_at(PlatformCheckpoint::PriorUnloaded, COUNT),
            Some(0)
        );
        assert_eq!(
            position_at(PlatformCheckpoint::CandidateLauncher, 1),
            Some(COUNT)
        );
        assert_eq!(position_at(PlatformCheckpoint::CandidateLayout, 2), Some(2));
        assert_eq!(
            position_at(PlatformCheckpoint::PriorLayoutRestored, COUNT + 1),
            None
        );
    }

    #[test]
    fn the_bootstrap_goes_first_and_the_app_rule_last() {
        assert_eq!(step(0).expect("first"), LayoutStep::Bootstrap);
        assert_eq!(step(1).expect("second"), LayoutStep::Broker);
        assert_eq!(
            step(COUNT - 1).expect("last"),
            LayoutStep::Rule(WindowsFirewallRule::App)
        );
        assert!(step(COUNT).is_err());
    }
}
