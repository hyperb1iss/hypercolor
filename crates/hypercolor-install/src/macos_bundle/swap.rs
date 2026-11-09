//! The bounded journal of one bundle swap.
//!
//! A transaction moves through exactly three positions, each named by what
//! the installed bundle and the transaction's staged copy hold:
//!
//! | Position  | Installed bundle | Staged copy |
//! | --------- | ---------------- | ----------- |
//! | `Before`  | prior            | absent      |
//! | `Staged`  | prior            | candidate   |
//! | `Swapped` | candidate        | prior       |
//!
//! Every position is matched against both bundles' cdhashes, so a step
//! repeated after a crash is recognized as done rather than redone. A
//! rollback swaps back and retires the candidate's copy to its discard
//! name in one rename, so the prior restarts without waiting on a tree
//! removal. Once the transaction is terminal, `settle` removes the
//! displaced prior or the retired candidate. With no prior bundle
//! installed, the prior side of each position is absent and the swap
//! becomes a move.

use super::model::{MacosBundleIdentity, MacosBundleObservation, error};
use crate::{InstallPlatformError, PlatformCheckpoint};

/// The layout operations of one transaction: stage, then swap.
pub(super) const SWAP_OPERATION_COUNT: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SwapPosition {
    Before,
    Staged,
    Swapped,
}

impl SwapPosition {
    fn from_index(index: u16) -> Option<Self> {
        match index {
            0 => Some(Self::Before),
            1 => Some(Self::Staged),
            2 => Some(Self::Swapped),
            _ => None,
        }
    }

    /// The position the coordinator expects at `checkpoint`.
    ///
    /// Only the layout checkpoints move with the operation cursor; every
    /// checkpoint before the layout holds the original bundles, and every
    /// one after it the swapped bundles, whatever the cursor says.
    pub(super) fn at(checkpoint: PlatformCheckpoint, index: u16) -> Option<Self> {
        match checkpoint {
            PlatformCheckpoint::PriorOriginal
            | PlatformCheckpoint::PriorUnloaded
            | PlatformCheckpoint::PriorManager
            | PlatformCheckpoint::PriorAutostart
            | PlatformCheckpoint::PriorRestored => Some(Self::Before),
            PlatformCheckpoint::CandidateLayout | PlatformCheckpoint::PriorLayoutRestored => {
                Self::from_index(index)
            }
            PlatformCheckpoint::CandidateLauncher
            | PlatformCheckpoint::CandidateActive
            | PlatformCheckpoint::CandidateManager
            | PlatformCheckpoint::CandidateAutostart
            | PlatformCheckpoint::CandidateRuntime
            | PlatformCheckpoint::PriorActiveRestored
            | PlatformCheckpoint::PriorLauncherRestored => Some(Self::Swapped),
        }
    }
}

/// The one effect that moves a swap between two adjacent positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SwapStep {
    /// Clone or durably copy the candidate into its staged copy.
    Materialize,
    /// Exchange the staged copy with the installed bundle in one rename.
    Exchange,
    /// Move the staged copy into place where nothing is installed.
    InstallStaged,
    /// Move the installed bundle back to the staging name.
    WithdrawInstalled,
    /// Move the staged copy to its discard name, for `settle` to remove.
    Retire,
}

/// The two bundles of one transaction.
#[derive(Debug, Clone, Copy)]
pub(super) struct SwapPlan<'a> {
    pub(super) prior: Option<&'a MacosBundleIdentity>,
    pub(super) candidate: &'a MacosBundleIdentity,
}

