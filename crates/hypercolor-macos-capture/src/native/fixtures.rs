use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::*;

struct FixtureScreenshotCall {
    filter_id: u64,
    dynamic_range: MacosCaptureDynamicRange,
    completion: ScreenshotImageCompletion,
}

#[derive(Default)]
pub(super) struct FixtureScreenshotBackend {
    calls: Mutex<VecDeque<FixtureScreenshotCall>>,
}

impl FixtureScreenshotBackend {
    pub(super) fn calls(&self) -> Vec<(u64, MacosCaptureDynamicRange)> {
        super::lock(&self.calls)
            .iter()
            .map(|call| (call.filter_id, call.dynamic_range))
            .collect()
    }

    pub(super) fn complete_next(
        &self,
        result: Result<MacosScreenshotReferenceImage, MacosCaptureError>,
    ) {
        let call = super::lock(&self.calls)
            .pop_front()
            .expect("fixture callback should be pending");
        (call.completion)(result);
    }
}

impl ScreenshotCaptureBackend for FixtureScreenshotBackend {
    fn capture(
        &self,
        filter: ScreenshotFilterHandle,
        dynamic_range: MacosCaptureDynamicRange,
        _cursor_composed: bool,
        completion: ScreenshotImageCompletion,
    ) -> Result<(), MacosCaptureError> {
        let ScreenshotFilterHandle::Fixture(filter_id) = filter else {
            panic!("fixture backend requires a fixture filter");
        };
        super::lock(&self.calls).push_back(FixtureScreenshotCall {
            filter_id,
            dynamic_range,
            completion,
        });
        Ok(())
    }
}

pub(super) struct FixtureScreenshotFence {
    pub(super) identity: Mutex<(Arc<str>, u64, u64)>,
}

impl ScreenshotIdentityFence for FixtureScreenshotFence {
    fn matches(&self, source_id: &str, generation: u64, revision: u64) -> bool {
        let identity = super::lock(&self.identity);
        identity.0.as_ref() == source_id && identity.1 == generation && identity.2 == revision
    }
}

pub(super) fn screenshot_fixture(
    capability: MacosScreenshotReferenceCapability,
) -> (
    ScreenshotTransactionSnapshot,
    Arc<FixtureScreenshotFence>,
    Arc<FixtureScreenshotBackend>,
) {
    let (source_id, generation) = match &capability {
        MacosScreenshotReferenceCapability::PendingFirstFrame => (Arc::from("pending"), 0),
        MacosScreenshotReferenceCapability::SdrOnly {
            source_id,
            generation,
        }
        | MacosScreenshotReferenceCapability::PairedSdrHdr {
            source_id,
            generation,
        } => (Arc::clone(source_id), *generation),
    };
    let selection_revision = 11;
    (
        ScreenshotTransactionSnapshot {
            filter: ScreenshotFilterHandle::Fixture(7),
            source_id: Arc::clone(&source_id),
            generation,
            selection_revision,
            capability,
        },
        Arc::new(FixtureScreenshotFence {
            identity: Mutex::new((source_id, generation, selection_revision)),
        }),
        Arc::new(FixtureScreenshotBackend::default()),
    )
}

pub(super) const ABSENT_TAHOE_PROBES: MacosTahoeRuntimeProbes = MacosTahoeRuntimeProbes {
    content_tone_mapping_info_symbol: MacosRuntimeCapability::Absent,
    screenshot_configuration_class: MacosRuntimeCapability::Absent,
    screenshot_dynamic_range_selector: MacosRuntimeCapability::Absent,
    screenshot_capture_selector: MacosRuntimeCapability::Absent,
};

pub(super) fn stream_slot_fixture(current_epoch: u64, selection_revision: u64) -> Arc<StreamSlot> {
    let shared = Arc::new(SessionShared::new(
        MacosProtectedSourceState::Live,
        super::MacosCaptureSelector::Auto,
        MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
    ));
    shared.set_capture_active(true);
    shared.activate_epoch(current_epoch);
    let streams = StreamSlot::new(shared, MacosStreamRequest::default())
        .expect("fixture native lifecycle starts");
    {
        let mut state = super::lock(&streams.state);
        state.selection_revision = selection_revision;
        state.selected_filter = Some(NativeSelectionFilter::fixture(1));
        state.fixture_current_epoch = (current_epoch != 0).then_some(current_epoch);
    }
    streams
}

pub(super) fn reserve_selection_candidate_fixture(
    streams: &StreamSlot,
    epoch: u64,
    request: MacosStreamRequest,
    selection_id: u64,
) -> Result<Option<(CandidateStage, Option<NativeStream>)>, MacosCaptureError> {
    streams
        .reserve_candidate_stage(
            epoch,
            request,
            Some(NativeSelectionFilter::fixture(selection_id)),
            None,
            None,
        )
        .map(|reservation| {
            reservation.map(|reservation| {
                StreamSlot::finish_replaced_candidate(reservation.replaced_settlement);
                (reservation.stage, reservation.replaced)
            })
        })
}

pub(super) fn reserve_request_candidate_fixture(
    streams: &StreamSlot,
    epoch: u64,
    request: MacosStreamRequest,
    pending: PendingStreamRequest,
) -> Result<Option<(CandidateStage, Option<NativeStream>)>, MacosCaptureError> {
    streams
        .reserve_candidate_stage(epoch, request, None, None, Some(pending))
        .map(|reservation| {
            reservation.map(|reservation| {
                StreamSlot::finish_replaced_candidate(reservation.replaced_settlement);
                (reservation.stage, reservation.replaced)
            })
        })
}

pub(super) fn pending_request(
    epoch: u64,
    request: MacosStreamRequest,
) -> (PendingStreamRequest, MacosStreamRequestTransaction) {
    let (transaction, completion) = stream_request_transaction(epoch);
    (
        PendingStreamRequest {
            epoch,
            request,
            completion,
        },
        transaction,
    )
}

pub(super) fn selection_filter_ids(streams: &StreamSlot) -> (Option<u64>, Option<(u64, u64)>) {
    let state = super::lock(&streams.state);
    (
        state
            .selected_filter
            .as_ref()
            .map(NativeSelectionFilter::fixture_id),
        state
            .pending_selection
            .as_ref()
            .map(|pending| (pending.epoch, pending.selection_filter.fixture_id())),
    )
}

pub(super) fn sdr_delivery_fixture() -> MacosValidatedStreamDelivery {
    let configured = MacosConfiguredStream {
        requested_dynamic_range: MacosCaptureDynamicRange::Sdr,
        requested_preset: MacosStreamPreset::SdrDefault,
        configured_dynamic_range: MacosCaptureDynamicRange::Sdr,
        configured_pixel_format: MacosCapturePixelFormat::Bgra8,
        configured_color_range: MacosColorRange::Full,
    };
    let delivered = MacosDeliveredFrameMetadata::new(
        MacosCapturePixelFormat::Bgra8,
        MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::Srgb,
            transfer: MacosTransferFunction::Srgb,
            matrix: None,
            range: MacosColorRange::Full,
            chroma_location: None,
        },
        None,
        None,
    )
    .expect("fixture delivery metadata should be valid");
    MacosValidatedStreamDelivery {
        configured,
        delivered,
    }
}
