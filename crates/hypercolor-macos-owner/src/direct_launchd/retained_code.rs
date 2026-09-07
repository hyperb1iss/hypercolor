use std::fmt::Write as _;
use std::fs::File;
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use super::MacosDirectLaunchdExecutableExpectation;
use super::mutation::run_deadline_read;
use crate::MacosOwnerExecutionError;

mod native;

use native::NativeAcceptedImageProof;

pub(super) use native::dynamic_cdhash_for_pid;

trait AcceptedImageProof {
    fn prove(
        &mut self,
        path: &Path,
        designated_requirement: &str,
        cdhash: &str,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError>;
}

/// Validate a manifest-bound executable image while retaining the exact source object.
///
/// The proof binds the retained bytes and metadata to the exact designated requirement and
/// architecture-selected CDHash of a suspended image accepted by the kernel. It does not claim
/// to contain a noncooperating same-UID process that sends the suspended child `SIGCONT`.
pub fn validate_retained_macos_executable(
    file: &File,
    expected: &MacosDirectLaunchdExecutableExpectation,
    timeout: Duration,
) -> Result<bool, MacosOwnerExecutionError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| MacosOwnerExecutionError::new("code validation deadline overflowed"))?;
    let file = file
        .try_clone()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    let expected = expected.clone();
    run_deadline_read(deadline, move || {
        validate_retained_with(&file, &expected, deadline, &mut NativeAcceptedImageProof)
    })
}

fn validate_retained_with(
    file: &File,
    expected: &MacosDirectLaunchdExecutableExpectation,
    deadline: Instant,
    proof: &mut impl AcceptedImageProof,
) -> Result<bool, MacosOwnerExecutionError> {
    require_time(deadline)?;
    let before = file
        .metadata()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if !metadata_matches(&before, expected) || !exact_hash_matches(file, expected, deadline)? {
        return Ok(false);
    }
    let after_hash = file
        .metadata()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if !same_exact_metadata(&before, &after_hash) || !metadata_matches(&after_hash, expected) {
        return Ok(false);
    }
    if !proof.prove(
        expected.path(),
        expected.designated_requirement(),
        expected.cdhash(),
        deadline,
    )? {
        return Ok(false);
    }
    if !exact_hash_matches(file, expected, deadline)? {
        return Ok(false);
    }
    let after_proof = file
        .metadata()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    Ok(same_exact_metadata(&before, &after_proof) && metadata_matches(&after_proof, expected))
}

fn exact_hash_matches(
    file: &File,
    expected: &MacosDirectLaunchdExecutableExpectation,
    deadline: Instant,
) -> Result<bool, MacosOwnerExecutionError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8 * 1_024];
    let mut offset = 0_u64;
    while offset <= expected.size() {
        require_time(deadline)?;
        let remaining = expected.size() + 1 - offset;
        let limit = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded executable read fits usize");
        let read = file
            .read_at(&mut buffer[..limit], offset)
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        offset += u64::try_from(read).expect("read length fits u64");
    }
    Ok(offset == expected.size() && hex_digest(&hasher.finalize()) == expected.sha256())
}

fn metadata_matches(
    metadata: &std::fs::Metadata,
    expected: &MacosDirectLaunchdExecutableExpectation,
) -> bool {
    metadata.is_file()
        && metadata.mode() & 0o7777 == expected.mode()
        && metadata.len() == expected.size()
        && metadata.nlink() == 1
        && metadata.dev() == expected.device()
        && metadata.ino() == expected.inode()
}

fn same_exact_metadata(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.mode() == right.mode()
        && left.len() == right.len()
        && left.nlink() == right.nlink()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
}

fn require_time(deadline: Instant) -> Result<(), MacosOwnerExecutionError> {
    if Instant::now() < deadline {
        Ok(())
    } else {
        Err(MacosOwnerExecutionError::new(
            "code validation exceeded its absolute deadline",
        ))
    }
}

fn hex_digest(digest: &[u8]) -> String {
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing into a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests;