impl SwapPlan<'_> {
    /// What the installed bundle and the staged copy hold at `position`.
    fn expected(
        &self,
        position: SwapPosition,
    ) -> (Option<&MacosBundleIdentity>, Option<&MacosBundleIdentity>) {
        match position {
            SwapPosition::Before => (self.prior, None),
            SwapPosition::Staged => (self.prior, Some(self.candidate)),
            SwapPosition::Swapped => (Some(self.candidate), self.prior),
        }
    }

    /// Whether `observation` shows exactly `position` for the staged copy
    /// named `name`. Interrupted leftovers never change the answer.
    pub(super) fn holds(
        &self,
        position: SwapPosition,
        observation: &MacosBundleObservation,
        name: &str,
    ) -> bool {
        let (installed, staged) = self.expected(position);
        observation.installed.holds(installed) && observation.staged_slot(name).holds(staged)
    }

    /// The position `observation` shows, if it shows any.
    pub(super) fn position_of(
        &self,
        observation: &MacosBundleObservation,
        name: &str,
    ) -> Option<SwapPosition> {
        [
            SwapPosition::Before,
            SwapPosition::Staged,
            SwapPosition::Swapped,
        ]
        .into_iter()
        .find(|position| self.holds(*position, observation, name))
    }

    /// The step layout operation `index` takes, and the positions it moves
    /// between, forward or in rollback.
    pub(super) fn step(
        &self,
        forward: bool,
        index: u16,
    ) -> Result<(SwapPosition, SwapPosition, SwapStep), InstallPlatformError> {
        let prior_installed = self.prior.is_some();
        match (forward, index) {
            (true, 0) => Ok((
                SwapPosition::Before,
                SwapPosition::Staged,
                SwapStep::Materialize,
            )),
            (true, 1) => Ok((
                SwapPosition::Staged,
                SwapPosition::Swapped,
                if prior_installed {
                    SwapStep::Exchange
                } else {
                    SwapStep::InstallStaged
                },
            )),
            (false, 1) => Ok((
                SwapPosition::Swapped,
                SwapPosition::Staged,
                if prior_installed {
                    SwapStep::Exchange
                } else {
                    SwapStep::WithdrawInstalled
                },
            )),
            (false, 0) => Ok((SwapPosition::Staged, SwapPosition::Before, SwapStep::Retire)),
            _ => Err(error(
                "macOS bundle layout cursor exceeds its two operations",
            )),
        }
    }

    /// Whether a terminal transaction still has a staged copy or leftovers
    /// to remove, after proving the installed bundle is the outcome's.
    ///
    /// A committed transaction may only remove the displaced prior, and a
    /// rolled-back one only its own candidate's copy.
    pub(super) fn settle(
        &self,
        committed: bool,
        observation: &MacosBundleObservation,
        name: &str,
    ) -> Result<bool, InstallPlatformError> {
        let (installed, removable) = if committed {
            (Some(self.candidate), self.prior)
        } else {
            (self.prior, Some(self.candidate))
        };
        let staged = observation.staged_slot(name);
        if !observation.installed.holds(installed)
            || !(staged.is_absent() || staged.holds(removable))
        {
            return Err(error(
                "the macOS bundles differ from this transaction's outcome; nothing was removed",
            ));
        }
        Ok(observation.staged.contains_key(name) || has_leftovers(observation, name))
    }
}

/// Whether an interrupted materialization or removal of the staged copy
/// named `name` left anything behind.
pub(super) fn has_leftovers(observation: &MacosBundleObservation, name: &str) -> bool {
    observation
        .interrupted
        .iter()
        .any(|leftover| is_leftover_of(leftover, name))
}

