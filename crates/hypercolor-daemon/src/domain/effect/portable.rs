//! Prepared custom HTML hydration with durable, operation-bound ownership.

mod files;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hypercolor_core::effect::{
    EffectEntry, EffectRegistry, HTML_EFFECT_BUNDLE_MANIFEST, HTML_EFFECT_BUNDLE_PENDING,
    HtmlEffectBundleManifest, html_effect_bundle, load_html_effect_file,
};
use hypercolor_core::portable_setup::SetupBlobStore;
use hypercolor_types::effect::{EffectId, EffectSource};
use hypercolor_types::portable_setup::{PortableBlob, PortableEffect, is_portable_bundle_path};
use serde::{Deserialize, Serialize};
use tokio::sync::OwnedRwLockWriteGuard;
use uuid::Uuid;

use super::EffectContext;
use crate::domain::DomainError;
use crate::persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};

pub(crate) struct PortableEffectsRestore<'a> {
    pub(crate) before: &'a BTreeMap<Uuid, PortableEffect>,
    pub(crate) desired: &'a BTreeMap<Uuid, PortableEffect>,
    pub(crate) blobs: &'a SetupBlobStore,
    pub(crate) managed_root: &'a Path,
    pub(crate) staging_root: &'a Path,
    pub(crate) operation_id: Uuid,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectPlan {
    schema_version: u32,
    operation_id: Uuid,
    input_hash: String,
    retired: BTreeMap<Uuid, RetiredEffect>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredEffect {
    root: PathBuf,
    bundled: bool,
    files: BTreeMap<String, PortableBlob>,
}

/// All live catalog mutation is excluded until the coordinator publishes.
pub(crate) struct PreparedPortableEffects {
    _update_guard: OwnedRwLockWriteGuard<()>,
    registry: OwnedRwLockWriteGuard<EffectRegistry>,
    candidate: Option<EffectRegistry>,
    desired: BTreeMap<Uuid, PortableEffect>,
    changed: BTreeSet<Uuid>,
    plan: EffectPlan,
    blobs: SetupBlobStore,
    managed_root: PathBuf,
    staging_root: PathBuf,
    durable: bool,
}

impl EffectContext {
    pub(crate) async fn prepare_portable_effects(
        &self,
        restore: PortableEffectsRestore<'_>,
    ) -> Result<PreparedPortableEffects, DomainError> {
        let update_guard = Arc::clone(&self.update_gate).write_owned().await;
        let registry = Arc::clone(&self.registry).write_owned().await;
        let before = restore.before.clone();
        let desired = restore.desired.clone();
        let blobs = restore.blobs.clone();
        let managed_root = restore.managed_root.to_owned();
        let staging_root = restore.staging_root.to_owned();
        let operation_id = restore.operation_id;
        tokio::task::spawn_blocking(move || {
            prepare(
                update_guard,
                registry,
                PortableEffectsRestore {
                    before: &before,
                    desired: &desired,
                    blobs: &blobs,
                    managed_root: &managed_root,
                    staging_root: &staging_root,
                    operation_id,
                },
            )
        })
        .await
        .map_err(files::io)?
    }
}

fn prepare(
    update_guard: OwnedRwLockWriteGuard<()>,
    registry: OwnedRwLockWriteGuard<EffectRegistry>,
    restore: PortableEffectsRestore<'_>,
) -> Result<PreparedPortableEffects, DomainError> {
    validate_effects(restore.before)?;
    validate_effects(restore.desired)?;
    if restore.operation_id.is_nil()
        || !restore.managed_root.is_absolute()
        || !restore.staging_root.is_absolute()
    {
        return Err(DomainError::validation(
            "effect restore requires an operation identity and absolute local roots",
        ));
    }
    let managed_root = files::directory(restore.managed_root)?;
    let staging_root = files::directory(restore.staging_root)?;
    if staging_root.starts_with(&managed_root) || managed_root.starts_with(&staging_root) {
        return Err(DomainError::validation(
            "effect staging must be separate from managed effects",
        ));
    }
    for search in registry.search_paths() {
        let search = search.canonicalize().unwrap_or_else(|_| search.clone());
        if staging_root.starts_with(search) {
            return Err(DomainError::validation(
                "effect staging directory is inside a discovery root",
            ));
        }
    }
    let input_hash = files::digest(
        &serde_json::to_vec(&(
            restore.operation_id,
            restore.before,
            restore.desired,
            &managed_root,
        ))
        .map_err(files::io)?,
    );
    let changed: BTreeSet<_> = restore
        .before
        .keys()
        .chain(restore.desired.keys())
        .copied()
        .filter(|id| restore.before.get(id) != restore.desired.get(id))
        .collect();
    let plan_path = staging_root.join("effects-plan.json");
    let plan = if plan_path.exists() {
        let plan: EffectPlan = serde_json::from_reader(
            hypercolor_platform_fs::open_no_follow(&plan_path).map_err(files::io)?,
        )
        .map_err(files::io)?;
        if plan.schema_version != 1
            || plan.operation_id != restore.operation_id
            || plan.input_hash != input_hash
        {
            return Err(DomainError::conflict(
                "effect staging belongs to a different restore operation",
            ));
        }
        validate_plan(&plan, &changed, restore.before)?;
        plan
    } else {
        let mut retired = BTreeMap::new();
        for id in &changed {
            let existing = registry.get(&EffectId::new(*id));
            match (restore.before.get(id), existing) {
                (Some(expected), Some(entry)) => {
                    retired.insert(
                        *id,
                        capture_retirement(entry, expected, &managed_root, restore.blobs)?,
                    );
                }
                (Some(_), None) => {
                    return Err(DomainError::conflict(
                        "custom effect disappeared before restore preparation",
                    ));
                }
                (None, Some(_)) => {
                    return Err(DomainError::conflict(
                        "restored effect identity already belongs to another source",
                    ));
                }
                (None, None) => {}
            }
        }
        let plan = EffectPlan {
            schema_version: 1,
            operation_id: restore.operation_id,
            input_hash,
            retired,
        };
        // Only private staging is durable here. The caller records its global
        // intent before invoking any participant's persistence method.
        files::write_bytes(
            &plan_path,
            &serde_json::to_vec_pretty(&plan).map_err(files::io)?,
        )?;
        plan
    };
    validate_plan(&plan, &changed, restore.before)?;
    for (id, retired) in &plan.retired {
        for descriptor in retired.files.values() {
            restore.blobs.read(descriptor).map_err(files::io)?;
        }
        if let Some(entry) = registry.get(&EffectId::new(*id)) {
            let old = managed_root
                .join(&retired.root)
                .join(&restore.before[id].entrypoint);
            let new = restore.desired.get(id).map(|effect| {
                destination(&managed_root, restore.operation_id, *id).join(&effect.entrypoint)
            });
            if !matches!(entry.metadata.source, EffectSource::Html { .. })
                || (entry.source_path != old && new.as_ref() != Some(&entry.source_path))
            {
                return Err(DomainError::conflict(
                    "effect identity changed while restore was interrupted",
                ));
            }
        }
    }
    for id in changed.iter().filter(|id| !plan.retired.contains_key(id)) {
        if let Some(entry) = registry.get(&EffectId::new(*id)) {
            let expected = restore.desired.get(id).map(|effect| {
                destination(&managed_root, restore.operation_id, *id).join(&effect.entrypoint)
            });
            if !matches!(entry.metadata.source, EffectSource::Html { .. })
                || expected.as_ref() != Some(&entry.source_path)
            {
                return Err(DomainError::conflict(
                    "restored effect identity already belongs to another source",
                ));
            }
        }
    }
    let mut candidate = registry.clone();
    for id in &changed {
        candidate.remove(&EffectId::new(*id));
        if let Some(effect) = restore.desired.get(id) {
            let stage =
                files::nested_directory(&staging_root, Path::new(&format!("effects/{id}")))?;
            for (path, descriptor) in &effect.files {
                restore.blobs.read(descriptor).map_err(files::io)?;
                files::copy_blob(restore.blobs, descriptor, &stage, Path::new(path))?;
            }
            let manifest = manifest(*id, effect);
            files::write_bytes(
                &stage.join(HTML_EFFECT_BUNDLE_MANIFEST),
                &serde_json::to_vec(&manifest).map_err(files::io)?,
            )?;
            let mut entry = load_html_effect_file(&stage.join(&effect.entrypoint))
                .map_err(|error| DomainError::validation(error.message))?
                .ok_or_else(|| DomainError::validation("restored HTML effect is not supported"))?;
            let final_path =
                destination(&managed_root, restore.operation_id, *id).join(&effect.entrypoint);
            entry.source_path.clone_from(&final_path);
            entry.metadata.source = EffectSource::Html { path: final_path };
            if entry.metadata.name != effect.name {
                return Err(DomainError::validation(
                    "effect name differs from its HTML metadata",
                ));
            }
            candidate.register(entry);
        }
    }
    Ok(PreparedPortableEffects {
        _update_guard: update_guard,
        registry,
        candidate: Some(candidate),
        desired: restore.desired.clone(),
        changed,
        plan,
        blobs: restore.blobs.clone(),
        managed_root,
        staging_root,
        durable: false,
    })
}

impl PreparedPortableEffects {
    pub(crate) fn candidate(&self) -> &EffectRegistry {
        self.candidate
            .as_ref()
            .expect("effect restore has not published")
    }

    /// Called on the coordinator's blocking worker. Errors preserve all guards
    /// and the plan so the same participant can resume after partial progress.
    pub(crate) fn persist(
        &mut self,
        shutdown: &PersistenceShutdown,
    ) -> Result<AtomicWriteWaitOutcome, DomainError> {
        if self.durable {
            return Ok(AtomicWriteWaitOutcome::Written);
        }
        for id in &self.changed {
            if shutdown.is_shutdown() {
                return Ok(AtomicWriteWaitOutcome::ShutdownPending);
            }
            if let Some(effect) = self.desired.get(id) {
                self.persist_bundle(*id, effect, shutdown)?;
                if shutdown.is_shutdown() {
                    return Ok(AtomicWriteWaitOutcome::ShutdownPending);
                }
            }
        }
        for (id, retired) in &self.plan.retired {
            if shutdown.is_shutdown() {
                return Ok(AtomicWriteWaitOutcome::ShutdownPending);
            }
            if retired.bundled {
                let skip = self
                    .changed
                    .iter()
                    .filter(|id| self.desired.contains_key(id))
                    .map(|id| destination(&self.managed_root, self.plan.operation_id, *id))
                    .collect();
                let present = files::inventory(&self.managed_root.join(&retired.root), &skip)?;
                if present.iter().any(|path| !retired.files.contains_key(path)) {
                    return Err(DomainError::conflict(
                        "effect bundle acquired an unowned resource during restore",
                    ));
                }
            }
            let archive =
                files::nested_directory(&self.staging_root, Path::new(&format!("retired/{id}")))?;
            // The old manifest remains until every resource HTML has retired.
            // Startup can never reinterpret those resources as loose effects.
            let paths = retired
                .files
                .keys()
                .filter(|path| path.as_str() != HTML_EFFECT_BUNDLE_MANIFEST)
                .chain(
                    retired
                        .files
                        .keys()
                        .filter(|path| path.as_str() == HTML_EFFECT_BUNDLE_MANIFEST),
                );
            for path in paths {
                if shutdown.is_shutdown() {
                    return Ok(AtomicWriteWaitOutcome::ShutdownPending);
                }
                let descriptor = &retired.files[path];
                let source = self.managed_root.join(&retired.root).join(path);
                if !files::verify(&source, descriptor)?
                    && !files::verify(&archive.join(path), descriptor)?
                {
                    return Err(DomainError::conflict(
                        "retired effect bytes disappeared outside the restore transaction",
                    ));
                }
                files::copy_blob(&self.blobs, descriptor, &archive, Path::new(path))?;
                if shutdown.is_shutdown() {
                    return Ok(AtomicWriteWaitOutcome::ShutdownPending);
                }
                files::remove_verified(&source, descriptor)?;
            }
        }
        for id in &self.changed {
            if shutdown.is_shutdown() {
                return Ok(AtomicWriteWaitOutcome::ShutdownPending);
            }
            if let Some(effect) = self.desired.get(id) {
                let root = destination(&self.managed_root, self.plan.operation_id, *id);
                verify_bundle(&root, *id, effect)?;
                let marker = root.join(HTML_EFFECT_BUNDLE_PENDING);
                match fs::remove_file(&marker) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(files::io(error)),
                }
                files::sync_directory(&root)?;
            }
        }
        self.durable = true;
        Ok(AtomicWriteWaitOutcome::Written)
    }

    fn persist_bundle(
        &self,
        id: Uuid,
        effect: &PortableEffect,
        shutdown: &PersistenceShutdown,
    ) -> Result<(), DomainError> {
        let leaf = format!("portable-{id}-{}", self.plan.operation_id);
        let root = files::nested_directory(&self.managed_root, Path::new(&leaf))?;
        let marker = root.join(HTML_EFFECT_BUNDLE_PENDING);
        let present = files::inventory(&root, &BTreeSet::new())?;
        if present.iter().any(|path| {
            !effect.files.contains_key(path)
                && path != HTML_EFFECT_BUNDLE_MANIFEST
                && path != HTML_EFFECT_BUNDLE_PENDING
        }) {
            return Err(DomainError::conflict(
                "effect destination contains an unowned resource",
            ));
        }
        if root.join(HTML_EFFECT_BUNDLE_MANIFEST).exists() && !marker.exists() {
            return verify_bundle(&root, id, effect);
        }
        if shutdown.is_shutdown() {
            return Ok(());
        }
        files::write_bytes(&marker, self.plan.input_hash.as_bytes())?;
        for (path, descriptor) in &effect.files {
            if shutdown.is_shutdown() {
                return Ok(());
            }
            files::copy_blob(&self.blobs, descriptor, &root, Path::new(path))?;
        }
        if shutdown.is_shutdown() {
            return Ok(());
        }
        files::write_bytes(
            &root.join(HTML_EFFECT_BUNDLE_MANIFEST),
            &serde_json::to_vec(&manifest(id, effect)).map_err(files::io)?,
        )?;
        verify_bundle(&root, id, effect)
    }

    pub(crate) fn publish(&mut self) {
        assert!(
            self.durable,
            "effect restore must be durable before publication"
        );
        *self.registry = self
            .candidate
            .take()
            .expect("effect restore publishes once");
    }
}

