use crate::install::{
    InstallPlatform, InstallPlatformError, InstallationState, PlatformCheckpoint,
    PlatformOwnerReceipt, PlatformState, PlatformTransactionRecord, PlatformTransitionStates,
    PreparedPlatformTransaction, UnitId, UnitRecord,
};

pub(super) struct PriorProof {
    pub(super) matches: bool,
}

impl PriorProof {
    pub(super) fn valid() -> Self {
        Self { matches: true }
    }
}

macro_rules! unexpected_mutations {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $output:ty;)*) => {
        $(fn $name(&mut self, $($arg: $ty),*) -> $output {
            panic!(concat!(stringify!($name), " is forbidden during locator preparation"))
        })*
    };
}

impl InstallPlatform for PriorProof {
    fn validate_transaction_plan(
        &mut self,
        _prior: &PlatformState,
        _target: &PlatformState,
        _transitions: &PlatformTransitionStates,
        _count: u16,
        _record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        Ok(())
    }

    fn matches_exact_state(
        &mut self,
        checkpoint: PlatformCheckpoint,
        _expected: &PlatformState,
        index: u16,
        _record: &PlatformTransactionRecord,
        receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<bool, InstallPlatformError> {
        assert_eq!(checkpoint, PlatformCheckpoint::PriorOriginal);
        assert_eq!(index, 0);
        assert!(receipt.is_none());
        Ok(self.matches)
    }

    unexpected_mutations! {
        inspect() -> Result<PlatformState, InstallPlatformError>;
        prepare_transaction(_candidate: &UnitRecord, _prior: &InstallationState,
            _target: &PlatformState) -> Result<PreparedPlatformTransaction, InstallPlatformError>;
        capture_candidate_owner_receipt(_expected: &PlatformState,
            _record: &PlatformTransactionRecord) -> Result<PlatformOwnerReceipt, InstallPlatformError>;
        preflight_authority(_candidate: &UnitId, _prior: &InstallationState,
            _record: &PlatformTransactionRecord) -> Result<(), InstallPlatformError>;
        wait_for_guard_release(_unloaded: &PlatformState,
            _record: &PlatformTransactionRecord) -> Result<(), InstallPlatformError>;
        install_launcher(_checkpoint: PlatformCheckpoint, _unit: Option<&UnitId>,
            _record: &PlatformTransactionRecord) -> Result<(), InstallPlatformError>;
        install_layout_operation(_checkpoint: PlatformCheckpoint, _unit: Option<&UnitId>,
            _index: u16, _record: &PlatformTransactionRecord) -> Result<(), InstallPlatformError>;
        reload_manager(_expected: &PlatformState, _record: &PlatformTransactionRecord)
            -> Result<(), InstallPlatformError>;
        restore_autostart(_expected: &PlatformState, _record: &PlatformTransactionRecord)
            -> Result<(), InstallPlatformError>;
        restore_runtime(_expected: &PlatformState, _record: &PlatformTransactionRecord,
            _receipt: Option<&PlatformOwnerReceipt>) -> Result<(), InstallPlatformError>;
        wait_for_newer_owner(_checkpoint: PlatformCheckpoint, _expected: &PlatformState,
            _record: &PlatformTransactionRecord, _receipt: Option<&PlatformOwnerReceipt>)
            -> Result<(), InstallPlatformError>;
    }
}
