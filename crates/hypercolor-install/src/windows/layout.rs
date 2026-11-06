//! The layout steps of one Windows transaction.
//!
//! Units never move, and the coordinator switches `active.json` itself, so
//! the layout holds only what names a unit from outside it:
//!
//! | Position | Broker `ImagePath` | Daemon rule | App rule  |
//! | -------- | ------------------ | ----------- | --------- |
//! | 0        | prior              | prior       | prior     |
//! | 1        | candidate          | prior       | prior     |
//! | 2        | candidate          | candidate   | prior     |
//! | 3        | candidate          | candidate   | candidate |
//!
//! Each step changes one recorded value, so a step repeated after a crash
//! is recognized as done. A compensation writes back the exact value the
//! record retained, or removes a rule that did not exist.
//!
//! Positions never depend on whether the broker runs. Windows starts it at
//! boot from whatever `ImagePath` names, and its failure actions restart it
//! on their own, so its run state can change under an open transaction at
//! any moment. The switch stops the broker itself, and the runtime step
//! that starts the release starts the broker first.

use super::model::{
    WINDOWS_BROKER_MEMBER, WindowsBrokerObservation, WindowsFirewallObservation,
    WindowsFirewallRule, error, same_windows_path, service_program,
};
use super::record::WindowsRecord;
use crate::{InstallPlatformError, PlatformCheckpoint};

/// The layout operations of one transaction.
pub(super) const LAYOUT_OPERATION_COUNT: u16 = 3;

/// The one effect that moves a transaction between two adjacent positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LayoutStep {
    Broker,
    Rule(WindowsFirewallRule),
}

const STEPS: [LayoutStep; LAYOUT_OPERATION_COUNT as usize] = [
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
    /// The broker program this transaction registers at `position`, when a
    /// broker is registered at all.
    pub(super) fn broker_program_at(&self, position: u16) -> Option<String> {
        let baseline = self.broker.as_ref()?;
        Some(if applied(LayoutStep::Broker, position) {
            self.candidate_path(WINDOWS_BROKER_MEMBER)
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
    /// A firewall that was running when the transaction was prepared and
    /// has stopped since enforces nothing, so it is no drift; it is judged
    /// again whenever it answers.
    pub(super) fn layout_holds(
        &self,
        position: u16,
        broker: Option<&WindowsBrokerObservation>,
        firewall: Option<&WindowsFirewallObservation>,
    ) -> bool {
        let broker_holds = self.broker_program_at(position).is_none_or(|expected| {
            broker.is_some_and(|broker| {
                service_program(&broker.image)
                    .is_some_and(|actual| same_windows_path(actual, &expected))
            })
        });
        let rule_holds = |rule| {
            let (Some(baseline), Some(firewall)) = (self.firewall.as_ref(), firewall) else {
                return true;
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
        broker_holds
            && rule_holds(WindowsFirewallRule::Daemon)
            && rule_holds(WindowsFirewallRule::App)
    }
}

#[cfg(test)]
mod tests {
    use super::{LAYOUT_OPERATION_COUNT, LayoutStep, position_at, step};
    use crate::PlatformCheckpoint;
    use crate::windows::WindowsFirewallRule;
    use crate::windows::model::{
        WINDOWS_APP_MEMBER, WINDOWS_BROKER_MEMBER, WINDOWS_DAEMON_MEMBER, WindowsBrokerObservation,
        WindowsFileIdentity, WindowsFirewallObservation, WindowsUnitBinding,
    };
    use crate::windows::record::{WindowsBrokerBaseline, WindowsRecord};

    const COUNT: u16 = LAYOUT_OPERATION_COUNT;
    const ROOT: &str = r"C:\H\units";

    /// Where the unit seeded `seed` keeps `member`.
    fn path(seed: char, member: &str) -> String {
        let unit = crate::UnitId::new(seed.to_string().repeat(64)).expect("unit");
        crate::windows::model::member_path(ROOT, &unit, member)
    }

    fn broker_of(seed: char) -> String {
        path(seed, WINDOWS_BROKER_MEMBER)
    }

    /// The firewall when each rule allows the seeded unit's program.
    fn rules(daemon: char, app: char) -> WindowsFirewallObservation {
        WindowsFirewallObservation {
            daemon: Some(path(daemon, WINDOWS_DAEMON_MEMBER)),
            app: Some(path(app, WINDOWS_APP_MEMBER)),
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
            broker: identity,
        }
    }

    fn record() -> WindowsRecord {
        WindowsRecord {
            units_root: ROOT.to_owned(),
            candidate: binding('b'),
            prior: Some(binding('a')),
            baseline: None,
            broker: Some(WindowsBrokerBaseline {
                program: broker_of('a'),
                running: true,
            }),
            firewall: Some(rules('a', 'a')),
        }
    }

    fn broker(program: &str) -> WindowsBrokerObservation {
        WindowsBrokerObservation {
            image: format!("\"{program}\""),
            running: false,
        }
    }

    /// Every position's exact observation, in order.
    fn positions() -> [(WindowsBrokerObservation, WindowsFirewallObservation); 4] {
        [
            (broker(&broker_of('a')), rules('a', 'a')),
            (broker(&broker_of('b')), rules('a', 'a')),
            (broker(&broker_of('b')), rules('b', 'a')),
            (broker(&broker_of('b')), rules('b', 'b')),
        ]
    }

    #[test]
    fn every_position_is_told_apart() {
        let record = record();
        for (position, (broker, firewall)) in (0_u16..).zip(positions()) {
            for candidate in 0..=COUNT {
                assert_eq!(
                    record.layout_holds(candidate, Some(&broker), Some(&firewall)),
                    candidate == position,
                    "observation of {position} judged at {candidate}"
                );
            }
        }
    }

    #[test]
    fn whether_the_broker_runs_never_changes_the_position() {
        let record = record();
        let (mut broker, firewall) = positions()[1].clone();
        for running in [false, true] {
            broker.running = running;
            assert!(record.layout_holds(1, Some(&broker), Some(&firewall)));
        }
    }

    #[test]
    fn a_broker_that_vanished_is_drift() {
        let record = record();
        let (_, firewall) = positions()[0].clone();
        assert!(!record.layout_holds(0, None, Some(&firewall)));
    }

    #[test]
    fn a_broker_line_that_is_not_one_program_is_drift() {
        let record = record();
        let (mut broker, firewall) = positions()[0].clone();
        broker.image = format!("\"{}\" --anything", broker_of('a'));
        assert!(!record.layout_holds(0, Some(&broker), Some(&firewall)));
    }

    #[test]
    fn a_stopped_firewall_is_no_drift() {
        let record = record();
        let (broker, _) = positions()[0].clone();
        for position in 0..=1 {
            assert_eq!(
                record.layout_holds(position, Some(&broker), None),
                position == 0
            );
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
        let (broker, _) = positions()[0].clone();
        assert!(record.layout_holds(0, Some(&broker), Some(&absent)));
        assert!(!record.layout_holds(0, Some(&broker), Some(&rules('a', 'a'))));
    }

    #[test]
    fn what_was_never_managed_holds_everywhere() {
        let mut record = record();
        record.broker = None;
        record.firewall = None;
        for position in 0..=COUNT {
            assert!(record.layout_holds(position, None, None));
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
    fn the_steps_run_broker_first() {
        assert_eq!(step(0).expect("first"), LayoutStep::Broker);
        assert_eq!(
            step(2).expect("last"),
            LayoutStep::Rule(WindowsFirewallRule::App)
        );
        assert!(step(COUNT).is_err());
    }
}
