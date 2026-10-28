//! User template files and their runtime registry form one guarded replacement.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hypercolor_core::attachment::{ComponentRegistry, TemplateFilter};
use hypercolor_types::attachment::{ComponentOrigin, ComponentTemplate, ComponentTemplateManifest};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

use super::internal;
use crate::domain::DomainError;
use crate::persistence::{
    AtomicFileWriter, AtomicWriteReservation, AtomicWriteWaitOutcome, PersistenceShutdown,
};

#[derive(Clone, PartialEq)]
struct TemplateFileProof {
    template: ComponentTemplate,
    digest: Vec<u8>,
}

struct TemplateWrite {
    destination: PathBuf,
    expected: Option<TemplateFileProof>,
    reservation: Option<AtomicWriteReservation>,
    payload: Vec<u8>,
    durable: bool,
}

pub(super) struct PreparedTemplates {
    guard: OwnedRwLockWriteGuard<ComponentRegistry>,
    candidate: Option<ComponentRegistry>,
    writes: Vec<TemplateWrite>,
    retirements: Vec<(PathBuf, PathBuf, TemplateFileProof)>,
    durable: bool,
}

pub(super) async fn prepare_templates(
    registry: Arc<RwLock<ComponentRegistry>>,
    before: &BTreeMap<String, ComponentTemplate>,
    desired: &BTreeMap<String, ComponentTemplate>,
    attachments_dir: &Path,
    operation_id: Uuid,
    _staging_root: &Path,
) -> Result<PreparedTemplates, DomainError> {
    let guard = registry.write_owned().await;
    let before = before.clone();
    let desired = desired.clone();
    let attachments_dir = attachments_dir.to_owned();
    tokio::task::spawn_blocking(move || {
        prepare(
            guard,
            &before,
            &desired,
            &attachments_dir,
            operation_id,
            false,
        )
    })
    .await
    .map_err(internal)?
}

pub(super) async fn prepare_templates_recovery(
    registry: Arc<RwLock<ComponentRegistry>>,
    before: &BTreeMap<String, ComponentTemplate>,
    desired: &BTreeMap<String, ComponentTemplate>,
    attachments_dir: &Path,
    operation_id: Uuid,
    _staging_root: &Path,
) -> Result<PreparedTemplates, DomainError> {
    let guard = registry.write_owned().await;
    let before = before.clone();
    let desired = desired.clone();
    let directory = attachments_dir.to_owned();
    tokio::task::spawn_blocking(move || {
        prepare(guard, &before, &desired, &directory, operation_id, true)
    })
    .await
    .map_err(internal)?
}

fn prepare(
    guard: OwnedRwLockWriteGuard<ComponentRegistry>,
    before: &BTreeMap<String, ComponentTemplate>,
    desired: &BTreeMap<String, ComponentTemplate>,
    directory: &Path,
    operation_id: Uuid,
    recovering: bool,
) -> Result<PreparedTemplates, DomainError> {
    let current: BTreeMap<_, _> = guard
        .list(&TemplateFilter {
            origin: Some(ComponentOrigin::User),
            ..TemplateFilter::default()
        })
        .into_iter()
        .map(|template| (template.id.clone(), template.clone()))
        .collect();
    if recovering {
        validate_recovery(&current, before, desired, directory, operation_id).map_err(internal)?;
    } else if current != *before {
        return Err(DomainError::conflict(
            "attachment templates changed during restore preparation",
        ));
    }
    let mut candidate = guard.clone();
    for id in current.keys() {
        candidate.remove(id).map_err(internal)?;
    }
    let mut writes = Vec::new();
    let mut canonical = HashSet::new();
    for (id, template) in desired {
        if template.id != *id || template.origin != ComponentOrigin::User {
            return Err(DomainError::validation(
                "portable template identity or origin differs",
            ));
        }
        candidate.register(template.clone()).map_err(internal)?;
        let mut digest = String::with_capacity(64);
        for byte in Sha256::digest(id.as_bytes()) {
            write!(&mut digest, "{byte:02x}").expect("String formatting");
        }
        let path = directory.join(format!("portable-{digest}.toml"));
        canonical.insert(path.clone());
        let expected = read_template_file(&path).map_err(internal)?;
        if let Some(proof) = &expected {
            validate_disk_template(proof, before, desired, recovering).map_err(internal)?;
            if proof.template.id != *id {
                return Err(DomainError::conflict(
                    "template destination belongs to another identity",
                ));
            }
        }
        let payload = toml::to_string_pretty(&ComponentTemplateManifest {
            template: template.clone(),
            ..ComponentTemplateManifest::default()
        })
        .map_err(internal)?
        .into_bytes();
        let writer = AtomicFileWriter::new(&path).map_err(internal)?;
        writes.push(TemplateWrite {
            destination: path,
            expected,
            reservation: Some(writer.reserve()),
            payload,
            durable: false,
        });
    }
    let mut retirements = Vec::new();
    for (path, proof) in template_files(directory).map_err(internal)? {
        let id = &proof.template.id;
        if (before.contains_key(id) || desired.contains_key(id)) && !canonical.contains(&path) {
            validate_disk_template(&proof, before, desired, recovering).map_err(internal)?;
            let retired = path.with_extension(format!("retired-{operation_id}"));
            retirements.push((path, retired, proof));
        }
    }
    Ok(PreparedTemplates {
        guard,
        candidate: Some(candidate),
        writes,
        retirements,
        durable: false,
    })
}

impl PreparedTemplates {
    pub(super) fn candidate_registry(&self) -> &ComponentRegistry {
        self.candidate
            .as_ref()
            .expect("unpublished template registry")
    }

