use std::sync::Arc;

use hypercolor_core::input::screen::{
    ResolvedScreenPublicationDescriptor, ScreenNativePreparationPayload,
    ScreenNativeTargetPreparation, ScreenNativeTargetPreparer,
};

struct TestNativeTargetPreparer {
    retained_bytes: u64,
}

pub const RETAINED_BYTES: u64 = 8_192;

impl ScreenNativeTargetPreparer for TestNativeTargetPreparer {
    fn prepare(
        &self,
        _descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        Ok(ScreenNativeTargetPreparation::new(
            platform.clone(),
            self.retained_bytes,
        ))
    }
}

pub fn preparer() -> Arc<dyn ScreenNativeTargetPreparer> {
    preparer_with_bytes(RETAINED_BYTES)
}

pub fn preparer_with_bytes(retained_bytes: u64) -> Arc<dyn ScreenNativeTargetPreparer> {
    Arc::new(TestNativeTargetPreparer { retained_bytes })
}
