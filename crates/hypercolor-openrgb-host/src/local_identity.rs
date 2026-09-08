//! Bind a daemon-advertised directory to its persisted local identity.

use std::{io, path::Path};

/// Reject missing, relative, or mismatched instance identities before local
/// hardware operations. A loopback SSH tunnel is not proof of a local daemon.
pub fn verify_instance_directory(directory: &Path, instance_id: &str) -> io::Result<()> {
    if !directory.is_absolute() || instance_id.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon returned an invalid local instance identity",
        ));
    }
    let local = std::fs::read_to_string(directory.join("instance_id"))?;
    if local.trim() != instance_id.trim() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon identity differs from the local instance; run this operation on its host",
        ));
    }
    Ok(())
}
