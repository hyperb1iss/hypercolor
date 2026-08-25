#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Duration;

    use super::fixtures::*;

    use super::{
        CandidateReservation, InterruptedRestage, InterruptionRecoveryPhase,
        MacosCaptureColorimetry, MacosCaptureDynamicRange, MacosCaptureError,
        MacosCapturePixelFormat, MacosColorPrimaries, MacosColorRange, MacosConfiguredStream,
        MacosDeliveredFrameMetadata, MacosFrameEvent, MacosFrameStatus, MacosHostArchitecture,
        MacosNativeTransactionError, MacosNativeTransactionPhase, MacosPixelExtent,
        MacosProtectedSourceState, MacosRuntimeCapability, MacosStreamDeliveryRejection,
        MacosStreamDeliveryState, MacosStreamDeliveryValidator, MacosStreamPreset,
        MacosTahoeCapabilities, MacosTahoeRuntimeProbes, MacosTransferFunction,
        MacosValidatedStreamDelivery, NativeSelectionFilter, PoolBackingLifetime, PoolObservation,
        SCCaptureDynamicRange, SCStreamConfiguration, SCStreamConfigurationPreset,
        ScreenshotCaptureBackend, ScreenshotIdentityFence, SessionShared, SourceResolution,
        StreamSlot, SysctlI32Value, capture_capabilities_from_probes, capture_dynamic_range,
        classify_delivery_error, color_range_from_fourcc, conservative_pool_quote,
        execute_screenshot_transaction, is_hypercolor_ui_bundle_identifier,
        route_retained_delivery, route_stream_activity, route_stream_lifecycle,
        session_selection_source_id, with_admitted_surface,
    };
    use crate::worker::{LatestSampleWorker, SamplePublishOutcome};
    use crate::{
        MacosCaptureCadence, MacosScreenshotReferenceCapability, MacosScreenshotReferenceImage,
        MacosScreenshotReferenceSet, MacosStreamRequest,
    };

    #[test]
    fn fractional_dirty_rects_round_outward_to_cover_the_damage() {
        let rect = objc2_core_foundation::CGRect {
            origin: objc2_core_foundation::CGPoint { x: 10.25, y: 7.5 },
            size: objc2_core_foundation::CGSize {
                width: 99.5,
                height: 41.25,
            },
        };
        let pixel = super::pixel_rect_from_cg(rect).expect("fractional rect must decode");
        assert_eq!(
            (pixel.x, pixel.y, pixel.width, pixel.height),
            (10, 7, 100, 42),
        );
    }

    #[test]
    fn chroma_location_defaults_to_left_when_unsignalled() {
        let mut raw: *mut objc2_core_video::CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: The out-pointer is a valid stack slot and no attribute
        // dictionary is supplied, matching the documented contract.
        let status = unsafe {
            objc2_core_video::CVPixelBufferCreate(
                None,
                4,
                4,
                0x3432_3076,
                None,
                std::ptr::NonNull::from(&mut raw),
            )
        };
        assert_eq!(status, 0, "CVPixelBufferCreate failed: {status}");
        // SAFETY: A zero status guarantees a retained, non-null buffer that
        // this test now owns.
        let buffer = unsafe {
            objc2_core_foundation::CFRetained::from_raw(
                std::ptr::NonNull::new(raw).expect("created pixel buffer"),
            )
        };

        let location = super::chroma_location(&buffer);
        assert!(
            matches!(location, Ok(crate::MacosChromaLocation::Left)),
            "unsignalled chroma location must default to left siting, got {location:?}",
        );
    }

    #[test]
    fn hypercolor_ui_exclusion_matches_only_the_stable_app_bundle() {
        assert!(is_hypercolor_ui_bundle_identifier(
            "tech.hyperbliss.hypercolor"
        ));
        assert!(!is_hypercolor_ui_bundle_identifier(
            "tech.hyperbliss.hypercolor.daemon"
        ));
        assert!(!is_hypercolor_ui_bundle_identifier(
            "com.example.hypercolor"
        ));
    }

    #[test]
    fn stream_selection_revision_advances_monotonically_across_lifecycles() {
        let shared = Arc::new(SessionShared::new(
            MacosProtectedSourceState::ReadyIdle,
            super::MacosCaptureSelector::Auto,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        ));
        let streams = StreamSlot::new(shared, MacosStreamRequest::default())
            .expect("fixture native lifecycle starts");
        assert_eq!(streams.selection_revision(), 0);

        assert!(streams.set_capture_active(true));
        assert!(streams.set_capture_active(false));
        assert_eq!(streams.selection_revision(), 1);

        assert!(streams.set_capture_active(true));
        assert!(streams.set_capture_active(false));
        assert_eq!(streams.selection_revision(), 2);
    }

    #[test]
    fn incomplete_native_delivery_never_enters_the_latest_frame_slot() {
        let latest_frame_slot_called = AtomicBool::new(false);
        let lifecycle_called = AtomicBool::new(false);

        route_retained_delivery(
            super::RetainedNativeDelivery::<()>::Lifecycle(MacosFrameStatus::Idle),
            |_| latest_frame_slot_called.store(true, Ordering::Release),
            |status| {
                assert_eq!(status, MacosFrameStatus::Idle);
                lifecycle_called.store(true, Ordering::Release);
            },
        );

        assert!(!latest_frame_slot_called.load(Ordering::Acquire));
        assert!(lifecycle_called.load(Ordering::Acquire));
    }

    #[test]
    fn current_publication_holds_lifecycle_until_publish_precedes_deactivation() {
        let streams = stream_slot_fixture(41, 9);
        let (publishing_tx, publishing_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let publishing_streams = Arc::clone(&streams);
        let publisher = thread::spawn(move || {
            publishing_streams.publish_decoded_event_with(41, false, None, || {
                publishing_tx
                    .send(())
                    .expect("publication should be observable");
                release_rx.recv().expect("publication should resume");
                publishing_streams
                    .shared
                    .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
            })
        });
        publishing_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("current publication should hold the lifecycle gate");
        assert!(streams.state.try_lock().is_ok());

        let (deactivation_started_tx, deactivation_started_rx) = mpsc::channel();
        let (deactivated_tx, deactivated_rx) = mpsc::channel();
        let deactivating_streams = Arc::clone(&streams);
        let deactivator = thread::spawn(move || {
            deactivation_started_tx
                .send(())
                .expect("deactivation attempt should be observable");
            let changed = deactivating_streams.set_capture_active(false);
            deactivating_streams
                .shared
                .set_status(MacosProtectedSourceState::ReadyIdle);
            deactivated_tx
                .send(changed)
                .expect("deactivation should be observable");
        });
        deactivation_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deactivation should reach the lifecycle gate");
        assert_eq!(
            deactivated_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        assert_eq!(streams.shared.current_epoch(), 41);

        release_tx.send(()).expect("publication should be released");
        assert!(publisher.join().expect("publisher thread should join"));
        assert!(
            deactivated_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("deactivation should follow publication")
        );
        deactivator.join().expect("deactivation thread should join");
        assert_eq!(streams.shared.current_epoch(), 0);
        assert_eq!(
            streams.shared.status(),
            MacosProtectedSourceState::ReadyIdle
        );
    }

    #[test]
    fn candidate_first_frame_publish_holds_lifecycle_until_deactivation() {
        let streams = stream_slot_fixture(41, 9);
        let (stage, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 2)
                .expect("candidate reservation should succeed")
                .expect("active capture should admit the candidate");
        assert!(streams.start_candidate_fixture(stage));
        let (publishing_tx, publishing_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let publishing_streams = Arc::clone(&streams);
        let publisher = thread::spawn(move || {
            publishing_streams.publish_decoded_event_with(
                42,
                true,
                Some(sdr_delivery_fixture()),
                || {
                    publishing_tx
                        .send(())
                        .expect("first-frame publication should be observable");
                    release_rx.recv().expect("publication should resume");
                    publishing_streams
                        .shared
                        .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
                },
            )
        });
        publishing_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("candidate activation should reach publication under the lifecycle gate");
        assert!(streams.state.try_lock().is_ok());
        assert_eq!(streams.shared.current_epoch(), 42);
        assert_eq!(selection_filter_ids(&streams), (Some(2), None));

        let (deactivation_started_tx, deactivation_started_rx) = mpsc::channel();
        let (deactivated_tx, deactivated_rx) = mpsc::channel();
        let deactivating_streams = Arc::clone(&streams);
        let deactivator = thread::spawn(move || {
            deactivation_started_tx
                .send(())
                .expect("deactivation attempt should be observable");
            let changed = deactivating_streams.set_capture_active(false);
            deactivating_streams
                .shared
                .set_status(MacosProtectedSourceState::ReadyIdle);
            deactivated_tx
                .send(changed)
                .expect("deactivation should be observable");
        });
        deactivation_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deactivation should reach the lifecycle gate");
        assert_eq!(
            deactivated_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );

        release_tx.send(()).expect("publication should be released");
        assert!(publisher.join().expect("publisher thread should join"));
        assert!(
            deactivated_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("deactivation should follow first-frame publication")
        );
        deactivator.join().expect("deactivation thread should join");
        assert_eq!(streams.shared.current_epoch(), 0);
        assert_eq!(selection_filter_ids(&streams), (Some(2), None));
        assert_eq!(
            streams.shared.status(),
            MacosProtectedSourceState::ReadyIdle
        );
    }

    #[test]
    fn stale_picker_resolution_cannot_mutate_filter_acceptance() {
        let streams = stream_slot_fixture(41, 9);
        let stale = streams
            .begin_resolution()
            .expect("picker resolution should begin");
        streams.shared.enable_picker_callbacks(stale);
        let picker_resolution = streams
            .shared
            .picker_resolution()
            .expect("picker update should retain its exact resolution");
        let initial_revision = streams.selection_revision();
        let initial_selection = selection_filter_ids(&streams);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let accepting_streams = Arc::clone(&streams);
        let accepting = thread::spawn(move || {
            accepting_streams.accept_selection_filter_with_hooks(
                NativeSelectionFilter::fixture(2),
                MacosStreamRequest::default(),
                42,
                picker_resolution,
                true,
                (
                    || {
                        ready_tx
                            .send(())
                            .expect("retained picker filter should be observable");
                        release_rx
                            .recv()
                            .expect("picker filter acceptance should resume");
                    },
                    || panic!("stale picker filter must not be accepted"),
                ),
            )
        });
        ready_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("picker filter should pause before the lifecycle transition");

        let fresh = streams
            .begin_picker_resolution()
            .expect("newer picker resolution should begin");
        release_tx
            .send(())
            .expect("stale picker acceptance should resume");
        assert!(matches!(
            accepting.join().expect("acceptance thread should join"),
            Ok(super::FilterAcceptance::Stale)
        ));
        assert_eq!(streams.selection_revision(), initial_revision);
        assert_eq!(selection_filter_ids(&streams), initial_selection);

        let retry = streams
            .accept_selection_filter(
                NativeSelectionFilter::fixture(3),
                MacosStreamRequest::default(),
                43,
                fresh,
                true,
            )
            .expect("fresh resolution should be accepted");
        assert!(matches!(retry, super::FilterAcceptance::Candidate { .. }));
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((43, 3))));
    }

    fn install_live_successor(streams: &StreamSlot, epoch: u64) {
        let (stage, _) = reserve_selection_candidate_fixture(
            streams,
            epoch,
            MacosStreamRequest::default(),
            epoch,
        )
        .expect("successor reservation should succeed")
        .expect("active capture should admit the successor");
        assert!(streams.start_candidate_fixture(stage));
        assert!(streams.activate_candidate_fixture(epoch));
        assert!(streams.publish_decoded_event_with(epoch, false, None, || {
            streams
                .shared
                .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
        }));
    }

    fn assert_retired_error_cannot_overwrite_successor(fatal: bool) {
        let streams = stream_slot_fixture(41, 9);
        let error = MacosCaptureError::CaptureWorkerStartFailed(
            "retired injected stream failure".to_owned(),
        );
        let (retired_tx, retired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let failing_streams = Arc::clone(&streams);
        let failing_shared = Arc::clone(&streams.shared);
        let finalizer = thread::spawn(move || {
            let after_retirement = || {
                retired_tx
                    .send(())
                    .expect("retired stream should be observable");
                release_rx.recv().expect("error finalization should resume");
            };
            if fatal {
                super::handle_owned_fatal_stream_error_with(
                    &failing_streams,
                    41,
                    failing_shared,
                    error,
                    after_retirement,
                );
            } else {
                super::handle_owned_stream_error_with(
                    &failing_streams,
                    41,
                    &failing_shared,
                    MacosProtectedSourceState::PermissionDenied,
                    error,
                    after_retirement,
                );
            }
        });
        retired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("old error should pause after retirement");
        assert_eq!(streams.shared.current_epoch(), 0);

        install_live_successor(&streams, 42);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        release_tx
            .send(())
            .expect("old error finalization should resume");
        finalizer.join().expect("error finalizer should join");

        assert_eq!(streams.shared.current_epoch(), 42);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle)))
        ));
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::RecoverableError(_)))
        ));
    }

    #[test]
    fn ordinary_error_finalization_cannot_overwrite_live_successor() {
        assert_retired_error_cannot_overwrite_successor(false);
    }

    #[test]
    fn fatal_error_finalization_cannot_overwrite_live_successor() {
        assert_retired_error_cannot_overwrite_successor(true);
    }

    #[test]
    fn duplicate_fatal_callbacks_invalidate_the_owned_epoch_once() {
        let streams = stream_slot_fixture(41, 9);
        let error = MacosCaptureError::CaptureWorkerStartFailed(
            "duplicate fatal callback fixture".to_owned(),
        );

        super::handle_owned_fatal_stream_error(
            &streams,
            41,
            Arc::clone(&streams.shared),
            error.clone(),
        );
        super::handle_owned_fatal_stream_error(&streams, 41, Arc::clone(&streams.shared), error);

        assert!(matches!(
            streams.shared.mailbox.take_latest_with_generation(),
            Some((_, 1, Err(MacosCaptureError::CaptureWorkerStartFailed(_))))
        ));
        assert!(!streams.shared.mailbox.has_pending());
    }

    #[test]
    fn retired_preparation_failure_cannot_overwrite_live_successor() {
        let streams = stream_slot_fixture(41, 9);
        let removal = streams.remove(41, None);
        assert_eq!(removal.role, super::StreamRole::Current);
        assert_eq!(streams.shared.current_epoch(), 0);
        let recovery = InterruptedRestage::interrupted(41, 9);
        let reservation = streams
            .reserve_candidate_stage(
                42,
                MacosStreamRequest::default(),
                Some(NativeSelectionFilter::fixture(1)),
                Some(recovery),
                None,
            )
            .expect("interrupted restage should reserve")
            .expect("active capture should admit interrupted restage");
        let CandidateReservation {
            stage,
            replaced,
            replaced_settlement,
            ..
        } = reservation;
        StreamSlot::finish_replaced_candidate(replaced_settlement);
        assert!(replaced.is_none());
        let failure = streams.fail_candidate_preparation_fixture(
            stage,
            MacosCaptureError::CaptureWorkerStartFailed(
                "retired interrupted restage failed to prepare".to_owned(),
            ),
        );
        let (paused_tx, paused_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finalized_tx, finalized_rx) = mpsc::channel();
        let failing_streams = Arc::clone(&streams);
        let finalizer = thread::spawn(move || {
            let finalized =
                failing_streams.finalize_candidate_preparation_failure_with(failure, None, || {
                    paused_tx
                        .send(())
                        .expect("post-retirement finalization pause should be observable");
                    release_rx
                        .recv()
                        .expect("preparation finalization should resume");
                });
            finalized_tx
                .send(finalized)
                .expect("preparation finalization result should be observable");
        });
        paused_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("old preparation failure should pause after retirement");

        let (successor, _) =
            reserve_selection_candidate_fixture(&streams, 43, MacosStreamRequest::default(), 43)
                .expect("successor reservation should succeed")
                .expect("active capture should admit the successor");
        assert!(streams.start_candidate_fixture(successor));
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Starting);
        release_tx
            .send(())
            .expect("old preparation finalization should resume");
        assert!(
            !finalized_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("stale preparation finalization should finish")
        );
        finalizer.join().expect("preparation finalizer should join");

        assert_eq!(streams.shared.current_epoch(), 0);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Starting);
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started)))
        ));
        assert!(streams.activate_candidate_fixture(43));
        assert!(streams.publish_decoded_event_with(43, false, None, || {
            streams
                .shared
                .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
        }));
        assert_eq!(streams.shared.current_epoch(), 43);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle)))
        ));
    }

    #[test]
    fn preparation_failure_revision_rejects_request_only_aba() {
        let original = MacosStreamRequest::default();
        let request_a = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("first request-only candidate should be valid");
        let request_b = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(45), true)
            .expect("second request-only candidate should be valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = original;

        let (pending_a, completion_a) = pending_request(42, request_a);
        let (stage_a, _) = reserve_request_candidate_fixture(&streams, 42, request_a, pending_a)
            .expect("first request candidate should reserve")
            .expect("active capture should stage the first request candidate");
        assert_eq!(streams.selection_revision(), 9);
        let revision_a = stage_a.lifecycle_revision;
        let failure_a = streams.fail_candidate_preparation_fixture(
            stage_a,
            MacosCaptureError::CaptureWorkerStartFailed(
                "first request candidate failed to prepare".to_owned(),
            ),
        );
        assert!(failure_a.stage.lifecycle_revision > revision_a);
        let failure_a_revision = failure_a.stage.lifecycle_revision;
        assert_eq!(completion_a.try_recv(), Err(mpsc::TryRecvError::Empty));

        let (paused_tx, paused_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finalized_tx, finalized_rx) = mpsc::channel();
        let failing_streams = Arc::clone(&streams);
        let finalizer_a = thread::spawn(move || {
            let finalized = failing_streams.finalize_candidate_preparation_failure_with(
                failure_a,
                None,
                || {
                    paused_tx
                        .send(())
                        .expect("first finalizer pause should be observable");
                    release_rx.recv().expect("first finalizer should resume");
                },
            );
            finalized_tx
                .send(finalized)
                .expect("first finalizer result should be observable");
        });
        paused_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first finalizer should pause before lifecycle validation");
        assert_eq!(completion_a.try_recv(), Err(mpsc::TryRecvError::Empty));

        let (pending_b, completion_b) = pending_request(43, request_b);
        let (stage_b, _) = reserve_request_candidate_fixture(&streams, 43, request_b, pending_b)
            .expect("second request candidate should reserve")
            .expect("active capture should stage the second request candidate");
        assert_eq!(streams.selection_revision(), 9);
        assert!(stage_b.lifecycle_revision > failure_a_revision);
        let error_b = MacosCaptureError::CaptureWorkerStartFailed(
            "second request candidate failed to prepare".to_owned(),
        );
        let failure_b = streams.fail_candidate_preparation_fixture(stage_b, error_b.clone());
        assert!(failure_b.stage.lifecycle_revision > stage_b.lifecycle_revision);
        let failure_b_revision = failure_b.stage.lifecycle_revision;
        assert_eq!(completion_b.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(streams.finalize_candidate_preparation_failure(failure_b, None));
        assert!(
            completion_b
                .recv()
                .expect("second request should complete after finalization")
                .is_err()
        );
        assert!(super::lock(&streams.state).lifecycle_revision > failure_b_revision);
        assert_eq!(streams.selection_revision(), 9);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);

        release_tx
            .send(())
            .expect("first finalizer should resume after the ABA lifecycle");
        assert!(
            !finalized_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("stale first finalizer should finish")
        );
        finalizer_a.join().expect("first finalizer should join");
        assert!(
            completion_a
                .recv()
                .expect("first request should complete after stale finalization")
                .is_err()
        );

        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::RecoverableError(error))) if error.as_ref() == &error_b
        ));
    }

    #[test]
    fn current_inactive_epoch_rejects_queued_frame_publication() {
        let streams = stream_slot_fixture(41, 9);
        streams.record_stream_activity(41, false, false);
        let published = AtomicBool::new(false);

        assert!(!streams.publish_decoded_event_with(
            41,
            true,
            Some(sdr_delivery_fixture()),
            || {
                published.store(true, Ordering::Release);
                streams
                    .shared
                    .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
            },
        ));
        assert!(!published.load(Ordering::Acquire));
        assert_eq!(streams.shared.current_epoch(), 41);
        assert_eq!(
            streams.shared.status(),
            MacosProtectedSourceState::NeedsSelection
        );
    }

    #[test]
    fn terminal_lifecycle_generation_rejects_frames_until_stream_reactivation() {
        let streams = stream_slot_fixture(41, 9);
        assert!(streams.publish_stream_lifecycle(41, MacosFrameStatus::Suspended));
        let stale_published = AtomicBool::new(false);

        assert!(!streams.publish_decoded_event_with(41, true, None, || {
            stale_published.store(true, Ordering::Release);
        }));
        assert!(!stale_published.load(Ordering::Acquire));
        assert!(matches!(
            streams.shared.mailbox.take_latest_with_generation(),
            Some((
                _,
                1,
                Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Suspended))
            ))
        ));

        streams.record_stream_activity(41, true, false);
        let resumed_published = AtomicBool::new(false);
        assert!(streams.publish_decoded_event_with(41, true, None, || {
            resumed_published.store(true, Ordering::Release);
        }));
        assert!(resumed_published.load(Ordering::Acquire));
    }

    #[test]
    fn rejected_terminal_lifecycle_does_not_advance_decode_generation() {
        let streams = stream_slot_fixture(41, 9);
        let mut worker = LatestSampleWorker::spawn(
            "macos-capture-terminal-generation-test",
            |sample: ()| sample,
            |(), _publication| {},
        )
        .expect("generation worker should start");
        let samples = worker.input();
        let initial_generation = samples.generation();

        route_stream_lifecycle(&samples, &streams, 999, MacosFrameStatus::Suspended);
        assert_eq!(samples.generation(), initial_generation);

        route_stream_lifecycle(&samples, &streams, 41, MacosFrameStatus::Suspended);
        let terminal_generation = samples.generation();
        assert!(terminal_generation > initial_generation);

        route_stream_lifecycle(&samples, &streams, 41, MacosFrameStatus::Suspended);
        assert_eq!(samples.generation(), terminal_generation);

        worker.close();
        worker.join().expect("generation worker should join");
    }

    #[test]
    fn terminal_invalidation_crossing_cannot_publish_the_old_generation() {
        let streams = stream_slot_fixture(41, 9);
        let publish_streams = Arc::clone(&streams);
        let old_published = Arc::new(AtomicBool::new(false));
        let new_published = Arc::new(AtomicBool::new(false));
        let worker_old_published = Arc::clone(&old_published);
        let worker_new_published = Arc::clone(&new_published);
        let (publication_entered_tx, publication_entered_rx) = mpsc::sync_channel(1);
        let (release_publication_tx, release_publication_rx) = mpsc::sync_channel(1);
        let (publication_result_tx, publication_result_rx) = mpsc::sync_channel(2);
        let mut worker = LatestSampleWorker::spawn(
            "macos-capture-terminal-crossing-test",
            |sample| sample,
            move |sample, publication| {
                if sample == 1 {
                    publication_entered_tx
                        .send(())
                        .expect("old publication should hold the generation lock");
                    release_publication_rx
                        .recv()
                        .expect("old publication should resume");
                }
                let published = publish_streams.publish_decoded_event_if(
                    41,
                    true,
                    None,
                    || publication.is_current(),
                    || {
                        if sample == 1 {
                            worker_old_published.store(true, Ordering::Release);
                        } else {
                            worker_new_published.store(true, Ordering::Release);
                        }
                    },
                );
                publication_result_tx
                    .send((sample, published))
                    .expect("publication outcome should be observable");
            },
        )
        .expect("crossing worker should start");
        let samples = worker.input();

        assert_eq!(samples.publish(1), SamplePublishOutcome::Accepted);
        publication_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("old decode should enter generation-locked publication");

        let terminal_samples = samples.clone();
        let terminal_streams = Arc::clone(&streams);
        let (invalidation_requested_tx, invalidation_requested_rx) = mpsc::sync_channel(1);
        let (terminal_done_tx, terminal_done_rx) = mpsc::sync_channel(1);
        let terminal = thread::spawn(move || {
            let accepted = terminal_samples.invalidate_if_observed(
                || {
                    invalidation_requested_tx
                        .send(())
                        .expect("terminal invalidation request should be observable");
                },
                || terminal_streams.publish_stream_lifecycle(41, MacosFrameStatus::Suspended),
            );
            terminal_done_tx
                .send(accepted)
                .expect("terminal outcome should be observable");
        });
        invalidation_requested_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("terminal callback should request invalidation");

        let active_samples = samples.clone();
        let active_streams = Arc::clone(&streams);
        let (active_started_tx, active_started_rx) = mpsc::sync_channel(1);
        let (active_done_tx, active_done_rx) = mpsc::sync_channel(1);
        let active = thread::spawn(move || {
            active_started_tx
                .send(())
                .expect("active callback start should be observable");
            route_stream_activity(&active_samples, &active_streams, 41, true, false);
            active_done_tx
                .send(())
                .expect("active callback completion should be observable");
        });
        active_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("exact active callback should start after terminal invalidation");
        assert_eq!(
            active_done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        release_publication_tx
            .send(())
            .expect("old generation publication should resume");

        assert_eq!(
            publication_result_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("old publication should settle"),
            (1, false)
        );
        assert!(!old_published.load(Ordering::Acquire));
        assert!(
            terminal_done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("terminal transition should settle")
        );
        terminal.join().expect("terminal callback should join");
        active_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("exact active callback should follow terminal invalidation");
        active.join().expect("active callback should join");

        assert_eq!(samples.publish(2), SamplePublishOutcome::Accepted);
        assert_eq!(
            publication_result_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("new generation should publish"),
            (2, true)
        );
        assert!(new_published.load(Ordering::Acquire));

        worker.close();
        worker.join().expect("crossing worker should join");
    }

    #[test]
    fn candidate_terminal_lifecycle_blocks_first_frame_until_exact_reactivation() {
        for status in [MacosFrameStatus::Suspended, MacosFrameStatus::Stopped] {
            let streams = stream_slot_fixture(41, 9);
            let (stage, _) = reserve_selection_candidate_fixture(
                &streams,
                42,
                MacosStreamRequest::default(),
                42,
            )
            .expect("candidate reservation should succeed")
            .expect("active capture should admit the candidate");
            assert!(streams.start_candidate_fixture(stage));
            let revision = super::lock(&streams.state).lifecycle_revision;

            assert!(!streams.publish_stream_lifecycle(999, status));
            assert_eq!(super::lock(&streams.state).lifecycle_revision, revision);
            assert!(streams.publish_stream_lifecycle(42, status));
            let terminal_revision = super::lock(&streams.state).lifecycle_revision;
            assert!(terminal_revision > revision);
            assert!(!streams.publish_stream_lifecycle(42, status));
            assert_eq!(
                super::lock(&streams.state).lifecycle_revision,
                terminal_revision
            );
            assert!(!streams.shared.mailbox.has_pending());

            let stale_published = AtomicBool::new(false);
            assert!(!streams.publish_decoded_event_with(
                42,
                true,
                Some(sdr_delivery_fixture()),
                || stale_published.store(true, Ordering::Release),
            ));
            assert!(!stale_published.load(Ordering::Acquire));
            assert_eq!(streams.shared.current_epoch(), 41);
            assert_eq!(super::lock(&streams.state).candidate_epoch, Some(42));

            streams.record_stream_activity(42, true, false);
            let resumed_published = AtomicBool::new(false);
            assert!(streams.publish_decoded_event_with(
                42,
                true,
                Some(sdr_delivery_fixture()),
                || resumed_published.store(true, Ordering::Release),
            ));
            assert!(resumed_published.load(Ordering::Acquire));
            assert_eq!(streams.shared.current_epoch(), 42);
        }
    }

    #[test]
    fn candidate_inactive_epoch_rejects_first_frame_activation() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("candidate request should be valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("candidate reservation should succeed")
            .expect("active capture should admit the candidate");
        assert!(streams.start_candidate_fixture(stage));
        streams.record_stream_activity(42, false, false);
        let published = AtomicBool::new(false);

        assert!(!streams.publish_decoded_event_with(
            42,
            true,
            Some(sdr_delivery_fixture()),
            || published.store(true, Ordering::Release),
        ));
        assert!(!published.load(Ordering::Acquire));
        assert_eq!(streams.shared.current_epoch(), 41);
        assert_eq!(streams.committed_request(), original);
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));
        let state = super::lock(&streams.state);
        assert_eq!(state.candidate_epoch, Some(42));
        assert_eq!(
            state.pending_request.as_ref().map(|request| request.epoch),
            Some(42)
        );
    }

    #[test]
    fn selection_stage_adopting_a_pending_request_keeps_deadline_authority() {
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("candidate request should be valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = MacosStreamRequest::default();
        let (pending, transaction) = pending_request(42, next);
        reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request reservation should succeed")
            .expect("active capture should admit the candidate");

        reserve_selection_candidate_fixture(&streams, 43, next, 43)
            .expect("selection reservation should succeed")
            .expect("the selection stage should adopt the in-flight request");

        let armed = streams
            .arm_candidate_deadline(
                43,
                MacosNativeTransactionPhase::StreamStart,
                Duration::from_secs(5),
            )
            .expect("deadline arming should not error");
        assert!(
            armed,
            "the adopted transaction must answer to the stage that owns it now"
        );
        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));
        let state = super::lock(&streams.state);
        assert_eq!(
            state
                .candidate_completion
                .as_ref()
                .map(|completion| completion.identity().generation),
            Some(43)
        );
        assert_eq!(
            state.pending_request.as_ref().map(|request| request.epoch),
            Some(43)
        );
    }

    #[test]
    fn cancelling_an_adopted_request_tears_down_the_adopting_stage() {
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("candidate request should be valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = MacosStreamRequest::default();
        let (pending, transaction) = pending_request(42, next);
        reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request reservation should succeed")
            .expect("active capture should admit the candidate");
        let (stage, _) = reserve_selection_candidate_fixture(&streams, 43, next, 43)
            .expect("selection reservation should succeed")
            .expect("the selection stage should adopt the in-flight request");
        assert!(streams.start_candidate_fixture(stage));
        let cancel_streams = Arc::clone(&streams);
        {
            let state = super::lock(&streams.state);
            state
                .candidate_completion
                .as_ref()
                .expect("candidate completion is installed")
                .set_cancel(move |generation| {
                    cancel_streams.cancel_candidate_transaction(generation);
                });
        }

        assert!(transaction.cancel());

        let state = super::lock(&streams.state);
        assert_eq!(state.candidate_epoch, None);
        assert!(state.candidate_completion.is_none());
        assert!(state.pending_request.is_none());
    }

    #[test]
    fn display_current_inactive_callback_does_not_block_publication() {
        let streams = stream_slot_fixture(41, 9);
        streams.record_stream_activity(41, false, true);
        let published = AtomicBool::new(false);

        assert!(streams.publish_decoded_event_with(41, true, None, || {
            published.store(true, Ordering::Release);
            streams
                .shared
                .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
        }));
        assert!(published.load(Ordering::Acquire));
        assert_eq!(streams.shared.current_epoch(), 41);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
    }

    #[test]
    fn display_candidate_inactive_callback_does_not_strand_request() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("candidate request should be valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("candidate reservation should succeed")
            .expect("active capture should admit the candidate");
        assert!(streams.start_candidate_fixture(stage));
        streams.record_stream_activity(42, false, true);

        assert!(
            streams.publish_decoded_event_with(42, true, Some(sdr_delivery_fixture()), || {
                streams
                    .shared
                    .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
            },)
        );
        assert_eq!(completion.recv(), Ok(Ok(())));
        assert_eq!(streams.shared.current_epoch(), 42);
        assert_eq!(streams.committed_request(), next);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
    }

    fn assert_latest_lifecycle(streams: &StreamSlot, expected: MacosFrameStatus) {
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::Lifecycle(actual))) if actual == expected
        ));
    }

    #[test]
    fn stale_picker_cancel_cannot_overwrite_successor_starting() {
        let streams = stream_slot_fixture(0, 0);
        super::lock(&streams.state).selected_filter = None;
        let stale = streams
            .begin_picker_resolution()
            .expect("first picker resolution should begin");
        let fresh = streams
            .begin_picker_resolution()
            .expect("successor picker resolution should begin");
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started));

        assert!(!streams.finalize_picker_cancel(&stale));
        assert_eq!(streams.shared.picker_resolution(), Some(fresh));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Starting);
        assert_latest_lifecycle(&streams, MacosFrameStatus::Started);
    }

    #[test]
    fn stale_picker_failure_cannot_overwrite_successor_live() {
        let streams = stream_slot_fixture(41, 9);
        let stale = streams
            .begin_picker_resolution()
            .expect("first picker resolution should begin");
        let fresh = streams
            .begin_picker_resolution()
            .expect("successor picker resolution should begin");
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));

        assert!(!streams.finalize_picker_failure(
            &stale,
            MacosCaptureError::CaptureWorkerStartFailed("stale picker failure".to_owned()),
        ));
        assert_eq!(streams.shared.picker_resolution(), Some(fresh));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        assert_latest_lifecycle(&streams, MacosFrameStatus::Idle);
    }

    #[test]
    fn stale_filter_error_cannot_overwrite_successor_starting() {
        let streams = stream_slot_fixture(0, 0);
        super::lock(&streams.state).selected_filter = None;
        let stale = streams
            .begin_picker_resolution()
            .expect("picker filter resolution should begin");
        let fresh = streams
            .begin_picker_resolution()
            .expect("successor filter resolution should begin");
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started));

        assert!(!streams.finalize_resolution_error(
            &stale,
            true,
            MacosCaptureError::RetainNativeFilterFailed,
        ));
        assert_eq!(streams.shared.picker_resolution(), Some(fresh));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Starting);
        assert_latest_lifecycle(&streams, MacosFrameStatus::Started);
    }

    #[test]
    fn stale_enumeration_error_cannot_overwrite_successor_live() {
        let streams = stream_slot_fixture(41, 9);
        let stale = streams
            .begin_resolution()
            .expect("first enumeration should begin");
        let fresh = streams
            .begin_resolution()
            .expect("successor enumeration should begin");
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));

        assert!(!streams.finalize_resolution_error(
            &stale,
            false,
            MacosCaptureError::MissingShareableContent,
        ));
        assert!(streams.shared.source_resolution_is_current(&fresh));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        assert_latest_lifecycle(&streams, MacosFrameStatus::Idle);
    }

    #[test]
    fn diagnostic_selector_remains_primary_across_concurrent_set_selector() {
        let streams = stream_slot_fixture(0, 7);
        let (diagnostic, completion) = streams
            .begin_restart_diagnostic(true, 7)
            .expect("diagnostic resolution should begin");
        let resolution = SourceResolution::Diagnostic(diagnostic.clone());
        assert_eq!(
            resolution.selector(),
            &super::MacosCaptureSelector::PrimaryDisplay
        );

        let lifecycle = super::lock(&streams.lifecycle_start);
        let (started_tx, started_rx) = mpsc::channel();
        let mutating_streams = Arc::clone(&streams);
        let mutation = thread::spawn(move || {
            started_tx
                .send(())
                .expect("selector mutation should be observable");
            mutating_streams
                .set_selector_and_begin_resolution(super::MacosCaptureSelector::Auto)
                .expect("selector mutation should begin its own resolution")
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("selector mutation should reach the lifecycle gate");
        assert_eq!(
            streams.shared.selector(),
            super::MacosCaptureSelector::PrimaryDisplay
        );
        drop(lifecycle);
        let successor = mutation.join().expect("selector mutation should join");

        assert_eq!(streams.shared.selector(), super::MacosCaptureSelector::Auto);
        assert_eq!(successor.selector(), &super::MacosCaptureSelector::Auto);
        assert_eq!(
            resolution.selector(),
            &super::MacosCaptureSelector::PrimaryDisplay
        );
        assert_eq!(completion.recv(), Ok(MacosProtectedSourceState::Failed));
    }

    #[test]
    fn diagnostic_setup_fences_old_filter_acceptance_and_new_picker_resolution() {
        let streams = stream_slot_fixture(41, 9);
        let stale_resolution = streams
            .begin_picker_resolution()
            .expect("old picker resolution should begin");
        let (setup_tx, setup_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let setup_streams = Arc::clone(&streams);
        let setup = thread::spawn(move || {
            setup_streams.setup_restart_diagnostic_with(true, || {
                setup_tx
                    .send(())
                    .expect("installed diagnostic setup should be observable");
                release_rx.recv().expect("diagnostic setup should resume");
            })
        });
        setup_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("diagnostic should pause while holding the lifecycle gate");

        assert_eq!(streams.shared.picker_resolution(), None);
        assert_eq!(
            streams.shared.selector(),
            super::MacosCaptureSelector::PrimaryDisplay
        );
        assert!(streams.shared.capture_active());
        assert_eq!(
            streams.shared.selection(),
            super::MacosCaptureSelection::None
        );
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Starting);

        let (filter_started_tx, filter_started_rx) = mpsc::channel();
        let (filter_done_tx, filter_done_rx) = mpsc::channel();
        let filter_streams = Arc::clone(&streams);
        let stale_filter_resolution = stale_resolution.clone();
        let stale_filter = thread::spawn(move || {
            filter_started_tx
                .send(())
                .expect("old filter acceptance should be observable");
            let result = filter_streams.accept_selection_filter(
                NativeSelectionFilter::fixture(2),
                MacosStreamRequest::default(),
                42,
                stale_filter_resolution,
                true,
            );
            filter_done_tx
                .send(result)
                .expect("old filter result should be observable");
        });
        let (picker_started_tx, picker_started_rx) = mpsc::channel();
        let (picker_done_tx, picker_done_rx) = mpsc::channel();
        let picker_streams = Arc::clone(&streams);
        let new_picker = thread::spawn(move || {
            picker_started_tx
                .send(())
                .expect("new picker resolution should be observable");
            let resolution = picker_streams.begin_picker_resolution();
            picker_done_tx
                .send(resolution)
                .expect("new picker result should be observable");
        });
        filter_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("old filter should reach the lifecycle gate");
        picker_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("new picker should reach the lifecycle gate");
        assert!(matches!(
            filter_done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(
            picker_done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );

        release_tx
            .send(())
            .expect("diagnostic setup should release the lifecycle gate");
        let (diagnostic, completion) = setup
            .join()
            .expect("diagnostic setup thread should join")
            .expect("diagnostic setup should succeed");
        assert!(matches!(
            filter_done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("old filter should finish after diagnostic setup"),
            Ok(super::FilterAcceptance::Stale)
        ));
        let picker_resolution = picker_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("new picker should finish after diagnostic setup")
            .expect("new picker resolution should succeed");
        stale_filter.join().expect("old filter thread should join");
        new_picker.join().expect("new picker thread should join");

        assert!(!streams.shared.diagnostic_resolution_is_current(&diagnostic));
        assert_eq!(
            streams.shared.picker_resolution(),
            Some(picker_resolution.clone())
        );
        assert!(
            streams
                .shared
                .source_resolution_is_current(&picker_resolution)
        );
        assert_eq!(completion.recv(), Ok(MacosProtectedSourceState::Failed));
        assert_eq!(selection_filter_ids(&streams), (None, None));
    }

    #[test]
    fn inactive_filter_acceptance_precedes_crossing_activation_atomically() {
        let streams = stream_slot_fixture(41, 9);
        assert!(streams.set_capture_active(false));
        streams.next_epoch.store(43, Ordering::Release);
        let resolution = streams
            .begin_resolution()
            .expect("filter resolution should begin");
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let accepting_streams = Arc::clone(&streams);
        let accepting = thread::spawn(move || {
            accepting_streams.accept_selection_filter_with(
                NativeSelectionFilter::fixture(2),
                MacosStreamRequest::default(),
                42,
                resolution,
                false,
                || {
                    accepted_tx
                        .send(())
                        .expect("filter acceptance should be observable");
                    release_rx.recv().expect("filter acceptance should resume");
                },
            )
        });
        accepted_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("filter acceptance should hold the lifecycle gate");

        let (activation_tx, activation_rx) = mpsc::channel();
        let activation_streams = Arc::clone(&streams);
        let activation = thread::spawn(move || {
            activation_tx
                .send(activation_streams.begin_capture_activation())
                .expect("activation result should be observable");
        });
        assert!(matches!(
            activation_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        release_tx
            .send(())
            .expect("filter acceptance should be released");
        assert!(matches!(
            accepting.join().expect("acceptance thread should join"),
            Ok(super::FilterAcceptance::Stored(None))
        ));
        let activation_result = activation_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("activation should finish after filter acceptance")
            .expect("activation should reserve the accepted filter");
        activation.join().expect("activation thread should join");
        let super::CaptureActivation::Candidate { reservation, .. } = activation_result else {
            panic!("activation should stage the accepted filter");
        };
        let CandidateReservation {
            stage,
            selection_filter,
            replaced_settlement,
            ..
        } = *reservation;
        StreamSlot::finish_replaced_candidate(replaced_settlement);
        assert_eq!(selection_filter.fixture_id(), 2);
        assert!(streams.start_candidate_fixture(stage));
        assert!(streams.activate_candidate_fixture(stage.epoch));
        assert_eq!(selection_filter_ids(&streams), (Some(2), None));
    }

    #[test]
    fn active_filter_acceptance_precedes_crossing_deactivation_atomically() {
        let streams = stream_slot_fixture(41, 9);
        let resolution = streams
            .begin_resolution()
            .expect("filter resolution should begin");
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let accepting_streams = Arc::clone(&streams);
        let accepting = thread::spawn(move || {
            accepting_streams.accept_selection_filter_with(
                NativeSelectionFilter::fixture(2),
                MacosStreamRequest::default(),
                42,
                resolution,
                false,
                || {
                    accepted_tx
                        .send(())
                        .expect("filter acceptance should be observable");
                    release_rx.recv().expect("filter acceptance should resume");
                },
            )
        });
        accepted_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("filter acceptance should hold the lifecycle gate");

        let (deactivation_tx, deactivation_rx) = mpsc::channel();
        let deactivation_streams = Arc::clone(&streams);
        let deactivation = thread::spawn(move || {
            deactivation_tx
                .send(deactivation_streams.set_capture_active(false))
                .expect("deactivation result should be observable");
        });
        assert_eq!(
            deactivation_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );

        release_tx
            .send(())
            .expect("filter acceptance should be released");
        let acceptance = accepting
            .join()
            .expect("acceptance thread should join")
            .expect("active acceptance should reserve a candidate");
        assert!(
            deactivation_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("deactivation should finish after filter acceptance")
        );
        deactivation
            .join()
            .expect("deactivation thread should join");
        let super::FilterAcceptance::Candidate { reservation, .. } = acceptance else {
            panic!("active acceptance should stage the delivered filter");
        };
        let CandidateReservation {
            stage,
            replaced_settlement,
            ..
        } = *reservation;
        StreamSlot::finish_replaced_candidate(replaced_settlement);
        assert!(!streams.start_candidate_fixture(stage));
        assert_eq!(selection_filter_ids(&streams), (Some(2), None));
        assert!(!streams.shared.capture_active());
    }

    #[test]
    fn candidate_activation_requires_its_pending_selection_revision() {
        let streams = stream_slot_fixture(41, 9);
        let (stage, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 2)
                .expect("candidate reservation succeeds")
                .expect("active capture admits a candidate");
        assert!(streams.start_candidate_fixture(stage));
        super::lock(&streams.state).selection_revision += 1;

        assert!(!streams.activate_candidate_fixture(42));
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((42, 2))));
        assert_eq!(streams.shared.current_epoch(), 41);
    }

    #[test]
    fn interrupted_restage_transitions_once_from_interrupted_to_live() {
        let recovery = InterruptedRestage::interrupted(41, 9);
        assert_eq!(recovery.phase(), InterruptionRecoveryPhase::Interrupted);
        assert!(recovery.can_schedule(true, 0, 9));

        let recovery = recovery
            .schedule(42)
            .expect("the next session epoch should schedule one recovery restage");
        assert_eq!(
            recovery.phase(),
            InterruptionRecoveryPhase::Starting { epoch: 42 }
        );
        assert_eq!(
            recovery.complete(42),
            Some(InterruptionRecoveryPhase::Live { epoch: 42 })
        );
        assert_eq!(recovery.complete(43), None);
        assert_eq!(recovery.schedule(43), None);
    }

    #[test]
    fn interrupted_restage_cancels_when_capture_demand_reaches_zero() {
        let recovery = InterruptedRestage::interrupted(41, 9);

        assert!(!recovery.can_schedule(false, 0, 9));
    }

    #[test]
    fn interrupted_restage_rejects_newer_selection_and_session_epochs() {
        let recovery = InterruptedRestage::interrupted(41, 9);

        assert!(!recovery.can_schedule(true, 0, 10));
        assert!(!recovery.can_schedule(true, 42, 9));
    }

    #[test]
    fn stream_slot_start_fixture_discards_a_candidate_after_demand_stops() {
        let streams = stream_slot_fixture(41, 9);
        let (stage, replaced) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("production slot reserves a candidate")
                .expect("active demand admits a candidate");
        assert!(replaced.is_none());

        assert!(streams.set_capture_active(false));

        assert!(!streams.start_candidate_fixture(stage));
        assert_eq!(super::lock(&streams.state).candidate_epoch, None);
    }

    #[test]
    fn stream_slot_start_fixture_rejects_a_repick_before_the_old_start_runs() {
        let streams = stream_slot_fixture(41, 9);
        let (stale, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("first candidate reserves")
                .expect("first candidate stages");
        let (current, _) =
            reserve_selection_candidate_fixture(&streams, 43, MacosStreamRequest::default(), 43)
                .expect("replacement candidate reserves")
                .expect("replacement candidate stages");

        assert!(!streams.start_candidate_fixture(stale));
        assert!(streams.start_candidate_fixture(current));
        assert_eq!(super::lock(&streams.state).candidate_epoch, Some(43));
    }

    #[test]
    fn deactivation_returns_before_queued_native_start_finishes() {
        let streams = stream_slot_fixture(41, 9);
        let (stage, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("candidate reservation succeeds")
                .expect("active capture admits a candidate");
        let invoked = Arc::new(AtomicBool::new(false));
        let (installed_tx, installed_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let starter_streams = Arc::clone(&streams);
        let starter_invoked = Arc::clone(&invoked);
        let starter = thread::spawn(move || {
            starter_streams.start_candidate_fixture_with(stage, move || {
                installed_tx
                    .send(())
                    .expect("installed candidate should be observable");
                release_rx
                    .recv()
                    .expect("native start invocation should be released");
                starter_invoked.store(true, Ordering::Release);
            })
        });
        installed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("candidate should install before the injected invocation pauses");

        let (deactivate_started_tx, deactivate_started_rx) = mpsc::channel();
        let (deactivate_done_tx, deactivate_done_rx) = mpsc::channel();
        let deactivate_streams = Arc::clone(&streams);
        let deactivate_invoked = Arc::clone(&invoked);
        let deactivate = thread::spawn(move || {
            deactivate_started_tx
                .send(())
                .expect("deactivation attempt should be observable");
            let changed = deactivate_streams.set_capture_active(false);
            deactivate_done_tx
                .send((changed, deactivate_invoked.load(Ordering::Acquire)))
                .expect("deactivation result should be observable");
        });
        deactivate_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deactivation should reach the lifecycle gate");
        assert_eq!(
            deactivate_done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("deactivation does not wait for native start"),
            (true, false)
        );
        assert_eq!(super::lock(&streams.state).candidate_epoch, None);

        release_tx
            .send(())
            .expect("native start invocation should resume");
        assert!(starter.join().expect("starter thread should join"));
        deactivate.join().expect("deactivation thread should join");
        assert_eq!(super::lock(&streams.state).candidate_epoch, None);
    }

    #[test]
    fn repick_returns_before_the_superseded_native_start_finishes() {
        let streams = stream_slot_fixture(41, 9);
        let (stage, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("candidate reservation succeeds")
                .expect("active capture admits a candidate");
        let invoked = Arc::new(AtomicBool::new(false));
        let (installed_tx, installed_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let starter_streams = Arc::clone(&streams);
        let starter_invoked = Arc::clone(&invoked);
        let starter = thread::spawn(move || {
            starter_streams.start_candidate_fixture_with(stage, move || {
                installed_tx
                    .send(())
                    .expect("installed candidate should be observable");
                release_rx
                    .recv()
                    .expect("native start invocation should be released");
                starter_invoked.store(true, Ordering::Release);
            })
        });
        installed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("candidate should install before the injected invocation pauses");

        let (repick_started_tx, repick_started_rx) = mpsc::channel();
        let (repick_done_tx, repick_done_rx) = mpsc::channel();
        let repick_streams = Arc::clone(&streams);
        let repick_invoked = Arc::clone(&invoked);
        let repick = thread::spawn(move || {
            repick_started_tx
                .send(())
                .expect("repick attempt should be observable");
            let (replacement, retired) = reserve_selection_candidate_fixture(
                &repick_streams,
                43,
                MacosStreamRequest::default(),
                43,
            )
            .expect("repick reservation succeeds")
            .expect("active capture admits the repick");
            assert!(retired.is_none());
            repick_done_tx
                .send((replacement.epoch, repick_invoked.load(Ordering::Acquire)))
                .expect("repick result should be observable");
        });
        repick_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("repick should reach the lifecycle gate");
        assert_eq!(
            repick_done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("repick does not wait for native start"),
            (43, false)
        );
        assert_eq!(super::lock(&streams.state).staging_epoch, Some(43));

        release_tx
            .send(())
            .expect("native start invocation should resume");
        assert!(starter.join().expect("starter thread should join"));
        repick.join().expect("repick thread should join");
        assert_eq!(super::lock(&streams.state).staging_epoch, Some(43));
    }

    #[test]
    fn stale_async_start_failure_cannot_retire_the_successor_candidate() {
        let streams = stream_slot_fixture(41, 9);
        let (diagnostic, diagnostic_completion) = streams
            .shared
            .begin_restart_diagnostic(true, 9)
            .expect("diagnostic attempt begins");
        streams.shared.record_filter_enumerated(&diagnostic, 42);
        let (callback_blocked_tx, callback_blocked_rx) = mpsc::channel();
        let (release_callback_tx, release_callback_rx) = mpsc::channel();
        streams.lifecycle_callbacks.exec_async(move || {
            callback_blocked_tx
                .send(())
                .expect("blocked lifecycle callback should be observable");
            release_callback_rx
                .recv()
                .expect("lifecycle callback should be released");
        });
        callback_blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("lifecycle callback queue should pause");
        let (stale, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("stale candidate reservation succeeds")
                .expect("active capture admits the stale candidate");
        let failure_streams = Arc::clone(&streams);
        let failure_shared = Arc::clone(&streams.shared);
        assert!(streams.start_candidate_fixture_with(stale, move || {
            super::dispatch_owned_stream_error(
                failure_streams,
                42,
                failure_shared,
                MacosProtectedSourceState::PermissionDenied,
                MacosCaptureError::CaptureWorkerStartFailed(
                    "stale injected start failure".to_owned(),
                ),
            );
        }));
        assert!(streams.set_capture_active(false));
        assert!(streams.set_capture_active(true));

        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("successor request is valid");
        let (pending, completion) = pending_request(43, next);
        let (successor, _) = reserve_request_candidate_fixture(&streams, 43, next, pending)
            .expect("successor reservation succeeds")
            .expect("reactivated capture admits the successor");
        assert!(streams.start_candidate_fixture(successor));

        release_callback_tx
            .send(())
            .expect("stale start completion should resume");
        streams.drain_lifecycle_callbacks();
        super::dispatch_stream_start_success(&Arc::downgrade(&streams), 42);
        streams.drain_lifecycle_callbacks();

        assert_eq!(super::lock(&streams.state).candidate_epoch, Some(43));
        assert_eq!(streams.request(), next);
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(
            diagnostic_completion.try_recv(),
            Ok(MacosProtectedSourceState::Failed)
        );
        assert!(streams.activate_candidate_fixture(43));
        assert_eq!(completion.recv(), Ok(Ok(())));
        streams
            .shared
            .fail_restart_diagnostic_attempt(diagnostic.attempt);
        assert_eq!(
            diagnostic_completion.try_recv(),
            Ok(MacosProtectedSourceState::Failed)
        );
    }

    #[test]
    fn deactivation_retires_diagnostic_before_queued_candidate_completion() {
        let streams = stream_slot_fixture(41, 9);
        let (diagnostic, diagnostic_completion) = streams
            .shared
            .begin_restart_diagnostic(true, 9)
            .expect("diagnostic attempt should begin");
        streams.shared.record_filter_enumerated(&diagnostic, 42);

        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        streams.lifecycle_callbacks.exec_async(move || {
            blocked_tx
                .send(())
                .expect("queued completion pause should be observable");
            release_rx.recv().expect("queued completion should resume");
        });
        blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("lifecycle queue should pause before candidate completion");

        let (candidate, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("diagnostic candidate should reserve")
                .expect("active capture should admit the diagnostic candidate");
        let callback_streams = Arc::clone(&streams);
        let callback_shared = Arc::clone(&streams.shared);
        assert!(streams.start_candidate_fixture_with(candidate, move || {
            super::dispatch_owned_stream_error(
                callback_streams,
                42,
                callback_shared,
                MacosProtectedSourceState::PermissionDenied,
                MacosCaptureError::CaptureWorkerStartFailed(
                    "queued diagnostic candidate completion".to_owned(),
                ),
            );
        }));

        assert!(streams.set_capture_active(false));
        assert_eq!(
            diagnostic_completion
                .recv()
                .expect("deactivation should terminally complete the diagnostic"),
            MacosProtectedSourceState::Failed
        );
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Stopped));

        release_tx
            .send(())
            .expect("stale candidate completion should resume");
        streams.drain_lifecycle_callbacks();
        super::dispatch_stream_start_success(&Arc::downgrade(&streams), 42);
        streams.drain_lifecycle_callbacks();

        assert!(!streams.shared.capture_active());
        assert_eq!(streams.shared.current_epoch(), 0);
        assert_eq!(super::lock(&streams.state).candidate_epoch, None);
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Stopped)))
        ));
    }

    fn assert_failure_before_activation_rejects_the_candidate(
        dispatch_failure: impl FnOnce(&Arc<StreamSlot>, Arc<SessionShared>, MacosCaptureError),
    ) {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("candidate request is valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("candidate reservation succeeds")
            .expect("active capture admits the candidate");
        assert!(streams.start_candidate_fixture(stage));

        let (callback_blocked_tx, callback_blocked_rx) = mpsc::channel();
        let (release_callback_tx, release_callback_rx) = mpsc::channel();
        streams.lifecycle_callbacks.exec_async(move || {
            callback_blocked_tx
                .send(())
                .expect("blocked lifecycle callback should be observable");
            release_callback_rx
                .recv()
                .expect("lifecycle callback should be released");
        });
        callback_blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("lifecycle callback queue should pause");

        dispatch_failure(
            &streams,
            Arc::clone(&streams.shared),
            MacosCaptureError::CaptureWorkerStartFailed(
                "injected candidate failure before activation".to_owned(),
            ),
        );

        assert!(!streams.accepts_epoch(42));
        assert!(!streams.activate_candidate_fixture(42));
        assert_eq!(streams.committed_request(), original);
        assert_eq!(streams.shared.current_epoch(), 41);
        assert_eq!(super::lock(&streams.state).candidate_epoch, Some(42));
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));

        release_callback_tx
            .send(())
            .expect("queued teardown should resume");
        streams.drain_lifecycle_callbacks();

        assert_eq!(super::lock(&streams.state).candidate_epoch, None);
        assert_eq!(streams.committed_request(), original);
        assert_eq!(streams.shared.current_epoch(), 41);
        assert!(matches!(completion.recv(), Ok(Err(_))));
    }

    #[test]
    fn start_failure_before_activation_rejects_the_exact_candidate_synchronously() {
        assert_failure_before_activation_rejects_the_candidate(|streams, shared, error| {
            super::dispatch_owned_stream_error(
                Arc::clone(streams),
                42,
                shared,
                MacosProtectedSourceState::PermissionDenied,
                error,
            );
        });
    }

    #[test]
    fn fatal_failure_before_activation_rejects_the_exact_candidate_synchronously() {
        assert_failure_before_activation_rejects_the_candidate(|streams, shared, error| {
            super::handle_fatal_stream_error(&Arc::downgrade(streams), 42, shared, error);
        });
    }

    #[test]
    fn stream_slot_start_fixture_never_regresses_a_newer_live_session_to_interrupted() {
        let streams = stream_slot_fixture(0, 9);
        let recovery = InterruptedRestage::interrupted(41, 9);

        assert!(recovery.can_begin(&super::lock(&streams.state), &streams.shared));
        streams.shared.activate_epoch(43);

        assert!(!recovery.can_begin(&super::lock(&streams.state), &streams.shared));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
    }

    #[test]
    fn pending_selection_request_after_repick_avoids_the_current_filter() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("request is valid");
        let streams = stream_slot_fixture(41, 9);
        let (repick, _) = reserve_selection_candidate_fixture(&streams, 42, original, 2)
            .expect("repick reserves")
            .expect("active capture stages the repick");
        assert!(streams.start_candidate_fixture(repick));
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((42, 2))));
        assert_eq!(streams.shared.current_epoch(), 41);

        streams.next_epoch.store(43, Ordering::Release);
        let (transaction, replaced) = streams
            .begin_request_candidate_fixture(next)
            .expect("request restages the repick selection");
        assert!(replaced.is_none());
        assert_eq!(transaction.generation(), 43);
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((43, 2))));
        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(!streams.activate_candidate_fixture(42));
        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(streams.shared.current_epoch(), 41);

        assert!(streams.activate_candidate_fixture(43));
        assert_eq!(transaction.recv(), Ok(Ok(())));
        assert_eq!(selection_filter_ids(&streams), (Some(2), None));
        assert_eq!(streams.committed_request(), next);
        assert_eq!(streams.shared.current_epoch(), 43);
    }

    #[test]
    fn pending_selection_request_after_first_candidate_keeps_the_only_filter() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("request is valid");
        let streams = stream_slot_fixture(0, 3);
        super::lock(&streams.state).selected_filter = None;
        let (first, _) = reserve_selection_candidate_fixture(&streams, 42, original, 7)
            .expect("first selection reserves")
            .expect("active capture stages the first selection");
        assert!(streams.start_candidate_fixture(first));
        assert_eq!(selection_filter_ids(&streams), (None, Some((42, 7))));

        streams.next_epoch.store(43, Ordering::Release);
        let (transaction, replaced) = streams
            .begin_request_candidate_fixture(next)
            .expect("request restages the only selection");
        assert!(replaced.is_none());
        assert_eq!(selection_filter_ids(&streams), (None, Some((43, 7))));
        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(!streams.activate_candidate_fixture(42));

        assert!(streams.activate_candidate_fixture(43));
        assert_eq!(transaction.recv(), Ok(Ok(())));
        assert_eq!(selection_filter_ids(&streams), (Some(7), None));
        assert_eq!(streams.committed_request(), next);
        assert_eq!(streams.shared.current_epoch(), 43);
    }

    #[test]
    fn pending_selection_request_fences_async_preinstall_ordering() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("request is valid");
        let streams = stream_slot_fixture(41, 9);
        let (uninstalled, _) = reserve_selection_candidate_fixture(&streams, 42, original, 8)
            .expect("async selection reserves")
            .expect("active capture stages the async selection");
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((42, 8))));

        streams.next_epoch.store(43, Ordering::Release);
        let (transaction, replaced) = streams
            .begin_request_candidate_fixture(next)
            .expect("request supersedes the pre-install stage");
        assert!(replaced.is_none());
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((43, 8))));
        assert!(!streams.start_candidate_fixture(uninstalled));
        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));

        assert!(streams.activate_candidate_fixture(43));
        assert_eq!(transaction.recv(), Ok(Ok(())));
        assert_eq!(selection_filter_ids(&streams), (Some(8), None));
        assert_eq!(streams.committed_request(), next);
    }

    #[test]
    fn stream_slot_request_restage_commits_only_at_candidate_activation() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(9, next);

        let (stage, replaced) = reserve_request_candidate_fixture(&streams, 9, next, pending)
            .expect("request restage should reserve")
            .expect("active request should stage a candidate");
        assert!(replaced.is_none());
        assert_eq!(streams.request(), next);
        assert_eq!(super::lock(&streams.state).request, original);
        assert_eq!(
            completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );

        assert!(streams.start_candidate_fixture(stage));
        assert_eq!(
            completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );
        assert!(streams.activate_candidate_fixture(stage.epoch));
        assert_eq!(completion.recv(), Ok(Ok(())));

        let state = super::lock(&streams.state);
        assert_eq!(state.request, next);
        assert!(state.pending_request.is_none());
        assert_eq!(state.candidate_epoch, None);
    }

    #[test]
    fn picker_replacement_retargets_the_pending_request_transaction() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("pending request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (request_stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request candidate reserves")
            .expect("active capture stages the request candidate");
        assert!(streams.start_candidate_fixture(request_stage));

        let (picker_stage, replaced) = reserve_selection_candidate_fixture(&streams, 43, next, 43)
            .expect("picker replacement reserves with the authoritative request")
            .expect("active capture stages the picker replacement");
        assert!(replaced.is_none());
        assert_eq!(picker_stage.request.map(|request| request.epoch), Some(43));
        assert_eq!(streams.request(), next);
        assert_eq!(streams.committed_request(), original);
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));

        assert!(!streams.fail_candidate_fixture(
            42,
            MacosCaptureError::CaptureWorkerStartFailed(
                "stale replaced candidate failed".to_owned(),
            )
        ));
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(streams.start_candidate_fixture(picker_stage));
        assert!(streams.activate_candidate_fixture(43));
        assert_eq!(completion.recv(), Ok(Ok(())));
        assert_eq!(streams.committed_request(), next);
    }

    #[test]
    fn stale_resolution_snapshot_cannot_displace_the_pending_request_transaction() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("pending request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (request_stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request candidate reserves")
            .expect("active capture stages the request candidate");
        assert!(streams.start_candidate_fixture(request_stage));
        let selection_revision = streams.selection_revision();

        let error = match reserve_selection_candidate_fixture(&streams, 43, original, 43) {
            Ok(_) => panic!("stale resolution snapshot must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("authoritative stream request"));
        assert_eq!(streams.selection_revision(), selection_revision);
        assert_eq!(super::lock(&streams.state).candidate_epoch, Some(42));
        assert_eq!(streams.request(), next);
        assert_eq!(streams.committed_request(), original);
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));

        let (retry, _) = reserve_selection_candidate_fixture(&streams, 44, next, 44)
            .expect("resolution retries with the authoritative pending request")
            .expect("retry stages a replacement candidate");
        assert!(!streams.fail_candidate_fixture(
            42,
            MacosCaptureError::CaptureWorkerStartFailed(
                "stale request candidate failed".to_owned(),
            )
        ));
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(streams.start_candidate_fixture(retry));
        assert!(streams.activate_candidate_fixture(44));
        assert_eq!(completion.recv(), Ok(Ok(())));
        assert_eq!(streams.committed_request(), next);
    }

    #[test]
    fn stale_resolution_snapshot_after_request_commit_cannot_replace_the_committed_request() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("pending request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (request_stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request candidate reserves")
            .expect("active capture stages the request candidate");
        assert!(streams.start_candidate_fixture(request_stage));
        assert!(streams.activate_candidate_fixture(42));
        assert_eq!(completion.recv(), Ok(Ok(())));

        let selection_revision = streams.selection_revision();
        let current_epoch = streams.shared.current_epoch();
        let error = match reserve_selection_candidate_fixture(&streams, 43, original, 43) {
            Ok(_) => panic!("post-commit stale resolution snapshot must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("authoritative stream request"));
        assert_eq!(streams.selection_revision(), selection_revision);
        assert_eq!(streams.shared.current_epoch(), current_epoch);
        {
            let state = super::lock(&streams.state);
            assert_eq!(state.request, next);
            assert!(state.pending_request.is_none());
            assert_eq!(state.staging_epoch, None);
            assert_eq!(state.candidate_epoch, None);
        }

        let (retry, replaced) = reserve_selection_candidate_fixture(&streams, 44, next, 44)
            .expect("resolution retries with the committed request")
            .expect("retry stages a replacement candidate");
        assert!(replaced.is_none());
        assert!(streams.start_candidate_fixture(retry));
        assert!(streams.activate_candidate_fixture(44));
        assert_eq!(streams.committed_request(), next);
        assert_eq!(streams.shared.current_epoch(), 44);
    }

    #[test]
    fn stale_resolution_snapshot_after_request_rollback_cannot_replace_the_committed_request() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("pending request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (request_stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request candidate reserves")
            .expect("active capture stages the request candidate");
        assert!(streams.start_candidate_fixture(request_stage));
        let failure =
            MacosCaptureError::CaptureWorkerStartFailed("fixture request failure".to_owned());
        assert!(streams.fail_candidate_fixture(42, failure.clone()));
        assert_eq!(completion.recv(), Ok(Err(failure)));

        let selection_revision = streams.selection_revision();
        let current_epoch = streams.shared.current_epoch();
        let error = match reserve_selection_candidate_fixture(&streams, 43, next, 43) {
            Ok(_) => panic!("post-rollback stale resolution snapshot must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("authoritative stream request"));
        assert_eq!(streams.selection_revision(), selection_revision);
        assert_eq!(streams.shared.current_epoch(), current_epoch);
        {
            let state = super::lock(&streams.state);
            assert_eq!(state.request, original);
            assert!(state.pending_request.is_none());
            assert_eq!(state.staging_epoch, None);
            assert_eq!(state.candidate_epoch, None);
        }

        let (retry, replaced) = reserve_selection_candidate_fixture(&streams, 44, original, 44)
            .expect("resolution retries with the rolled-back committed request")
            .expect("retry stages a replacement candidate");
        assert!(replaced.is_none());
        assert!(streams.start_candidate_fixture(retry));
        assert!(streams.activate_candidate_fixture(44));
        assert_eq!(streams.committed_request(), original);
        assert_eq!(streams.shared.current_epoch(), 44);
    }

    #[test]
    fn stream_slot_request_restage_failure_rolls_back_pending_request() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new_hdr(MacosCaptureCadence::NativeRefresh, true)
            .expect("fixture HDR request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(12, next);

        let (stage, replaced) = reserve_request_candidate_fixture(&streams, 12, next, pending)
            .expect("request restage should reserve")
            .expect("active request should stage a candidate");
        assert!(replaced.is_none());
        assert!(streams.start_candidate_fixture(stage));
        let error = MacosCaptureError::CaptureWorkerStartFailed("fixture async failure".to_owned());
        assert!(streams.fail_candidate_fixture(stage.epoch, error.clone()));
        assert_eq!(completion.recv(), Ok(Err(error)));

        let state = super::lock(&streams.state);
        assert_eq!(state.request, original);
        assert!(state.pending_request.is_none());
        assert_eq!(state.staging_epoch, None);
        assert_eq!(state.candidate_epoch, None);
    }

    #[test]
    fn missing_start_completion_times_out_without_retiring_the_current_stream() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        streams.next_epoch.store(12, Ordering::Release);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(next)
            .expect("candidate transaction starts");
        let deadline = transaction
            .current_deadline()
            .expect("start transaction has a deadline");

        streams
            .native_lifecycle
            .deadlines()
            .expire_through(deadline);

        assert_eq!(
            transaction.wait(),
            Err(MacosNativeTransactionError::TimedOut {
                phase: MacosNativeTransactionPhase::StreamStart,
                generation: 12,
            })
        );
        let state = super::lock(&streams.state);
        assert_eq!(StreamSlot::current_epoch(&state), Some(7));
        assert_eq!(state.candidate_epoch, None);
        assert_eq!(state.request, original);
    }

    #[test]
    fn missing_source_callback_times_out_and_fences_the_exact_resolution() {
        let streams = stream_slot_fixture(7, 3);
        let resolution = streams
            .begin_resolution()
            .expect("general source resolution starts");
        let completion = super::lock(&streams.source_transaction)
            .as_ref()
            .expect("source transaction is installed")
            .completion
            .clone();
        let deadline = completion
            .current_deadline()
            .expect("general source resolution is bounded");

        streams
            .native_lifecycle
            .deadlines()
            .expire_through(deadline);

        assert!(!completion.is_open());
        assert!(!streams.shared.source_resolution_is_current(&resolution));
        assert!(super::lock(&streams.source_transaction).is_none());
        assert_eq!(streams.shared.current_epoch(), 7);
    }

    #[test]
    fn picker_selection_has_cancellation_without_a_wall_clock_deadline() {
        let streams = stream_slot_fixture(7, 3);
        let resolution = streams
            .begin_picker_resolution()
            .expect("picker resolution starts");
        let completion = super::lock(&streams.source_transaction)
            .as_ref()
            .expect("picker transaction is installed")
            .completion
            .clone();

        assert_eq!(completion.current_deadline(), None);
        assert!(completion.is_open());
        let settlement = streams.cancel_source_transaction(&resolution);
        settlement
            .expect("picker cancellation claims the source transaction")
            .publish();

        assert!(!completion.is_open());
        assert!(super::lock(&streams.source_transaction).is_none());
    }

    #[test]
    fn source_success_remains_unpublished_until_resolution_commit() {
        let streams = stream_slot_fixture(7, 3);
        let resolution = streams
            .begin_picker_resolution()
            .expect("picker resolution starts");
        let completion = super::lock(&streams.source_transaction)
            .as_ref()
            .expect("source transaction is installed")
            .completion
            .clone();

        let settlement = streams
            .claim_source_transaction(&resolution)
            .expect("source callback claims success");
        assert_eq!(completion.outcome(), None);
        assert!(super::lock(&streams.source_transaction).is_none());

        streams
            .shared
            .set_status(MacosProtectedSourceState::ReadyIdle);
        assert_eq!(completion.outcome(), None);
        settlement.publish();

        assert_eq!(completion.outcome(), Some(Ok(())));
        assert_eq!(
            streams.shared.status(),
            MacosProtectedSourceState::ReadyIdle
        );
    }

    #[test]
    fn missing_first_complete_frame_uses_the_original_deadline_and_times_out() {
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        streams.next_epoch.store(12, Ordering::Release);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(next)
            .expect("candidate transaction starts");

        super::dispatch_stream_start_success(&Arc::downgrade(&streams), 12);
        streams.drain_lifecycle_callbacks();
        let deadline = transaction
            .current_deadline()
            .expect("first frame transaction has a deadline");
        streams
            .native_lifecycle
            .deadlines()
            .expire_through(deadline);

        assert_eq!(
            transaction.wait(),
            Err(MacosNativeTransactionError::TimedOut {
                phase: MacosNativeTransactionPhase::FirstCompleteFrame,
                generation: 12,
            })
        );
        assert_eq!(streams.shared.current_epoch(), 7);
        assert_eq!(super::lock(&streams.state).candidate_epoch, None);
    }

    #[test]
    fn observed_start_callback_preserves_the_absolute_deadline_before_queue_delivery() {
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        streams.next_epoch.store(12, Ordering::Release);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(next)
            .expect("candidate transaction starts");
        let stale_start_deadline = transaction
            .current_deadline()
            .expect("start transaction has a deadline");
        let (blocked_tx, blocked_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        streams.lifecycle_callbacks.exec_async(move || {
            blocked_tx
                .send(())
                .expect("lifecycle queue block is observable");
            release_rx
                .recv()
                .expect("lifecycle queue block is released");
        });
        blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("lifecycle queue is blocked");

        super::dispatch_stream_start_success(&Arc::downgrade(&streams), 12);
        streams
            .native_lifecycle
            .deadlines()
            .expire_through(stale_start_deadline);

        release_tx.send(()).expect("lifecycle queue should resume");
        streams.drain_lifecycle_callbacks();
        assert_eq!(
            transaction.wait(),
            Err(MacosNativeTransactionError::TimedOut {
                phase: MacosNativeTransactionPhase::FirstCompleteFrame,
                generation: 12,
            })
        );
        assert!(!streams.activate_candidate_fixture(12));
    }

    #[test]
    fn first_frame_and_timeout_commit_exactly_one_candidate_result() {
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let winner = stream_slot_fixture(7, 3);
        winner.next_epoch.store(12, Ordering::Release);
        let (committed, _) = winner
            .begin_request_candidate_fixture(next)
            .expect("winning candidate starts");
        let stale_deadline = committed
            .current_deadline()
            .expect("winning candidate has a deadline");
        assert!(winner.activate_candidate_fixture(12));
        winner
            .native_lifecycle
            .deadlines()
            .expire_through(stale_deadline);
        assert_eq!(committed.wait(), Ok(()));
        assert_eq!(winner.shared.current_epoch(), 12);

        let timed_out = stream_slot_fixture(7, 3);
        timed_out.next_epoch.store(12, Ordering::Release);
        let (rejected, _) = timed_out
            .begin_request_candidate_fixture(next)
            .expect("losing candidate starts");
        let deadline = rejected
            .current_deadline()
            .expect("losing candidate has a deadline");
        timed_out
            .native_lifecycle
            .deadlines()
            .expire_through(deadline);
        assert!(!timed_out.activate_candidate_fixture(12));
        assert!(matches!(
            rejected.wait(),
            Err(MacosNativeTransactionError::TimedOut { .. })
        ));
        assert_eq!(timed_out.shared.current_epoch(), 7);
    }

    #[test]
    fn claimed_cancellation_retires_only_the_candidate_before_publishing() {
        let request = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(request)
            .expect("request candidate starts");
        let epoch = transaction.generation();
        let completion = super::lock(&streams.state)
            .candidate_completion
            .as_ref()
            .cloned()
            .expect("candidate completion is installed");
        let cancel_selected = Arc::new(std::sync::Barrier::new(2));
        let selected = Arc::clone(&cancel_selected);
        let resume_cancel = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::clone(&resume_cancel);
        let cancel_streams = Arc::clone(&streams);
        completion.set_cancel(move |generation| {
            selected.wait();
            resume.wait();
            cancel_streams.cancel_candidate_transaction(generation);
        });
        let cancel = thread::spawn(move || transaction.cancel());
        cancel_selected.wait();

        assert_eq!(completion.current_deadline(), None);
        assert!(!completion.has_deadline_ticket());
        assert_eq!(completion.outcome(), None);
        assert!(!streams.activate_candidate_fixture(epoch));
        assert_eq!(streams.shared.current_epoch(), 7);

        resume_cancel.wait();
        assert!(cancel.join().expect("cancellation attempt exits"));
        assert!(matches!(
            completion.outcome(),
            Some(Err(MacosNativeTransactionError::Cancelled { .. }))
        ));

        let state = super::lock(&streams.state);
        assert_eq!(state.fixture_current_epoch, Some(7));
        assert_eq!(state.fixture_candidate_epoch, None);
        assert_eq!(state.candidate_epoch, None);
        assert!(state.candidate_completion.is_none());
        assert!(state.pending_request.is_none());
        assert_eq!(state.request, MacosStreamRequest::default());
        drop(state);
        assert_eq!(streams.shared.current_epoch(), 7);
    }

    #[test]
    fn successful_claim_wakes_only_after_current_and_first_publication_commit() {
        let request = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(request)
            .expect("request candidate starts");
        let epoch = transaction.generation();
        let completion = super::lock(&streams.state)
            .candidate_completion
            .as_ref()
            .cloned()
            .expect("candidate completion is installed");
        let published = Arc::new(AtomicBool::new(false));
        let observed_publication = Arc::clone(&published);
        let observer_streams = Arc::clone(&streams);
        let (observed_tx, observed_rx) = mpsc::sync_channel(1);
        let waiter = thread::spawn(move || {
            let result = transaction.wait();
            let state = super::lock(&observer_streams.state);
            observed_tx
                .send((
                    result,
                    observer_streams.shared.current_epoch(),
                    state.fixture_current_epoch,
                    state.pending_selection.is_none(),
                    state.request,
                    observed_publication.load(Ordering::Acquire),
                ))
                .expect("waiter observation is delivered");
        });
        let claim_reached = Arc::new(std::sync::Barrier::new(2));
        let claimed = Arc::clone(&claim_reached);
        let resume_commit = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::clone(&resume_commit);
        let publish_streams = Arc::clone(&streams);
        let publish_flag = Arc::clone(&published);
        let publisher = thread::spawn(move || {
            publish_streams.publish_decoded_event_with_claim_hook(
                epoch,
                sdr_delivery_fixture(),
                move || {
                    claimed.wait();
                    resume.wait();
                },
                move || publish_flag.store(true, Ordering::Release),
            )
        });
        claim_reached.wait();

        assert_eq!(completion.current_deadline(), None);
        assert!(!completion.has_deadline_ticket());
        assert_eq!(completion.outcome(), None);
        assert_eq!(observed_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(!completion.cancel());

        resume_commit.wait();
        assert!(publisher.join().expect("first publication exits"));
        let observation = observed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("successful transaction wakes after publication");
        waiter.join().expect("request waiter exits");

        assert_eq!(observation.0, Ok(()));
        assert_eq!(observation.1, epoch);
        assert_eq!(observation.2, Some(epoch));
        assert!(observation.3);
        assert_eq!(observation.4, request);
        assert!(observation.5);
        assert!(!completion.is_open());
    }

    #[test]
    fn panic_after_success_claim_cleans_candidate_before_failure_publication() {
        let request = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(request)
            .expect("request candidate starts");
        let epoch = transaction.generation();
        let completion = super::lock(&streams.state)
            .candidate_completion
            .as_ref()
            .cloned()
            .expect("candidate completion is installed");
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            streams.publish_decoded_event_with_claim_hook(
                epoch,
                sdr_delivery_fixture(),
                || panic!("abort after reserving transaction success"),
                || panic!("publication must not run after claim abort"),
            )
        }));

        assert!(unwind.is_err());
        assert!(matches!(
            transaction.wait(),
            Err(MacosNativeTransactionError::Cancelled { .. })
        ));
        let state = super::lock(&streams.state);
        assert_eq!(state.fixture_current_epoch, Some(7));
        assert_eq!(state.fixture_candidate_epoch, None);
        assert_eq!(state.candidate_epoch, None);
        assert!(state.candidate_completion.is_none());
        assert!(state.pending_request.is_none());
        assert_eq!(state.request, MacosStreamRequest::default());
        drop(state);
        assert_eq!(streams.shared.current_epoch(), 7);
        assert!(!completion.has_deadline_ticket());
    }

    #[test]
    fn panic_before_first_publication_restores_prior_current_before_failure_wakes() {
        let request = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(request)
            .expect("request candidate starts");
        let epoch = transaction.generation();
        let completion = super::lock(&streams.state)
            .candidate_completion
            .as_ref()
            .cloned()
            .expect("candidate completion is installed");
        let previous_status = streams.shared.status();
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            streams.publish_decoded_event_with(epoch, true, Some(sdr_delivery_fixture()), || {
                panic!("abort before first publication commits")
            })
        }));

        assert!(unwind.is_err());
        assert!(matches!(
            transaction.wait(),
            Err(MacosNativeTransactionError::Cancelled { .. })
        ));
        let state = super::lock(&streams.state);
        assert_eq!(state.fixture_current_epoch, Some(7));
        assert_eq!(state.fixture_candidate_epoch, None);
        assert_eq!(state.candidate_epoch, None);
        assert_eq!(state.request, MacosStreamRequest::default());
        assert!(state.pending_selection.is_none());
        assert!(state.pending_request.is_none());
        drop(state);
        assert_eq!(streams.shared.current_epoch(), 7);
        assert_eq!(streams.shared.status(), previous_status);
        assert!(!completion.has_deadline_ticket());
    }

    #[test]
    fn stream_slot_serializes_request_transactions_while_a_candidate_is_pending() {
        let original = MacosStreamRequest::default();
        let first = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("first fixture request is valid");
        let second = MacosStreamRequest::new_hdr(MacosCaptureCadence::NativeRefresh, true)
            .expect("second fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(12, first);
        let (stage, _) = reserve_request_candidate_fixture(&streams, 12, first, pending)
            .expect("first request reserves")
            .expect("first request stages");
        assert!(streams.start_candidate_fixture(stage));
        let reserve_pool: super::PoolReservationFactory =
            Arc::new(|_, _| -> Result<PoolObservation, MacosCaptureError> {
                unreachable!("serialized request never prepares another native stream")
            });

        let error = match streams.set_request(second, &reserve_pool) {
            Ok(_) => panic!("a second request cannot overtake the pending transaction"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("still pending"));
        assert_eq!(
            completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );
        assert!(streams.activate_candidate_fixture(stage.epoch));
        assert_eq!(completion.recv(), Ok(Ok(())));
        assert_eq!(streams.committed_request(), first);
    }

    #[test]
    fn repeated_activate_deactivate_cancels_every_pending_transaction() {
        let streams = stream_slot_fixture(7, 3);
        let requests = [
            MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
                .expect("first fixture request is valid"),
            MacosStreamRequest::new_hdr(MacosCaptureCadence::NativeRefresh, true)
                .expect("second fixture request is valid"),
        ];

        for request in requests {
            streams
                .begin_picker_resolution()
                .expect("picker resolution begins");
            let (transaction, _) = streams
                .begin_request_candidate_fixture(request)
                .expect("request candidate starts");
            assert!(transaction.current_deadline().is_some());

            assert!(streams.set_capture_active(false));
            assert!(transaction.current_deadline().is_none());
            assert!(matches!(
                transaction.wait(),
                Err(MacosNativeTransactionError::Cancelled { .. })
            ));
            assert!(super::lock(&streams.source_transaction).is_none());
            let state = super::lock(&streams.state);
            assert!(state.pending_request.is_none());
            assert!(state.candidate_completion.is_none());
            assert_eq!(state.candidate_epoch, None);
            assert_eq!(state.staging_epoch, None);
            drop(state);
            assert_eq!(streams.native_lifecycle.deadlines().pending(), 0);
            assert_eq!(streams.native_lifecycle.pending_retirements(), 0);

            assert!(!streams.set_capture_active(false));
            assert!(streams.set_capture_active(true));
        }
    }

    #[test]
    fn timed_out_old_stop_error_cannot_degrade_a_live_successor() {
        let shared = SessionShared::new(
            MacosProtectedSourceState::Live,
            super::MacosCaptureSelector::Auto,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        );
        shared.activate_epoch(41);
        shared.record_retirement_error(&MacosCaptureError::StreamStopCompletionLost);

        shared.activate_epoch(42);
        shared.set_status(MacosProtectedSourceState::Live);
        shared.record_retirement_error(&MacosCaptureError::CaptureWorkerStartFailed(
            "late stop callback failed".to_owned(),
        ));

        assert_eq!(shared.current_epoch(), 42);
        assert_eq!(shared.status(), MacosProtectedSourceState::Live);
        assert!(!shared.mailbox.has_pending());
        assert_eq!(shared.diagnostics().total_dropped(), 2);
    }

    #[test]
    fn restart_diagnostic_requires_grant_enumeration_and_stream_permission_failure() {
        let shared = SessionShared::new(
            MacosProtectedSourceState::Starting,
            super::MacosCaptureSelector::PrimaryDisplay,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        );
        let (resolution, completion) = shared
            .begin_restart_diagnostic(true, 7)
            .expect("diagnostic attempt begins");
        shared.record_filter_enumerated(&resolution, 42);

        assert_eq!(
            shared.record_stream_diagnostic_result(42, MacosProtectedSourceState::PermissionDenied),
            MacosProtectedSourceState::NeedsProcessRestart
        );
        assert_eq!(
            completion.recv(),
            Ok(MacosProtectedSourceState::NeedsProcessRestart)
        );
    }

    #[test]
    fn restart_diagnostic_requires_its_exact_resolution_provenance() {
        let shared = SessionShared::new(
            MacosProtectedSourceState::Starting,
            super::MacosCaptureSelector::PrimaryDisplay,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        );
        let (stale, stale_completion) = shared
            .begin_restart_diagnostic(true, 7)
            .expect("first diagnostic begins");
        let (fresh, fresh_completion) = shared
            .begin_restart_diagnostic(true, 8)
            .expect("second diagnostic supersedes it");
        assert_eq!(
            stale_completion.recv(),
            Ok(MacosProtectedSourceState::Failed)
        );

        shared.record_non_stream_diagnostic_failure(&stale, MacosProtectedSourceState::Failed);
        assert_eq!(
            fresh_completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );
        shared.record_filter_enumerated(&fresh, 43);
        assert_eq!(
            shared.record_stream_diagnostic_result(43, MacosProtectedSourceState::PermissionDenied),
            MacosProtectedSourceState::NeedsProcessRestart
        );
        assert_eq!(
            fresh_completion.recv(),
            Ok(MacosProtectedSourceState::NeedsProcessRestart)
        );
    }

    #[test]
    fn claimed_diagnostic_cancellation_cannot_be_overwritten_by_stream_success() {
        let streams = stream_slot_fixture(0, 7);
        let (resolution, transaction) = streams
            .begin_restart_diagnostic(true, 7)
            .expect("diagnostic transaction begins");
        streams.shared.record_filter_enumerated(&resolution, 42);
        let completion = streams
            .shared
            .restart_diagnostic_completion(resolution.attempt)
            .expect("diagnostic completion remains active");
        let cancel_selected = Arc::new(std::sync::Barrier::new(2));
        let selected = Arc::clone(&cancel_selected);
        let resume_cancel = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::clone(&resume_cancel);
        let cancel_streams = Arc::clone(&streams);
        let attempt = resolution.attempt;
        completion.set_cancel(move |_| {
            selected.wait();
            resume.wait();
            cancel_streams.finish_restart_diagnostic(attempt);
        });
        let cancellation = thread::spawn(move || transaction.cancel());
        cancel_selected.wait();

        assert_eq!(completion.current_deadline(), None);
        assert_eq!(completion.outcome(), None);
        assert_eq!(
            streams
                .shared
                .record_stream_diagnostic_result(42, MacosProtectedSourceState::PermissionDenied,),
            MacosProtectedSourceState::PermissionDenied
        );
        assert!(
            streams
                .shared
                .restart_diagnostic_completion(resolution.attempt)
                .is_some()
        );

        resume_cancel.wait();
        assert!(cancellation.join().expect("diagnostic cancellation exits"));
        assert!(matches!(
            completion.outcome(),
            Some(Err(MacosNativeTransactionError::Cancelled { .. }))
        ));
        assert!(
            streams
                .shared
                .restart_diagnostic_completion(resolution.attempt)
                .is_none()
        );
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Failed);
        assert!(!completion.has_deadline_ticket());
    }

    #[test]
    fn ordinary_resolution_supersedes_the_diagnostic_without_stranding_its_receiver() {
        let shared = SessionShared::new(
            MacosProtectedSourceState::Starting,
            super::MacosCaptureSelector::PrimaryDisplay,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        );
        let (diagnostic, completion) = shared
            .begin_restart_diagnostic(true, 7)
            .expect("diagnostic begins");

        let ordinary = shared
            .begin_resolution()
            .expect("ordinary resolution begins");

        assert!(shared.resolution_is_current(ordinary));
        assert_eq!(completion.recv(), Ok(MacosProtectedSourceState::Failed));
        shared.record_filter_enumerated(&diagnostic, 42);
        assert_eq!(
            shared.record_stream_diagnostic_result(42, MacosProtectedSourceState::PermissionDenied),
            MacosProtectedSourceState::PermissionDenied
        );
    }

    #[test]
    fn primary_display_diagnostic_clears_picker_identity_before_enumeration() {
        let shared = Arc::new(SessionShared::new(
            MacosProtectedSourceState::ReadyIdle,
            super::MacosCaptureSelector::SessionScoped,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        ));
        shared.set_unconfirmed_selection(super::MacosCaptureSelection::SessionScoped {
            content_style: super::MacosCaptureContentStyle::Window,
        });
        let streams = StreamSlot::new(Arc::clone(&shared), MacosStreamRequest::default())
            .expect("fixture native lifecycle starts");
        {
            let mut state = super::lock(&streams.state);
            state.staging_epoch = Some(8);
            state.pending_request = Some(pending_request(8, MacosStreamRequest::default()).0);
        }

        streams
            .clear_selection()
            .expect("diagnostic reset clears prior selection state");
        shared.set_selector(super::MacosCaptureSelector::PrimaryDisplay);

        let state = super::lock(&streams.state);
        assert!(state.selected_filter.is_none());
        assert_eq!(state.staging_epoch, None);
        assert!(state.pending_request.is_none());
        assert_eq!(shared.selection(), super::MacosCaptureSelection::None);
        assert_eq!(
            shared.selector(),
            super::MacosCaptureSelector::PrimaryDisplay
        );
    }

    #[test]
    fn old_stream_completion_cannot_satisfy_primary_display_diagnostic() {
        let shared = SessionShared::new(
            MacosProtectedSourceState::Starting,
            super::MacosCaptureSelector::PrimaryDisplay,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        );
        let (resolution, completion) = shared
            .begin_restart_diagnostic(true, 7)
            .expect("diagnostic begins");
        shared.record_filter_enumerated(&resolution, 42);

        assert_eq!(
            shared.record_stream_diagnostic_result(41, MacosProtectedSourceState::ReadyIdle),
            MacosProtectedSourceState::ReadyIdle
        );
        assert_eq!(
            completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );
        assert_eq!(
            shared.record_stream_diagnostic_result(42, MacosProtectedSourceState::PermissionDenied),
            MacosProtectedSourceState::NeedsProcessRestart
        );
        assert_eq!(
            completion.recv(),
            Ok(MacosProtectedSourceState::NeedsProcessRestart)
        );
    }

    #[test]
    fn missing_arm64_and_translation_sysctls_resolve_native_intel_sdr() {
        let capabilities = capture_capabilities_from_probes(
            Ok(SysctlI32Value::Missing),
            Ok(SysctlI32Value::Missing),
            ABSENT_TAHOE_PROBES,
        )
        .expect("missing Apple Silicon sysctls identify a native Intel host");

        assert_eq!(capabilities.host_architecture, MacosHostArchitecture::Intel);
        assert!(!capabilities.translated_process);
        assert_eq!(
            capabilities.validate_dynamic_range(MacosCaptureDynamicRange::Sdr),
            Ok(())
        );
        assert_eq!(
            capabilities.validate_dynamic_range(MacosCaptureDynamicRange::Hdr),
            Err(MacosStreamDeliveryRejection::UnsupportedIntelHdr)
        );
    }

    #[test]
    fn translated_process_resolves_the_native_apple_silicon_host() {
        let capabilities = capture_capabilities_from_probes(
            Ok(SysctlI32Value::Missing),
            Ok(SysctlI32Value::Present(1)),
            ABSENT_TAHOE_PROBES,
        )
        .expect("translation is direct evidence of an Apple Silicon host");

        assert_eq!(
            capabilities.host_architecture,
            MacosHostArchitecture::AppleSilicon
        );
        assert!(capabilities.translated_process);
        assert_eq!(
            capabilities.validate_dynamic_range(MacosCaptureDynamicRange::Hdr),
            Ok(())
        );
    }

    #[test]
    fn nonmissing_sysctl_failures_remain_typed() {
        assert_eq!(
            capture_capabilities_from_probes(
                Err(MacosCaptureError::CapabilityProbeFailed(
                    "hw.optional.arm64"
                )),
                Ok(SysctlI32Value::Missing),
                ABSENT_TAHOE_PROBES,
            ),
            Err(MacosCaptureError::CapabilityProbeFailed(
                "hw.optional.arm64"
            ))
        );
    }

    #[test]
    fn partial_tahoe_runtime_surfaces_fail_closed_per_capability() {
        let screenshot_only = MacosTahoeRuntimeProbes {
            screenshot_configuration_class: MacosRuntimeCapability::Present,
            screenshot_dynamic_range_selector: MacosRuntimeCapability::Present,
            screenshot_capture_selector: MacosRuntimeCapability::Present,
            ..ABSENT_TAHOE_PROBES
        };
        let capabilities = capture_capabilities_from_probes(
            Ok(SysctlI32Value::Present(1)),
            Ok(SysctlI32Value::Missing),
            screenshot_only,
        )
        .expect("independent Tahoe capability probes should not disable capture");

        assert_eq!(
            capabilities.tahoe.content_tone_mapping_info,
            MacosRuntimeCapability::Absent
        );
        assert_eq!(
            capabilities.tahoe.screenshot_api,
            MacosRuntimeCapability::Present
        );

        let incomplete_screenshot = MacosTahoeRuntimeProbes {
            screenshot_configuration_class: MacosRuntimeCapability::Present,
            ..ABSENT_TAHOE_PROBES
        };
        let capabilities = capture_capabilities_from_probes(
            Ok(SysctlI32Value::Present(1)),
            Ok(SysctlI32Value::Missing),
            incomplete_screenshot,
        )
        .expect("an incomplete diagnostic surface should not disable streaming");
        assert_eq!(
            capabilities.tahoe.screenshot_api,
            MacosRuntimeCapability::Absent
        );
    }

    #[test]
    fn malformed_delivery_metadata_is_fatal_only_before_confirmation() {
        let configured = MacosConfiguredStream {
            requested_dynamic_range: MacosCaptureDynamicRange::Sdr,
            requested_preset: MacosStreamPreset::SdrDefault,
            configured_dynamic_range: MacosCaptureDynamicRange::Sdr,
            configured_pixel_format: MacosCapturePixelFormat::Bgra8,
            configured_color_range: MacosColorRange::Full,
        };
        let rejection =
            MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata("dynamic_range");
        let mut awaiting = MacosStreamDeliveryValidator::new(configured);
        assert_eq!(
            classify_delivery_error(
                &mut awaiting,
                MacosCaptureError::StreamDeliveryRejected(rejection),
            ),
            MacosCaptureError::StreamDeliveryRejected(rejection)
        );
        assert_eq!(
            awaiting.state(),
            &MacosStreamDeliveryState::Rejected(rejection)
        );

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
        .expect("valid SDR delivery");
        let mut confirmed = MacosStreamDeliveryValidator::new(configured);
        confirmed
            .observe_first_complete(Some(delivered))
            .expect("matching delivery should confirm the stream");

        assert_eq!(
            classify_delivery_error(
                &mut confirmed,
                MacosCaptureError::StreamDeliveryRejected(rejection),
            ),
            MacosCaptureError::FrameDeliveryDropped(rejection)
        );
        assert!(matches!(
            confirmed.state(),
            MacosStreamDeliveryState::Confirmed(_)
        ));
    }

    #[test]
    fn session_selection_identity_is_canonical_and_membership_exact() {
        let window_ids = vec![41, 7, 41];
        let application_ids = vec![
            "tech.hyperbliss.zeta".to_owned(),
            "tech.hyperbliss.alpha".to_owned(),
            "tech.hyperbliss.zeta".to_owned(),
        ];

        assert_eq!(
            session_selection_source_id(
                super::MacosCaptureContentStyle::Mixed,
                window_ids,
                application_ids,
            )
            .as_ref(),
            "macos:session:mixed:w7:w41:a21:tech.hyperbliss.alpha:a20:tech.hyperbliss.zeta"
        );
    }

    #[test]
    fn repick_preserves_the_live_record_until_replacement_confirms() {
        let tahoe = MacosTahoeCapabilities {
            content_tone_mapping_info: MacosRuntimeCapability::Present,
            screenshot_api: MacosRuntimeCapability::Present,
        };
        let shared = SessionShared::new(
            MacosProtectedSourceState::Live,
            super::MacosCaptureSelector::Auto,
            tahoe,
        );
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
        .expect("valid SDR delivery");
        let delivery = MacosValidatedStreamDelivery {
            configured,
            delivered,
        };
        shared.confirm_selection(
            super::MacosCaptureSelection::Display {
                source_id: Arc::from("display:a"),
            },
            Arc::from("display:a"),
            1,
            delivery,
        );

        shared
            .begin_resolution()
            .expect("repick resolution should begin");
        assert!(shared.tahoe_selection_for("display:a", 1).is_some());

        shared.confirm_selection(
            super::MacosCaptureSelection::Display {
                source_id: Arc::from("display:b"),
            },
            Arc::from("display:b"),
            2,
            delivery,
        );
        assert_eq!(shared.tahoe_selection_for("display:a", 1), None);
        assert!(shared.tahoe_selection_for("display:b", 2).is_some());

        shared.clear_tahoe_selection();
        assert_eq!(shared.tahoe_selection_for("display:b", 2), None);
    }

    #[test]
    fn pending_screenshot_capability_dispatches_no_native_call() {
        let (snapshot, fence, backend) =
            screenshot_fixture(MacosScreenshotReferenceCapability::PendingFirstFrame);
        let result = execute_screenshot_transaction(
            snapshot,
            fence,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(|_| panic!("pending capability must not complete asynchronously")),
        );

        assert_eq!(result, Err(MacosCaptureError::ScreenshotCapabilityPending));
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn sdr_screenshot_dispatches_one_configuration() {
        let capability = MacosScreenshotReferenceCapability::SdrOnly {
            source_id: Arc::from("display:a"),
            generation: 4,
        };
        let (snapshot, fence, backend) = screenshot_fixture(capability);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        execute_screenshot_transaction(
            snapshot,
            fence,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(move |result| result_tx.send(result).expect("receiver remains live")),
        )
        .expect("SDR transaction should start");
        assert_eq!(backend.calls(), vec![(7, MacosCaptureDynamicRange::Sdr)]);

        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Sdr,
            1,
        )));
        assert!(matches!(
            result_rx.recv().expect("SDR result should arrive"),
            Ok(MacosScreenshotReferenceSet::Sdr { .. })
        ));
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn paired_screenshot_dispatches_exactly_two_ranges_on_one_filter() {
        let capability = MacosScreenshotReferenceCapability::PairedSdrHdr {
            source_id: Arc::from("display:a"),
            generation: 4,
        };
        let (snapshot, fence, backend) = screenshot_fixture(capability);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        execute_screenshot_transaction(
            snapshot,
            fence,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(move |result| result_tx.send(result).expect("receiver remains live")),
        )
        .expect("paired transaction should start");

        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Sdr,
            1,
        )));
        assert_eq!(backend.calls(), vec![(7, MacosCaptureDynamicRange::Hdr)]);
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Hdr,
            2,
        )));
        assert!(matches!(
            result_rx.recv().expect("paired result should arrive"),
            Ok(MacosScreenshotReferenceSet::Paired { .. })
        ));
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn paired_screenshot_partial_failure_publishes_no_partial_set() {
        let capability = MacosScreenshotReferenceCapability::PairedSdrHdr {
            source_id: Arc::from("display:a"),
            generation: 4,
        };
        let (snapshot, fence, backend) = screenshot_fixture(capability);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        execute_screenshot_transaction(
            snapshot,
            fence,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(move |result| result_tx.send(result).expect("receiver remains live")),
        )
        .expect("paired transaction should start");
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Sdr,
            1,
        )));
        backend.complete_next(Err(MacosCaptureError::NativeOperation {
            operation: "fixture HDR screenshot",
            code: 9,
            message: "redacted".to_owned(),
        }));

        assert!(matches!(
            result_rx.recv().expect("failure should arrive"),
            Err(MacosCaptureError::NativeOperation { code: 9, .. })
        ));
    }

    #[test]
    fn repick_between_paired_callbacks_rejects_the_complete_pair() {
        let capability = MacosScreenshotReferenceCapability::PairedSdrHdr {
            source_id: Arc::from("display:a"),
            generation: 4,
        };
        let (snapshot, fence, backend) = screenshot_fixture(capability);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        execute_screenshot_transaction(
            snapshot,
            Arc::clone(&fence) as Arc<dyn ScreenshotIdentityFence>,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(move |result| result_tx.send(result).expect("receiver remains live")),
        )
        .expect("paired transaction should start");
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Sdr,
            1,
        )));
        super::lock(&fence.identity).2 = 12;
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Hdr,
            2,
        )));

        assert!(matches!(
            result_rx.recv().expect("fence failure should arrive"),
            Err(MacosCaptureError::ScreenshotSelectionChanged)
        ));
    }

    #[test]
    fn late_sdr_completion_after_consumer_timeout_is_inert() {
        let capability = MacosScreenshotReferenceCapability::SdrOnly {
            source_id: Arc::from("display:a"),
            generation: 4,
        };
        let (snapshot, fence, backend) = screenshot_fixture(capability);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        let completion_ran = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&completion_ran);
        execute_screenshot_transaction(
            snapshot,
            fence,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(move |result| {
                observed.store(true, Ordering::SeqCst);
                // The daemon consumer mirrors this: a timed-out receiver
                // makes the send fail, and the failure is discarded.
                drop(result_tx.send(result));
            }),
        )
        .expect("SDR transaction should start");

        // The requesting consumer gives up before ScreenCaptureKit answers.
        drop(result_rx);
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Sdr,
            1,
        )));

        assert!(completion_ran.load(Ordering::SeqCst));
        assert!(backend.calls().is_empty());
        assert_eq!(Arc::strong_count(&completion_ran), 1);
    }

    #[test]
    fn late_paired_completion_after_consumer_timeout_is_inert() {
        let capability = MacosScreenshotReferenceCapability::PairedSdrHdr {
            source_id: Arc::from("display:a"),
            generation: 4,
        };
        let (snapshot, fence, backend) = screenshot_fixture(capability);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        let completion_ran = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&completion_ran);
        execute_screenshot_transaction(
            snapshot,
            fence,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(move |result| {
                observed.store(true, Ordering::SeqCst);
                drop(result_tx.send(result));
            }),
        )
        .expect("paired transaction should start");
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Sdr,
            1,
        )));
        assert_eq!(backend.calls(), vec![(7, MacosCaptureDynamicRange::Hdr)]);

        drop(result_rx);
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Hdr,
            2,
        )));

        assert!(completion_ran.load(Ordering::SeqCst));
        assert!(backend.calls().is_empty());
        assert_eq!(Arc::strong_count(&completion_ran), 1);
    }

    #[test]
    fn canonical_hdr_preset_resolves_to_a_valid_hdr_configuration() {
        // SAFETY: The deployment floor includes this pure configuration
        // constructor, which does not start capture or request TCC access.
        let configuration = unsafe {
            SCStreamConfiguration::streamConfigurationWithPreset(
                SCStreamConfigurationPreset::CaptureHDRStreamCanonicalDisplay,
            )
        };
        // SAFETY: Both values are initialized scalar configuration properties.
        let configured = unsafe {
            let fourcc = configuration.pixelFormat();
            assert_eq!(
                configuration.captureDynamicRange(),
                SCCaptureDynamicRange::HDRCanonicalDisplay
            );
            MacosConfiguredStream {
                requested_dynamic_range: MacosCaptureDynamicRange::Hdr,
                requested_preset: MacosStreamPreset::CaptureHdrStreamCanonicalDisplay,
                configured_dynamic_range: capture_dynamic_range(
                    configuration.captureDynamicRange(),
                )
                .expect("preset dynamic range should decode"),
                configured_pixel_format: MacosCapturePixelFormat::from_fourcc(fourcc)
                    .expect("preset pixel format should be supported"),
                configured_color_range: color_range_from_fourcc(fourcc),
            }
        };
        configured
            .validate()
            .expect("canonical HDR preset should resolve to an accepted stream format");
    }

    #[test]
    fn conservative_bgra_pool_quote_covers_aligned_native_storage() {
        let extent = MacosPixelExtent::new(3_840, 2_160).expect("4K extent is valid");
        let quote = conservative_pool_quote(extent, MacosCapturePixelFormat::Bgra8)
            .expect("4K quote should fit");
        assert!(quote.per_surface_bytes >= 3_840 * 2_160 * 4);
        assert_eq!(quote.per_surface_bytes % (16 * 1024), 0);
        assert!(quote.stream_metadata_bytes > 0);
    }

    #[test]
    fn hdr_pool_quotes_cover_rgba16f_and_multiplane_storage() {
        let extent = MacosPixelExtent::new(3_840, 2_160).expect("4K extent is valid");
        let rgba = conservative_pool_quote(extent, MacosCapturePixelFormat::Rgba16Float)
            .expect("RGBA16F quote should fit");
        let yuv = conservative_pool_quote(extent, MacosCapturePixelFormat::Yuv420VideoRange)
            .expect("YUV quote should fit");
        assert!(rgba.per_surface_bytes >= 3_840 * 2_160 * 8);
        assert!(yuv.per_surface_bytes >= 3_840 * 2_160 * 3 / 2);
        assert_eq!(rgba.per_surface_bytes % (16 * 1024), 0);
        assert_eq!(yuv.per_surface_bytes % (16 * 1024), 0);
    }

    #[test]
    fn rejected_surface_never_reaches_the_retain_operation() {
        let pool = Arc::new(|_, _| -> Result<PoolBackingLifetime, MacosCaptureError> {
            Err(MacosCaptureError::ScreenResourceExhausted {
                requested_bytes: 128,
                available_bytes: 64,
            })
        }) as PoolObservation;
        let retained = AtomicBool::new(false);

        assert!(matches!(
            with_admitted_surface(&pool, 7, 128, |_| retained.store(true, Ordering::Release)),
            Err(MacosCaptureError::ScreenResourceExhausted { .. })
        ));
        assert!(!retained.load(Ordering::Acquire));
    }
}
