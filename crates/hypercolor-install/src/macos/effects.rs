use super::super::{InstallPlatformError, PlatformCheckpoint, UnitId, UnitRecord};
use super::executor::MacosInstallExecutor;
use super::model::{
    MAX_LEGACY_FILE_BYTES, MacosDirectoryState, MacosEntryPublication, MacosExactEntry,
    MacosFilePublication, MacosLauncher, MacosRecord, entries_match, error, hex_digest,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EffectDirection {
    Candidate,
    Prior,
}

pub(super) fn candidate_launcher_entry(launcher: Option<&MacosLauncher>) -> MacosExactEntry {
    launcher.map_or(MacosExactEntry::Absent, |launcher| {
        MacosExactEntry::RegularFile {
            mode: launcher.mode,
            sha256: hex_digest(launcher.bytes.as_bytes()),
            snapshot_unit: None,
            snapshot_path: None,
        }
    })
}

pub(super) fn candidate_layout_entry(target: Option<&str>) -> MacosExactEntry {
    target.map_or(MacosExactEntry::Absent, |target| MacosExactEntry::Symlink {
        target: target.to_owned(),
    })
}

pub(super) fn require_exact_entry(
    actual: &MacosExactEntry,
    expected: &MacosExactEntry,
    message: &'static str,
) -> Result<(), InstallPlatformError> {
    if !entries_match(actual, expected) {
        return Err(error(message));
    }
    Ok(())
}

pub(super) fn directory_effect(
    direction: EffectDirection,
    current: MacosDirectoryState,
) -> Result<bool, InstallPlatformError> {
    match direction {
        EffectDirection::Candidate if current == MacosDirectoryState::Absent => Ok(true),
        EffectDirection::Prior if current == MacosDirectoryState::Present => Ok(false),
        EffectDirection::Candidate => Err(error("macOS public scaffold drifted before creation")),
        EffectDirection::Prior => Err(error("retained macOS public scaffold disappeared")),
    }
}

pub(super) fn launcher_effect(
    checkpoint: PlatformCheckpoint,
    unit: Option<&UnitId>,
    record: &MacosRecord,
) -> Result<(MacosExactEntry, Option<MacosFilePublication>), InstallPlatformError> {
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
                return Err(error("candidate macOS launcher requested an unknown unit"));
            }
            let publication =
                record
                    .candidate_launcher
                    .as_ref()
                    .map(|launcher| MacosFilePublication {
                        mode: launcher.mode,
                        contents: launcher.bytes.as_bytes().to_vec(),
                    });
            Ok((record.prior_launcher.clone(), publication))
        }
        EffectDirection::Prior => {
            let expected_unit = if matches!(record.prior_launcher, MacosExactEntry::Absent) {
                None
            } else {
                record.prior.as_ref().map(|prior| &prior.unit)
            };
            if unit != expected_unit {
                return Err(error("prior macOS launcher requested an unknown unit"));
            }
            let publication = match &record.prior_launcher {
                MacosExactEntry::RegularFile { mode, .. } => Some(MacosFilePublication {
                    mode: *mode,
                    contents: record.prior_launcher_bytes.as_bytes().to_vec(),
                }),
                MacosExactEntry::Absent | MacosExactEntry::Symlink { .. } => None,
            };
            Ok((
                candidate_launcher_entry(record.candidate_launcher.as_ref()),
                publication,
            ))
        }
    }
}

pub(super) fn layout_effect<E: MacosInstallExecutor>(
    executor: &mut E,
    checkpoint: PlatformCheckpoint,
    unit: Option<&UnitId>,
    record: &MacosRecord,
    prior: &MacosExactEntry,
    candidate_target: Option<&str>,
    known_units: &[UnitRecord],
) -> Result<(MacosExactEntry, Option<MacosEntryPublication>), InstallPlatformError> {
    let direction = layout_direction(checkpoint)?;
    validate_layout_unit(direction, unit, record)?;
    match direction {
        EffectDirection::Candidate => Ok((
            prior.clone(),
            candidate_target.map(|target| MacosEntryPublication::Symlink(target.to_owned())),
        )),
        EffectDirection::Prior => Ok((
            candidate_layout_entry(candidate_target),
            prior_publication(executor, prior, known_units)?,
        )),
    }
}

pub(super) fn validate_layout_unit(
    direction: EffectDirection,
    unit: Option<&UnitId>,
    record: &MacosRecord,
) -> Result<(), InstallPlatformError> {
    let expected = match direction {
        EffectDirection::Candidate => Some(&record.candidate.unit),
        EffectDirection::Prior => record.prior.as_ref().and_then(|prior| {
            record
                .layout
                .iter()
                .any(|operation| match &operation.effect {
                    super::model::MacosLayoutEffect::Entry { prior, .. } => {
                        !matches!(prior, MacosExactEntry::Absent)
                    }
                    super::model::MacosLayoutEffect::Directory { .. } => false,
                })
                .then_some(&prior.unit)
        }),
    };
    if unit == expected {
        Ok(())
    } else {
        Err(error("macOS layout effect requested an unknown unit"))
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

fn prior_publication<E: MacosInstallExecutor>(
    executor: &mut E,
    prior: &MacosExactEntry,
    known_units: &[UnitRecord],
) -> Result<Option<MacosEntryPublication>, InstallPlatformError> {
    match prior {
        MacosExactEntry::Absent => Ok(None),
        MacosExactEntry::Symlink { target } => {
            Ok(Some(MacosEntryPublication::Symlink(target.clone())))
        }
        MacosExactEntry::RegularFile {
            mode,
            sha256,
            snapshot_unit,
            snapshot_path,
        } => {
            let snapshot_unit = snapshot_unit
                .as_ref()
                .ok_or_else(|| error("prior macOS file lacks its snapshot unit"))?;
            let snapshot_path = snapshot_path
                .as_deref()
                .ok_or_else(|| error("prior macOS file lacks its snapshot path"))?;
            let unit = known_units
                .iter()
                .find(|unit| unit.id() == snapshot_unit)
                .ok_or_else(|| error("prior macOS snapshot authority is not retained"))?;
            let contents =
                executor.read_snapshot_file(unit, snapshot_path, MAX_LEGACY_FILE_BYTES)?;
            if hex_digest(&contents) != *sha256 {
                return Err(error("prior macOS snapshot digest changed"));
            }
            Ok(Some(MacosEntryPublication::RegularFile(
                MacosFilePublication {
                    mode: *mode,
                    contents,
                },
            )))
        }
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
            "macOS effect received an invalid destination checkpoint",
        ))
    }
}