/// Whether `leftover` is the scratch or discard name of the staged copy
/// named `name`.
fn is_leftover_of(leftover: &str, name: &str) -> bool {
    leftover.strip_prefix(name).is_some_and(|suffix| {
        suffix == super::MACOS_BUNDLE_PARTIAL_SUFFIX || suffix == super::MACOS_BUNDLE_DISCARD_SUFFIX
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{SwapPlan, SwapPosition, SwapStep};
    use crate::PlatformCheckpoint;
    use crate::macos_bundle::{MacosBundleIdentity, MacosBundleObservation, MacosBundleSlot};

    const NAME: &str = "c";

    fn identity(seed: char) -> MacosBundleIdentity {
        MacosBundleIdentity {
            app: seed.to_string().repeat(40),
            daemon: seed.to_string().repeat(40),
            cli: seed.to_string().repeat(40),
        }
    }

    fn observed(
        installed: Option<&MacosBundleIdentity>,
        staged: Option<&MacosBundleIdentity>,
    ) -> MacosBundleObservation {
        let slot = |identity: Option<&MacosBundleIdentity>| {
            identity.map_or(MacosBundleSlot::Absent, |identity| {
                MacosBundleSlot::Bundle(identity.clone())
            })
        };
        MacosBundleObservation {
            installed: slot(installed),
            staged: staged
                .map(|staged| BTreeMap::from([(NAME.to_owned(), slot(Some(staged)))]))
                .unwrap_or_default(),
            interrupted: BTreeSet::new(),
        }
    }

    #[test]
    fn every_position_is_told_apart_by_both_identities() {
        let (prior, candidate) = (identity('a'), identity('b'));
        let plan = SwapPlan {
            prior: Some(&prior),
            candidate: &candidate,
        };
        for (observation, expected) in [
            (observed(Some(&prior), None), Some(SwapPosition::Before)),
            (
                observed(Some(&prior), Some(&candidate)),
                Some(SwapPosition::Staged),
            ),
            (
                observed(Some(&candidate), Some(&prior)),
                Some(SwapPosition::Swapped),
            ),
            (observed(Some(&candidate), None), None),
            (observed(Some(&prior), Some(&prior)), None),
            (observed(None, Some(&candidate)), None),
        ] {
            assert_eq!(plan.position_of(&observation, NAME), expected);
        }
        let mut unreadable = observed(Some(&prior), None);
        unreadable.installed = MacosBundleSlot::Unrecognized("torn".to_owned());
        assert_eq!(plan.position_of(&unreadable, NAME), None);
    }

    #[test]
    fn checkpoints_outside_the_layout_ignore_the_cursor() {
        assert_eq!(
            SwapPosition::at(PlatformCheckpoint::PriorUnloaded, 2),
            Some(SwapPosition::Before)
        );
        assert_eq!(
            SwapPosition::at(PlatformCheckpoint::CandidateLauncher, 1),
            Some(SwapPosition::Swapped)
        );
        assert_eq!(
            SwapPosition::at(PlatformCheckpoint::CandidateLayout, 1),
            Some(SwapPosition::Staged)
        );
        assert_eq!(
            SwapPosition::at(PlatformCheckpoint::PriorLayoutRestored, 3),
            None
        );
    }

    #[test]
    fn steps_without_an_installed_prior_move_instead_of_swapping() {
        let candidate = identity('b');
        let plan = SwapPlan {
            prior: None,
            candidate: &candidate,
        };
        assert_eq!(
            plan.step(true, 1).expect("forward").2,
            SwapStep::InstallStaged
        );
        assert_eq!(
            plan.step(false, 1).expect("reverse").2,
            SwapStep::WithdrawInstalled
        );
        assert_eq!(plan.step(false, 0).expect("reverse").2, SwapStep::Retire);
        assert!(plan.step(true, 2).is_err());
    }

    #[test]
    fn settling_removes_only_the_outcome_s_leftover() {
        let (prior, candidate) = (identity('a'), identity('b'));
        let plan = SwapPlan {
            prior: Some(&prior),
            candidate: &candidate,
        };
        let swapped = observed(Some(&candidate), Some(&prior));
        assert!(plan.settle(true, &swapped, NAME).expect("committed"));
        assert!(plan.settle(false, &swapped, NAME).is_err());
        let staged = observed(Some(&prior), Some(&candidate));
        assert!(plan.settle(false, &staged, NAME).expect("rolled back"));
        assert!(plan.settle(true, &staged, NAME).is_err());
        let mut settled = observed(Some(&candidate), None);
        assert!(!plan.settle(true, &settled, NAME).expect("already settled"));
        settled.interrupted.insert(format!("{NAME}.discard"));
        assert!(plan.settle(true, &settled, NAME).expect("leftover"));
    }
}
