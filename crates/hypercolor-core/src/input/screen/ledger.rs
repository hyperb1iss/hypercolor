use std::sync::Arc;

use thiserror::Error;

use super::plan::ScreenExternalResourceAdmission;
use super::{
    AdmittedScreenNativeTargetPreparation, ResolvedScreenPublicationDescriptor,
    ScreenByteReservation, ScreenExactResource, ScreenExactResourceLedger,
    ScreenNativeExecutionTarget, ScreenNativePreparationPayload, ScreenPlanError,
    ScreenPreparedWorkerToken, ScreenResourceLifetime, ScreenWorkerPreparationTicket,
};

/// Ticket-scoped construction of one exhaustive exact worker ledger.
#[derive(Debug)]
pub struct ScreenWorkerExactLedgerBuilder {
    ticket: ScreenWorkerPreparationTicket,
    reported_bytes: Vec<Option<u64>>,
    additional_resources: Vec<ScreenExactResource>,
    admission_top_ups: Vec<ScreenByteReservation>,
    external_admissions: Vec<ScreenExternalResourceAdmission>,
}

impl ScreenWorkerExactLedgerBuilder {
    /// Prepare explicit report slots for every required resource scope.
    ///
    /// # Errors
    ///
    /// Returns an allocation failure before any resource lifetime is bound.
    pub fn new(
        ticket: ScreenWorkerPreparationTicket,
    ) -> Result<Self, ScreenWorkerLedgerBuildError> {
        let mut reported_bytes = Vec::new();
        reported_bytes
            .try_reserve_exact(ticket.required_minimums().len())
            .map_err(|_| ScreenWorkerLedgerBuildError::AllocationFailed)?;
        reported_bytes.resize(ticket.required_minimums().len(), None);
        Ok(Self {
            ticket,
            reported_bytes,
            additional_resources: Vec::new(),
            admission_top_ups: Vec::new(),
            external_admissions: Vec::new(),
        })
    }

    /// Reserve unmodeled exact bytes before allocating their backing storage.
    pub fn preflight_additional_bytes(
        &mut self,
        bytes: u64,
    ) -> Result<(), ScreenWorkerLedgerBuildError> {
        let reservation = self.ticket.reserve_additional_exact_bytes(bytes)?;
        self.admission_top_ups
            .try_reserve(1)
            .map_err(|_| ScreenWorkerLedgerBuildError::AllocationFailed)?;
        self.admission_top_ups.push(reservation);
        Ok(())
    }

    /// Quote, admit, prepare, and report one native renderer target.
    ///
    /// The returned preparation and this builder share one dedicated byte
    /// lease. Dropping either side cannot leave renderer backing unadmitted;
    /// [`Self::finish`] installs the same lease on its exact lifetime.
    ///
    /// # Errors
    ///
    /// Propagates target contract, admission, and renderer preparation errors.
    pub fn prepare_native_target(
        &mut self,
        target: &ScreenNativeExecutionTarget,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
        resource_name: impl Into<Arc<str>>,
        accounting_scope: impl Into<Arc<str>>,
    ) -> anyhow::Result<AdmittedScreenNativeTargetPreparation> {
        let quote = target.quote_preparation(descriptor, platform)?;
        self.additional_resources
            .try_reserve(1)
            .map_err(|_| ScreenWorkerLedgerBuildError::AllocationFailed)?;
        self.external_admissions
            .try_reserve(1)
            .map_err(|_| ScreenWorkerLedgerBuildError::AllocationFailed)?;
        let reservation = self
            .ticket
            .reserve_additional_exact_bytes(quote.retained_bytes())?;
        let preparation = target.prepare_quoted(descriptor, platform, quote)?;
        let resource = preparation.exact_resource(resource_name, accounting_scope)?;
        let resource_name = Arc::clone(resource.name());
        self.report_native_target(resource)?;
        let lease = reservation.freeze();
        self.external_admissions
            .push(ScreenExternalResourceAdmission::new(
                resource_name,
                lease.clone(),
            ));
        Ok(AdmittedScreenNativeTargetPreparation::new(
            preparation,
            lease,
        ))
    }

    /// Source- and transaction-bound worker ticket being reported.
    #[must_use]
    pub const fn ticket(&self) -> &ScreenWorkerPreparationTicket {
        &self.ticket
    }

    /// Number of resource lifetimes that [`Self::finish`] will retain.
    ///
    /// # Errors
    ///
    /// Returns an allocation failure when the combined count overflows.
    pub fn prospective_resource_count(&self) -> Result<usize, ScreenWorkerLedgerBuildError> {
        self.reported_bytes
            .len()
            .checked_add(self.additional_resources.len())
            .ok_or(ScreenWorkerLedgerBuildError::AllocationFailed)
    }