    pub(super) fn persist(
        &mut self,
        shutdown: &PersistenceShutdown,
    ) -> Result<AtomicWriteWaitOutcome, DomainError> {
        if self.durable {
            return Ok(AtomicWriteWaitOutcome::Written);
        }
        let retired = self.persist_retirements(shutdown)?;
        if retired != AtomicWriteWaitOutcome::Written {
            return Ok(retired);
        }
        for write in &mut self.writes {
            if shutdown.is_shutdown() {
                return Ok(AtomicWriteWaitOutcome::ShutdownPending);
            }
            if write.durable {
                continue;
            }
            if read_template_file(&write.destination).map_err(internal)? != write.expected {
                return Err(DomainError::conflict(
                    "template file changed after restore preparation",
                ));
            }
            let outcome = write
                .reservation
                .take()
                .expect("template admitted once")
                .admit(std::mem::take(&mut write.payload))
                .commit_and_wait_durable_or_shutdown(shutdown);
            if outcome != AtomicWriteWaitOutcome::Written {
                return Ok(outcome);
            }
            write.durable = true;
        }
        self.durable = true;
        Ok(AtomicWriteWaitOutcome::Written)
    }

    fn persist_retirements(
        &mut self,
        shutdown: &PersistenceShutdown,
    ) -> Result<AtomicWriteWaitOutcome, DomainError> {
        for (source, destination, proof) in &self.retirements {
            if shutdown.is_shutdown() {
                return Ok(AtomicWriteWaitOutcome::ShutdownPending);
            }
            retire_template(source, destination, proof).map_err(internal)?;
        }
        Ok(AtomicWriteWaitOutcome::Written)
    }

    pub(super) fn publish(&mut self) {
        assert!(self.durable, "template files durable before publication");
        *self.guard = self.candidate.take().expect("templates published once");
    }
}

fn template_files(root: &Path) -> anyhow::Result<Vec<(PathBuf, TemplateFileProof)>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut directories = vec![root.to_owned()];
    let mut result = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_symlink()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
            {
                anyhow::bail!("cannot restore over a symbolic-link template");
            } else if kind.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
            {
                let proof = read_template_file(&path)?
                    .ok_or_else(|| anyhow::anyhow!("template disappeared during scan"))?;
                result.push((path, proof));
            }
        }
    }
    Ok(result)
}

fn retire_template(
    source: &Path,
    destination: &Path,
    expected: &TemplateFileProof,
) -> anyhow::Result<()> {
    if let Some(actual) = read_template_file(source)? {
        anyhow::ensure!(
            &actual == expected,
            "template content changed during retirement"
        );
        hypercolor_platform_fs::durable_replace(source, destination)?;
    } else if let Some(actual) = read_template_file(destination)? {
        anyhow::ensure!(&actual == expected, "retired template content changed");
        let parent = destination
            .parent()
            .ok_or_else(|| anyhow::anyhow!("missing retirement parent"))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        let mut old = hypercolor_platform_fs::open_no_follow(destination)?;
        std::io::copy(&mut old, temporary.as_file_mut())?;
        temporary.as_file().sync_all()?;
        hypercolor_platform_fs::durable_replace(temporary.path(), destination)?;
    } else {
        anyhow::bail!("template and retirement proof disappeared");
    }
    Ok(())
}

fn read_template_file(path: &Path) -> anyhow::Result<Option<TemplateFileProof>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut manifest: ComponentTemplateManifest = toml::from_str(std::str::from_utf8(&bytes)?)?;
    manifest.template.origin = ComponentOrigin::User;
    Ok(Some(TemplateFileProof {
        template: manifest.template,
        digest: Sha256::digest(&bytes).to_vec(),
    }))
}

fn validate_disk_template(
    proof: &TemplateFileProof,
    before: &BTreeMap<String, ComponentTemplate>,
    desired: &BTreeMap<String, ComponentTemplate>,
    recovering: bool,
) -> anyhow::Result<()> {
    let id = &proof.template.id;
    anyhow::ensure!(
        before.get(id) == Some(&proof.template)
            || (recovering && desired.get(id) == Some(&proof.template)),
        "template on disk differs from restore authority"
    );
    Ok(())
}

#[cfg(test)]
mod tests;

fn validate_recovery(
    current: &BTreeMap<String, ComponentTemplate>,
    before: &BTreeMap<String, ComponentTemplate>,
    desired: &BTreeMap<String, ComponentTemplate>,
    directory: &Path,
    operation_id: Uuid,
) -> anyhow::Result<()> {
    for (id, value) in current {
        anyhow::ensure!(
            before.get(id) == Some(value) || desired.get(id) == Some(value),
            "template changed outside pending restore"
        );
    }
    let retired = retired_templates(directory, operation_id)?;
    for (id, old) in before {
        if !current.contains_key(id) {
            anyhow::ensure!(
                retired.iter().any(|template| template == old),
                "missing template lacks exact operation retirement proof"
            );
        }
    }
    Ok(())
}

fn retired_templates(root: &Path, operation_id: Uuid) -> anyhow::Result<Vec<ComponentTemplate>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let extension = format!("retired-{operation_id}");
    let mut directories = vec![root.to_owned()];
    let mut result = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file()
                && path
                    .extension()
                    .is_some_and(|value| value == extension.as_str())
            {
                let mut manifest: ComponentTemplateManifest =
                    toml::from_str(&fs::read_to_string(path)?)?;
                manifest.template.origin = ComponentOrigin::User;
                result.push(manifest.template);
            }
        }
    }
    Ok(result)
}