fn destination(root: &Path, operation_id: Uuid, effect_id: Uuid) -> PathBuf {
    root.join(format!("portable-{effect_id}-{operation_id}"))
}

fn manifest(id: Uuid, effect: &PortableEffect) -> HtmlEffectBundleManifest {
    HtmlEffectBundleManifest {
        schema_version: 1,
        id: EffectId::new(id),
        entrypoint: effect.entrypoint.clone(),
    }
}

fn validate_effects(effects: &BTreeMap<Uuid, PortableEffect>) -> Result<(), DomainError> {
    for (id, effect) in effects {
        if id.is_nil() || !effect.files.contains_key(&effect.entrypoint) {
            return Err(DomainError::validation(
                "effect identity or entrypoint is missing",
            ));
        }
        let mut names = BTreeSet::new();
        for (path, descriptor) in &effect.files {
            if !is_portable_bundle_path(path)
                || !names.insert(path.to_lowercase())
                || Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.eq_ignore_ascii_case(HTML_EFFECT_BUNDLE_MANIFEST)
                            || name.eq_ignore_ascii_case(HTML_EFFECT_BUNDLE_PENDING)
                    })
            {
                return Err(DomainError::validation(
                    "effect resource path is invalid or reserved",
                ));
            }
            descriptor.validate().map_err(files::io)?;
        }
    }
    Ok(())
}