    /// Report the actual retained bytes for one required named scope.
    ///
    /// # Errors
    ///
    /// Rejects unknown, repeated, or understated scopes.
    pub fn report(
        &mut self,
        name: &str,
        actual_bytes: u64,
    ) -> Result<(), ScreenWorkerLedgerBuildError> {
        let index = self
            .ticket
            .required_minimums()
            .binary_search_by(|minimum| minimum.name().as_ref().cmp(name))
            .map_err(|_| ScreenWorkerLedgerBuildError::UnknownResource {
                name: Arc::from(name),
            })?;
        let minimum = &self.ticket.required_minimums()[index];
        if self.reported_bytes[index].is_some() {
            return Err(ScreenWorkerLedgerBuildError::DuplicateResource {
                name: Arc::clone(minimum.name()),
            });
        }
        if actual_bytes < minimum.minimum_bytes() {
            return Err(ScreenWorkerLedgerBuildError::UnderstatedResource {
                name: Arc::clone(minimum.name()),
                minimum: minimum.minimum_bytes(),
                actual: actual_bytes,
            });
        }
        self.reported_bytes[index] = Some(actual_bytes);
        Ok(())
    }

    /// Report one additional allocation under a required accounting scope.
    ///
    /// The resource category is inherited from the scope, so platform workers
    /// cannot accidentally relabel bytes while describing concrete storage.
    ///
    /// # Errors
    ///
    /// Rejects unknown scopes, repeated names, invalid names, and allocation
    /// failure while retaining prior reports.
    pub fn report_scoped(
        &mut self,
        name: &str,
        accounting_scope: &str,
        actual_bytes: u64,
    ) -> Result<(), ScreenWorkerLedgerBuildError> {
        let scope_index = self
            .ticket
            .required_minimums()
            .binary_search_by(|minimum| minimum.name().as_ref().cmp(accounting_scope))
            .map_err(|_| ScreenWorkerLedgerBuildError::UnknownResourceScope {
                name: Arc::from(name),
                scope: Arc::from(accounting_scope),
            })?;
        let duplicate_required = self
            .ticket
            .required_minimums()
            .binary_search_by(|minimum| minimum.name().as_ref().cmp(name))
            .is_ok();
        let duplicate_additional = self
            .additional_resources
            .iter()
            .any(|resource| resource.name().as_ref() == name);
        if duplicate_required || duplicate_additional {
            return Err(ScreenWorkerLedgerBuildError::DuplicateResource {
                name: Arc::from(name),
            });
        }
        self.additional_resources
            .try_reserve(1)
            .map_err(|_| ScreenWorkerLedgerBuildError::AllocationFailed)?;
        let scope = &self.ticket.required_minimums()[scope_index];
        let resource = ScreenExactResource::try_new_scoped(
            Arc::from(name),
            Arc::clone(scope.name()),
            scope.resource(),
            actual_bytes,
        )?;
        self.additional_resources.push(resource);
        Ok(())
    }

    /// Report one renderer preparation through its target-bound ledger entry.
    ///
    /// # Errors
    ///
    /// Rejects generic worker resources, unknown or non-runtime scopes,
    /// repeated names, and allocation failure while retaining prior reports.
    pub(crate) fn report_native_target(
        &mut self,
        resource: ScreenExactResource,
    ) -> Result<(), ScreenWorkerLedgerBuildError> {
        if resource.native_binding().is_none() {
            return Err(ScreenWorkerLedgerBuildError::UnboundNativeTargetResource {
                name: Arc::clone(resource.name()),
            });
        }
        let scope_index = self
            .ticket
            .required_minimums()
            .binary_search_by(|minimum| {
                minimum
                    .name()
                    .as_ref()
                    .cmp(resource.accounting_scope().as_ref())
            })
            .map_err(|_| ScreenWorkerLedgerBuildError::UnknownResourceScope {
                name: Arc::clone(resource.name()),
                scope: Arc::clone(resource.accounting_scope()),
            })?;
        let scope = &self.ticket.required_minimums()[scope_index];
        if scope.resource() != super::ScreenResourceKind::WorkerAdditional {
            return Err(ScreenWorkerLedgerBuildError::InvalidNativeTargetScope {
                name: Arc::clone(resource.name()),
                scope: Arc::clone(resource.accounting_scope()),
            });
        }
        let duplicate_required = self
            .ticket
            .required_minimums()
            .binary_search_by(|minimum| minimum.name().as_ref().cmp(resource.name().as_ref()))
            .is_ok();
        let duplicate_additional = self
            .additional_resources
            .iter()
            .any(|retained| retained.name() == resource.name());
        if duplicate_required || duplicate_additional {
            return Err(ScreenWorkerLedgerBuildError::DuplicateResource {
                name: Arc::clone(resource.name()),
            });
        }
        self.additional_resources
            .try_reserve(1)
            .map_err(|_| ScreenWorkerLedgerBuildError::AllocationFailed)?;
        self.additional_resources.push(resource);
        Ok(())
    }

