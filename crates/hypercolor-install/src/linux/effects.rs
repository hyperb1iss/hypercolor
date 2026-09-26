use super::super::{InstallPlatformError, PlatformCheckpoint, UnitId, UnitRecord};
use super::model::{
    LinuxDirectoryState, LinuxExactEntry, LinuxFilePublication, LinuxLauncher,
    LinuxLayoutPublication, LinuxRecord, entries_match, error, hex_digest,
};
use super::proof::read_unit_file;

#[derive(Clone, Copy)]
pub(super) enum EffectDirection {
    Candidate,
    Prior,
}

pub(super) fn autostart_operation(
    enabled: bool,
    observation: &super::model::LinuxSystemdObservation,
) -> Option<&'static str> {
    if !enabled && observation.load_state == "not-found" {
        None
    } else if enabled {
        Some("enable")
    } else {
        Some("disable")
    }
}

pub(super) fn candidate_launcher_entry(launcher: Option<&LinuxLauncher>) -> LinuxExactEntry {
    launcher.map_or(LinuxExactEntry::Absent, |launcher| {
        LinuxExactEntry::RegularFile {
            mode: launcher.mode,
            sha256: hex_digest(&launcher.bytes),
            snapshot_unit: None,
            snapshot_path: None,
        }
    })
}

pub(super) fn candidate_layout_entry(target: &str) -> LinuxExactEntry {
    LinuxExactEntry::Symlink {
        target: target.to_owned(),
    }
}

pub(super) fn require_exact_entry(
    actual: &LinuxExactEntry,
    expected: &LinuxExactEntry,
    message: &'static str,
) -> Result<(), InstallPlatformError> {
    if !entries_match(actual, expected) {
        return Err(error(message));
    }
    Ok(())
}

pub(super) fn directory_create_effect(
    direction: EffectDirection,
    current: LinuxDirectoryState,
) -> Result<bool, InstallPlatformError> {
    match direction {
        EffectDirection::Candidate if current == LinuxDirectoryState::Absent => Ok(true),
        EffectDirection::Prior if current == LinuxDirectoryState::Present => Ok(false),
        EffectDirection::Candidate => {
            Err(error("Linux public scaffolding drifted before creation"))
        }
        EffectDirection::Prior => Err(error("Linux retained public scaffolding disappeared")),
    }
}

pub(super) fn launcher_effect(
    checkpoint: PlatformCheckpoint,
    unit: Option<&UnitId>,
    record: &LinuxRecord,
) -> Result<(LinuxExactEntry, Option<LinuxFilePublication>), InstallPlatformError> {
    match effect_direction(
        checkpoint,
        PlatformCheckpoint::CandidateLauncher,
        PlatformCheckpoint::PriorLauncherRestored,
    )? {
        EffectDirection::Candidate => {
            let expected_unit = record
                .candidate_launcher
                .as_ref()
                .map(|_| &record.candidate.unit);
            if unit != expected_unit {
                return Err(error("candidate launcher effect requested an unknown unit"));
            }
            let publication =
                record
                    .candidate_launcher
                    .as_ref()
                    .map(|launcher| LinuxFilePublication {
                        mode: launcher.mode,
                        contents: launcher.bytes.clone(),
                    });
            Ok((record.prior_launcher.clone(), publication))
        }
        EffectDirection::Prior => {
            let expected_unit = if matches!(record.prior_launcher, LinuxExactEntry::Absent) {
                None
            } else {
                record.prior.as_ref().map(|prior| &prior.unit)
            };
            if unit != expected_unit {
                return Err(error("prior launcher effect requested an unknown unit"));
            }
            let publication = match &record.prior_launcher {
                LinuxExactEntry::RegularFile { mode, .. } => Some(LinuxFilePublication {
                    mode: *mode,
                    contents: record.prior_launcher_bytes.clone(),
                }),
                LinuxExactEntry::Absent | LinuxExactEntry::Symlink { .. } => None,
            };
            Ok((
                candidate_launcher_entry(record.candidate_launcher.as_ref()),
                publication,
            ))
        }
    }
}

