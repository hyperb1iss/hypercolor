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

struct TemplateWrite {
    reservation: Option<AtomicWriteReservation>,
    payload: Vec<u8>,
    durable: bool,
}

pub(super) struct PreparedTemplates {
    guard: OwnedRwLockWriteGuard<ComponentRegistry>,
    candidate: Option<ComponentRegistry>,
    writes: Vec<TemplateWrite>,
    retirements: Vec<(PathBuf, PathBuf, String)>,
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
        prepare(guard, &before, &desired, &attachments_dir, operation_id)
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
) -> Result<PreparedTemplates, DomainError> {
    let current: BTreeMap<_, _> = guard
        .list(&TemplateFilter {
            origin: Some(ComponentOrigin::User),
            ..TemplateFilter::default()
        })
        .into_iter()
        .map(|template| (template.id.clone(), template.clone()))
        .collect();
    if current != *before {
        return Err(DomainError::conflict(
            "attachment templates changed during restore preparation",
        ));
    }
    let mut candidate = guard.clone();
    for id in before.keys() {
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
        if path.exists() {
            let disk: ComponentTemplateManifest =
                toml::from_str(&fs::read_to_string(&path).map_err(internal)?).map_err(internal)?;
            if disk.template.id != *id {
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
            reservation: Some(writer.reserve()),
            payload,
            durable: false,
        });
    }
    let mut retirements = Vec::new();
    for (path, id) in template_files(directory).map_err(internal)? {
        if (before.contains_key(&id) || desired.contains_key(&id)) && !canonical.contains(&path) {
            let retired = path.with_extension(format!("retired-{operation_id}"));
            retirements.push((path, retired, id));
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
        for (source, destination, id) in &self.retirements {
            if shutdown.is_shutdown() {
                return Ok(AtomicWriteWaitOutcome::ShutdownPending);
            }
            retire_template(source, destination, id).map_err(internal)?;
        }
        for write in &mut self.writes {
            if shutdown.is_shutdown() {
                return Ok(AtomicWriteWaitOutcome::ShutdownPending);
            }
            if write.durable {
                continue;
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

    pub(super) fn publish(&mut self) {
        assert!(self.durable, "template files durable before publication");
        *self.guard = self.candidate.take().expect("templates published once");
    }
}

fn template_files(root: &Path) -> anyhow::Result<Vec<(PathBuf, String)>> {
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
                let manifest: ComponentTemplateManifest =
                    toml::from_str(&fs::read_to_string(&path)?)?;
                result.push((path, manifest.template.id));
            }
        }
    }
    Ok(result)
}

fn retire_template(source: &Path, destination: &Path, id: &str) -> anyhow::Result<()> {
    if source.exists() {
        let manifest: ComponentTemplateManifest = toml::from_str(&fs::read_to_string(source)?)?;
        anyhow::ensure!(
            manifest.template.id == id,
            "template identity changed during retirement"
        );
        hypercolor_platform_fs::durable_replace(source, destination)?;
    } else if destination.exists() {
        let parent = destination
            .parent()
            .ok_or_else(|| anyhow::anyhow!("missing retirement parent"))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        let mut old = hypercolor_platform_fs::open_no_follow(destination)?;
        std::io::copy(&mut old, temporary.as_file_mut())?;
        temporary.as_file().sync_all()?;
        hypercolor_platform_fs::durable_replace(temporary.path(), destination)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
