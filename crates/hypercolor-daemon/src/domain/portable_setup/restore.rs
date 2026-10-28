//! Cancellation-safe restore ownership and durable transport receipts.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use hypercolor_types::portable_setup::{PortableBackupV1, PortableSetupV1, SetupBindings};
use uuid::Uuid;

use super::journal::{BackupAncillary, RestoreJournal, RestoreRecovery};
use super::{
    PortableSetupService, RestoreOptions, RestoreReceipt, RestoreReport, RestoreRuntime,
    SetupIdentity, internal,
};
use crate::domain::DomainError;
use crate::extensions::ShutdownPersistenceDisposition;
use crate::persistence::PersistenceShutdown;

pub(super) struct ActiveRestore {
    runtime: Arc<RestoreRuntime>,
    pub(super) shutdown: PersistenceShutdown,
}

impl ActiveRestore {
    pub(super) fn begin(runtime: Arc<RestoreRuntime>) -> Self {
        let shutdown = PersistenceShutdown::default();
        *runtime
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(shutdown.clone());
        if runtime.closed.load(Ordering::Acquire) {
            shutdown.writers_stopped();
        }
        Self { runtime, shutdown }
    }
}

impl Drop for ActiveRestore {
    fn drop(&mut self) {
        self.runtime
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

impl PortableSetupService {
    pub(crate) fn has_pending_restore(&self) -> Result<bool, DomainError> {
        super::pending_restore_exists(&self.state.state_dir)
    }

    pub(crate) async fn reconcile_active_bindings(&self) -> Result<(), DomainError> {
        if let Some(active) = self.read_active_projection()? {
            self.reconcile_bindings(SetupIdentity {
                setup_id: active.receipt.setup_id,
                name: active.receipt.applied.manifest.name,
            })
            .await?;
        }
        Ok(())
    }
    /// Resume durable local intents before transports admit new setup changes.
    pub async fn recover(&self) -> Result<(), DomainError> {
        let service = self.clone();
        tokio::spawn(async move {
            let _gate = service.restore_runtime.gate.lock().await;
            service.ensure_restore_open()?;
            service.recover_pending_locked().await?;
            Ok(())
        })
        .await
        .map_err(internal)?
    }

    /// Recover the public commit before letting a transport advance its own ACK.
    pub async fn last_restore_receipt(
        &self,
        setup_id: Uuid,
    ) -> Result<Option<RestoreReceipt>, DomainError> {
        let service = self.clone();
        tokio::spawn(async move {
            let _gate = service.restore_runtime.gate.lock().await;
            service.ensure_restore_open()?;
            service.recover_pending_locked().await?;
            service.recover_locked(setup_id).await?;
            match service.read_journal(setup_id)? {
                Some(RestoreJournal::Committed { receipt, .. }) => Ok(Some(*receipt)),
                None => Ok(None),
                Some(RestoreJournal::Pending { .. }) => {
                    Err(DomainError::conflict("setup restoration remains pending"))
                }
            }
        })
        .await
        .map_err(internal)?
    }

    /// Dropping the caller never drops admitted writes or their domain guards.
    pub async fn restore(
        &self,
        manifest: &PortableSetupV1,
        bindings: &SetupBindings,
        options: RestoreOptions,
    ) -> Result<RestoreReport, DomainError> {
        self.restore_request(manifest, bindings, options, None)
            .await
    }

    /// Restore lighting and manual backup choices through one durable intent.
    pub async fn restore_backup(
        &self,
        backup: &PortableBackupV1,
        bindings: &SetupBindings,
        options: RestoreOptions,
    ) -> Result<RestoreReport, DomainError> {
        self.restore_backup_checked(
            backup,
            bindings,
            options,
            super::BackupRestoreExpectation::default(),
        )
        .await
    }

    /// Restore a confirmed archive only if its projected preferences still match.
    pub async fn restore_backup_checked(
        &self,
        backup: &PortableBackupV1,
        bindings: &SetupBindings,
        options: RestoreOptions,
        expected: super::BackupRestoreExpectation,
    ) -> Result<RestoreReport, DomainError> {
        backup
            .validate()
            .map_err(|error| DomainError::validation(error.to_string()))?;
        self.restore_request(
            &backup.setup,
            bindings,
            options,
            Some(BackupAncillary {
                expected_preferences: expected.preferences,
                preferences: backup.preferences.clone(),
                favorites: backup.favorites.clone(),
                tui_preferences: backup.tui_preferences.clone(),
                expected_tui_preferences: expected.tui_preferences,
            }),
        )
        .await
    }

    async fn restore_request(
        &self,
        manifest: &PortableSetupV1,
        bindings: &SetupBindings,
        options: RestoreOptions,
        ancillary: Option<BackupAncillary>,
    ) -> Result<RestoreReport, DomainError> {
        let service = self.clone();
        let manifest = manifest.clone();
        let bindings = bindings.clone();
        let requested_revision = match &ancillary {
            Some(choices) => super::fingerprint(&(&manifest, choices))?,
            None => super::fingerprint(&manifest)?,
        };
        tokio::spawn(async move {
            let _gate = service.restore_runtime.gate.lock().await;
            service.ensure_restore_open()?;
            service.recover_pending_locked().await?;
            service.recover_locked(manifest.setup_id).await?;
            if let Some(RestoreJournal::Committed { receipt, .. }) =
                service.read_journal(manifest.setup_id)?
                && receipt.operation_id == options.operation_id
            {
                if receipt.requested_revision != requested_revision {
                    return Err(DomainError::conflict(
                        "restore operation identity was reused for different content",
                    ));
                }
                return Ok(RestoreReport {
                    applied: receipt.applied,
                    unresolved_bindings: receipt.unresolved_bindings,
                });
            }
            let current = service
                .export_raw(
                    SetupIdentity {
                        setup_id: manifest.setup_id,
                        name: manifest.name.clone(),
                    },
                    &bindings,
                )
                .await?;
            if current.revision != options.expected_revision {
                return Err(DomainError::conflict(
                    "lighting changed before setup restoration",
                ));
            }
            let candidate = super::plan::candidate(
                &current.manifest,
                &manifest,
                &options.mode,
                options.apply_selection,
            )?;
            let active = ActiveRestore::begin(Arc::clone(&service.restore_runtime));
            Box::pin(service.apply_candidate(
                current.manifest,
                candidate,
                requested_revision,
                bindings,
                options,
                &active.shutdown,
                None,
                ancillary,
            ))
            .await
        })
        .await
        .map_err(internal)?
    }

    pub async fn shutdown_when_writers_closed(
        &self,
    ) -> Result<ShutdownPersistenceDisposition, DomainError> {
        self.restore_runtime.closed.store(true, Ordering::Release);
        self.restore_runtime.closed_notify.notify_waiters();
        if let Some(shutdown) = self
            .restore_runtime
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            shutdown.writers_stopped();
        }
        let _gate = self.restore_runtime.gate.lock().await;
        let pending = self.restore_runtime.pending.load(Ordering::Acquire)
            || self.journal_ids()?.into_iter().any(|id| {
                !matches!(
                    self.read_journal(id),
                    Ok(None
                        | Some(RestoreJournal::Committed {
                            finalized: true,
                            ..
                        }))
                )
            });
        Ok(if pending {
            ShutdownPersistenceDisposition::PreservePendingTransaction
        } else {
            ShutdownPersistenceDisposition::SaveCurrentState
        })
    }

    pub(super) fn ensure_restore_open(&self) -> Result<(), DomainError> {
        if self.restore_runtime.closed.load(Ordering::Acquire) {
            Err(DomainError::conflict("Hypercolor is shutting down"))
        } else {
            Ok(())
        }
    }

    fn journal_ids(&self) -> Result<Vec<Uuid>, DomainError> {
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(internal)? {
            let entry = entry.map_err(internal)?;
            let name = entry.file_name();
            let Some(name) = name
                .to_str()
                .and_then(|name| name.strip_suffix(".transaction.json"))
            else {
                continue;
            };
            ids.push(name.parse().map_err(internal)?);
        }
        ids.sort_unstable();
        Ok(ids)
    }

    pub(super) async fn recover_locked(&self, setup_id: Uuid) -> Result<(), DomainError> {
        let journal = self.read_journal(setup_id)?;
        if let Some(RestoreJournal::Committed {
            receipt, finalized, ..
        }) = &journal
        {
            if *finalized {
                return Ok(());
            }
            let active = ActiveRestore::begin(Arc::clone(&self.restore_runtime));
            self.finish_committed(receipt.setup_id, &active.shutdown)
                .await?;
            return Ok(());
        }
        let Some(RestoreJournal::Pending {
            before,
            candidate,
            requested_revision,
            bindings: previous_bindings,
            options,
            runtime,
            ancillary,
            ..
        }) = journal
        else {
            return Ok(());
        };
        self.restore_runtime.pending.store(true, Ordering::Release);
        let active = ActiveRestore::begin(Arc::clone(&self.restore_runtime));
        let bindings = self.assign_bindings(setup_id, Some(&candidate)).await?;
        Box::pin(self.apply_candidate(
            *before,
            *candidate,
            requested_revision,
            bindings,
            options,
            &active.shutdown,
            Some(RestoreRecovery {
                runtime,
                bindings: previous_bindings,
            }),
            ancillary,
        ))
        .await?;
        Ok(())
    }

    pub(super) async fn finish_committed(
        &self,
        setup_id: Uuid,
        shutdown: &PersistenceShutdown,
    ) -> Result<(), DomainError> {
        let Some(RestoreJournal::Committed {
            receipt,
            finalized: false,
            reconnect_network_outputs,
            ancillary,
            ..
        }) = self.read_journal(setup_id)?
        else {
            return Ok(());
        };
        let closed = self.restore_runtime.closed_notify.notified();
        tokio::pin!(closed);
        closed.as_mut().enable();
        self.ensure_restore_open()?;
        tokio::select! {
            () = &mut closed => return Err(DomainError::conflict("runtime reconciliation is saved for the next start")),
            result = async {
                let outcome = super::media::finalize_media(
                    Arc::clone(&self.state.asset_library), receipt.operation_id,
                    &receipt.applied.manifest.assets, shutdown,
                ).await?;
                super::transaction::written(outcome)?;
                if let Some(preferences) = ancillary.as_ref().and_then(|choices| choices.preferences.as_ref()) {
                    self.reconcile_runtime_preferences(preferences).await?;
                }
                let devices = receipt.materialized_bindings.devices.keys().copied().collect::<Vec<_>>();
                self.reconcile_runtime_outputs(&devices, reconnect_network_outputs).await
            } => result?,
        }
        let service = self.clone();
        let shutdown = shutdown.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            super::transaction::written(service.write_active_projection(&receipt, &shutdown)?)?;
            service.write_journal(
                setup_id,
                &RestoreJournal::Committed {
                    schema_version: 1,
                    receipt,
                    finalized: true,
                    reconnect_network_outputs,
                    ancillary,
                },
                &shutdown,
            )
        })
        .await
        .map_err(internal)??;
        super::transaction::written(outcome)?;
        self.restore_runtime.pending.store(false, Ordering::Release);
        Ok(())
    }

    pub(super) async fn recover_pending_locked(&self) -> Result<(), DomainError> {
        for setup_id in self.journal_ids()? {
            if matches!(
                self.read_journal(setup_id)?,
                Some(
                    RestoreJournal::Pending { .. }
                        | RestoreJournal::Committed {
                            finalized: false,
                            ..
                        }
                )
            ) {
                self.recover_locked(setup_id).await?;
            }
        }
        self.restore_runtime.pending.store(false, Ordering::Release);
        Ok(())
    }
}