fn validate_plan(
    plan: &EffectPlan,
    changed: &BTreeSet<Uuid>,
    before: &BTreeMap<Uuid, PortableEffect>,
) -> Result<(), DomainError> {
    let expected_ids: BTreeSet<_> = changed
        .iter()
        .filter(|id| before.contains_key(id))
        .copied()
        .collect();
    if plan.retired.keys().copied().collect::<BTreeSet<_>>() != expected_ids {
        return Err(DomainError::conflict(
            "effect retirement plan is incomplete",
        ));
    }
    for (id, retired) in &plan.retired {
        let expected = before
            .get(id)
            .filter(|_| changed.contains(id))
            .ok_or_else(|| {
                DomainError::conflict("effect staging plan has an unrelated retirement")
            })?;
        if retired.root.is_absolute()
            || retired
                .root
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(DomainError::conflict(
                "effect retirement path escapes the managed root",
            ));
        }
        let mut expected_paths: BTreeSet<_> = if retired.bundled {
            expected.files.keys().cloned().collect()
        } else {
            BTreeSet::from([expected.entrypoint.clone()])
        };
        if retired.bundled {
            expected_paths.insert(HTML_EFFECT_BUNDLE_MANIFEST.to_owned());
        }
        if retired.files.keys().cloned().collect::<BTreeSet<_>>() != expected_paths {
            return Err(DomainError::conflict(
                "effect retirement resource list is incomplete",
            ));
        }
        for (path, descriptor) in &retired.files {
            descriptor.validate().map_err(files::io)?;
            if !is_portable_bundle_path(path)
                || (path != HTML_EFFECT_BUNDLE_MANIFEST
                    && expected.files.get(path) != Some(descriptor))
            {
                return Err(DomainError::conflict(
                    "effect retirement bytes differ from the authored snapshot",
                ));
            }
        }
    }
    Ok(())
}