pub(super) fn layout_effect(
    checkpoint: PlatformCheckpoint,
    unit: Option<&UnitId>,
    record: &LinuxRecord,
    prior: &LinuxExactEntry,
    candidate_target: &str,
    known_units: &[UnitRecord],
) -> Result<(LinuxExactEntry, Option<LinuxLayoutPublication>), InstallPlatformError> {
    let direction = layout_direction(checkpoint)?;
    validate_layout_unit(direction, unit, record)?;
    match direction {
        EffectDirection::Candidate => Ok((
            prior.clone(),
            Some(LinuxLayoutPublication::Symlink(candidate_target.to_owned())),
        )),
        EffectDirection::Prior => Ok((
            candidate_layout_entry(candidate_target),
            prior_layout_publication(prior, known_units)?,
        )),
    }
}

pub(super) fn validate_layout_unit(
    direction: EffectDirection,
    unit: Option<&UnitId>,
    record: &LinuxRecord,
) -> Result<(), InstallPlatformError> {
    let expected = match direction {
        EffectDirection::Candidate => Some(&record.candidate.unit),
        EffectDirection::Prior => {
            let prior_present = record.layout.iter().any(|operation| {
                matches!(
                    &operation.effect,
                    super::model::LinuxLayoutEffect::Entry { prior, .. }
                        if !matches!(prior, LinuxExactEntry::Absent)
                )
            });
            prior_present
                .then(|| record.prior.as_ref().map(|prior| &prior.unit))
                .flatten()
        }
    };
    if unit == expected {
        Ok(())
    } else {
        Err(error("layout effect requested an unknown unit"))
    }
}

pub(super) fn layout_direction(
    checkpoint: PlatformCheckpoint,
) -> Result<EffectDirection, InstallPlatformError> {
    effect_direction(
        checkpoint,
        PlatformCheckpoint::CandidateLayout,
        PlatformCheckpoint::PriorLayoutRestored,
    )
}

fn prior_layout_publication(
    prior: &LinuxExactEntry,
    known_units: &[UnitRecord],
) -> Result<Option<LinuxLayoutPublication>, InstallPlatformError> {
    match prior {
        LinuxExactEntry::Symlink { target } => {
            Ok(Some(LinuxLayoutPublication::Symlink(target.clone())))
        }
        LinuxExactEntry::RegularFile {
            mode,
            sha256,
            snapshot_unit,
            snapshot_path,
        } => {
            let snapshot_unit = snapshot_unit
                .as_ref()
                .ok_or_else(|| error("prior regular layout entry lacks immutable snapshot unit"))?;
            let snapshot_path = snapshot_path
                .as_ref()
                .ok_or_else(|| error("prior regular layout entry lacks immutable snapshot path"))?;
            let retained = known_units
                .iter()
                .find(|known| known.id() == snapshot_unit)
                .ok_or_else(|| error("prior layout snapshot unit authority is not retained"))?;
            let contents = read_unit_file(
                retained,
                snapshot_path,
                hypercolor_platform_fs::MAX_EXACT_ENTRY_BYTES,
            )?;
            if hex_digest(&contents) != *sha256 {
                return Err(error(
                    "prior layout snapshot digest does not match the record",
                ));
            }
            Ok(Some(LinuxLayoutPublication::RegularFile(
                LinuxFilePublication {
                    mode: *mode,
                    contents,
                },
            )))
        }
        LinuxExactEntry::Absent => Ok(None),
    }
}

fn effect_direction(
    actual: PlatformCheckpoint,
    candidate: PlatformCheckpoint,
    prior: PlatformCheckpoint,
) -> Result<EffectDirection, InstallPlatformError> {
    if actual == candidate {
        Ok(EffectDirection::Candidate)
    } else if actual == prior {
        Ok(EffectDirection::Prior)
    } else {
        Err(error(
            "Linux effect received an invalid destination checkpoint",
        ))
    }
}
