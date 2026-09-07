use std::sync::Arc;

use hypercolor_core::input::screen::planner::{
    ResolvedScreenPublicationDescriptor, ScreenNativePreparationPayload,
    ScreenNativeTargetPreparation, ScreenNativeTargetPreparer,
};

struct TestNativeTargetPreparer {
    retained_bytes: u64,
}

#[derive(Debug)]
pub struct TestNativeTargetPayload;

pub const RETAINED_BYTES: u64 = 8_192;

impl ScreenNativeTargetPreparer for TestNativeTargetPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        Ok(self.retained_bytes)
    }

    fn prepare(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        Ok(ScreenNativeTargetPreparation::new(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(TestNativeTargetPayload),
            ),
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