fn capture_retirement(
    entry: &EffectEntry,
    expected: &PortableEffect,
    managed_root: &Path,
    blobs: &SetupBlobStore,
) -> Result<RetiredEffect, DomainError> {
    if !matches!(entry.metadata.source, EffectSource::Html { .. })
        || entry.metadata.name != expected.name
    {
        return Err(DomainError::conflict(
            "effect ownership changed before restore",
        ));
    }
    let source = entry.source_path.canonicalize().map_err(files::io)?;
    if !source.starts_with(managed_root) {
        return Err(DomainError::conflict(
            "move this custom effect into the managed user effects directory before restoring it",
        ));
    }
    let bundle =
        html_effect_bundle(&source).map_err(|error| DomainError::validation(error.message))?;
    let root = bundle.as_ref().map_or_else(
        || source.parent().expect("effect parent").to_path_buf(),
        |bundle| bundle.root.clone(),
    );
    if !root.starts_with(managed_root) {
        return Err(DomainError::conflict(
            "effect bundle is not owned by the managed root",
        ));
    }
    let entrypoint = source
        .strip_prefix(&root)
        .map_err(files::io)?
        .to_string_lossy()
        .replace('\\', "/");
    if entrypoint != expected.entrypoint {
        return Err(DomainError::conflict(
            "effect entrypoint changed before restore",
        ));
    }
    if let Some(bundle) = &bundle {
        if bundle.manifest.id != entry.metadata.id {
            return Err(DomainError::conflict("effect bundle identity changed"));
        }
        let mut expected_paths: BTreeSet<_> = expected.files.keys().cloned().collect();
        expected_paths.insert(HTML_EFFECT_BUNDLE_MANIFEST.to_owned());
        if files::inventory(&root, &BTreeSet::new())? != expected_paths {
            return Err(DomainError::conflict(
                "effect bundle resources changed before restore",
            ));
        }
    }
    for (path, descriptor) in &expected.files {
        if !files::verify(&root.join(path), descriptor)? {
            return Err(DomainError::conflict(
                "effect resource disappeared before restore",
            ));
        }
    }
    for (path, descriptor) in &expected.files {
        if blobs.capture(&root.join(path)).map_err(files::io)? != *descriptor {
            return Err(DomainError::conflict(
                "effect resource changed during capture",
            ));
        }
    }
    let mut retired_files = if bundle.is_some() {
        expected.files.clone()
    } else {
        BTreeMap::from([(entrypoint.clone(), expected.files[&entrypoint].clone())])
    };
    if bundle.is_some() {
        retired_files.insert(
            HTML_EFFECT_BUNDLE_MANIFEST.to_owned(),
            blobs
                .capture(&root.join(HTML_EFFECT_BUNDLE_MANIFEST))
                .map_err(files::io)?,
        );
    }
    Ok(RetiredEffect {
        bundled: bundle.is_some(),
        root: root
            .strip_prefix(managed_root)
            .map_err(files::io)?
            .to_path_buf(),
        files: retired_files,
    })
}

fn verify_bundle(root: &Path, id: Uuid, effect: &PortableEffect) -> Result<(), DomainError> {
    let bytes = serde_json::to_vec(&manifest(id, effect)).map_err(files::io)?;
    if !files::verify(
        &root.join(HTML_EFFECT_BUNDLE_MANIFEST),
        &PortableBlob {
            sha256: files::digest(&bytes),
            byte_len: u64::try_from(bytes.len()).map_err(files::io)?,
        },
    )? {
        return Err(DomainError::conflict("restored effect manifest is missing"));
    }
    for (path, descriptor) in &effect.files {
        if !files::verify(&root.join(path), descriptor)? {
            return Err(DomainError::conflict("restored effect resource is missing"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