    /// Bind every reported allocation lifetime and produce its prepared token.
    ///
    /// # Errors
    ///
    /// Rejects omitted scopes, allocation failures, and ticket validation
    /// failures without exposing a partial token.
    pub fn finish(self) -> Result<ScreenWorkerExactLedger, ScreenWorkerLedgerBuildError> {
        let missing = self
            .reported_bytes
            .iter()
            .position(Option::is_none)
            .map(|index| Arc::clone(self.ticket.required_minimums()[index].name()));
        if let Some(name) = missing {
            return Err(ScreenWorkerLedgerBuildError::MissingResource { name });
        }

        let resource_count = self.prospective_resource_count()?;
        let mut resources = Vec::new();
        resources
            .try_reserve_exact(resource_count)
            .map_err(|_| ScreenWorkerLedgerBuildError::AllocationFailed)?;
        let mut lifetimes = Vec::new();
        lifetimes
            .try_reserve_exact(resource_count)
            .map_err(|_| ScreenWorkerLedgerBuildError::AllocationFailed)?;
        for (minimum, actual_bytes) in self
            .ticket
            .required_minimums()
            .iter()
            .zip(self.reported_bytes)
        {
            let resource = ScreenExactResource::try_new(
                Arc::clone(minimum.name()),
                minimum.resource(),
                actual_bytes.expect("missing reports return before resource construction"),
            )?;
            lifetimes.push(self.ticket.bind_resource_lifetime(&resource)?);
            resources.push(resource);
        }
        for resource in self.additional_resources {
            lifetimes.push(self.ticket.bind_resource_lifetime(&resource)?);
            resources.push(resource);
        }
        let exact_ledger = ScreenExactResourceLedger::try_new(resources)?;
        let lifetimes = lifetimes.into_boxed_slice();
        let token = self.ticket.acknowledge_with_external_admission(
            exact_ledger,
            &lifetimes,
            self.admission_top_ups,
            self.external_admissions,
        )?;
        Ok(ScreenWorkerExactLedger { token, lifetimes })
    }
}

/// Prepared worker token paired with every worker-owned allocation lifetime.
#[derive(Debug)]
pub struct ScreenWorkerExactLedger {
    token: ScreenPreparedWorkerToken,
    lifetimes: Box<[ScreenResourceLifetime]>,
}

impl ScreenWorkerExactLedger {
    /// Token acknowledged against the ticket's exhaustive exact ledger.
    #[must_use]
    pub const fn token(&self) -> &ScreenPreparedWorkerToken {
        &self.token
    }

    /// Worker-owned allocation lifetimes retained through activation.
    #[must_use]
    pub fn lifetimes(&self) -> &[ScreenResourceLifetime] {
        &self.lifetimes
    }

    /// Separate the opaque token from worker-owned allocation lifetimes.
    #[must_use]
    pub fn into_parts(self) -> (ScreenPreparedWorkerToken, Box<[ScreenResourceLifetime]>) {
        (self.token, self.lifetimes)
    }
}

/// Failure to construct one exhaustive ticket-scoped exact ledger.
#[derive(Debug, Error)]
pub enum ScreenWorkerLedgerBuildError {
    /// Report metadata could not be reserved.
    #[error("failed to allocate exact worker ledger report metadata")]
    AllocationFailed,
    /// Caller named no required allocation scope.
    #[error("unknown exact worker resource scope {name}")]
    UnknownResource { name: Arc<str> },
    /// An additional allocation named no required accounting scope.
    #[error("exact worker resource {name} references unknown accounting scope {scope}")]
    UnknownResourceScope { name: Arc<str>, scope: Arc<str> },
    /// A generic worker entry was presented as a renderer-bound allocation.
    #[error("exact worker resource {name} is not bound to a native target")]
    UnboundNativeTargetResource { name: Arc<str> },
    /// Native renderer allocations must belong to the runtime accounting scope.
    #[error("native target resource {name} references non-runtime accounting scope {scope}")]
    InvalidNativeTargetScope { name: Arc<str>, scope: Arc<str> },
    /// Caller reported one required allocation scope twice.
    #[error("duplicate exact worker resource scope {name}")]
    DuplicateResource { name: Arc<str> },
    /// Caller omitted one required allocation scope.
    #[error("missing exact worker resource scope {name}")]
    MissingResource { name: Arc<str> },
    /// Actual retained storage is smaller than the planner-required minimum.
    #[error("exact worker resource {name} is understated: minimum {minimum}, actual {actual}")]
    UnderstatedResource {
        name: Arc<str>,
        minimum: u64,
        actual: u64,
    },
    /// Ticket resource construction or acknowledgement failed.
    #[error(transparent)]
    Plan(#[from] ScreenPlanError),
}
