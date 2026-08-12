//! ScreenCaptureKit core worker fixture contracts.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureConfig, MacosScreenCaptureFixture, PixelExtent, ScreenAdmissionCapacity,
    ScreenByteAdmissionCoordinator, ScreenCaptureDemand,
};
use hypercolor_core::input::{
    InputData, InputManager, InputSource, MacosArchitecture, MacosAuthorizationState,
    MacosCapabilityOwner, MacosDaemonOwnerConflict,
    MacosProtectedSourceState as CoreProtectedSourceState, MacosSelectionState,
    SourcePlatformStatus,
};
use hypercolor_macos_capture::{
    MacosAttachment, MacosCaptureCapabilities, MacosCaptureColorimetry, MacosCaptureError,
    MacosCaptureFrame, MacosCapturePixelFormat, MacosCaptureSelection, MacosCaptureSurface,
    MacosColorPrimaries, MacosColorRange, MacosFrameDecoder, MacosFrameEvent,
    MacosHostArchitecture, MacosPixelExtent, MacosPointRect, MacosProtectedSourceState,
    MacosRawCapturePlane, MacosRawCaptureSample, MacosRawCompleteFrame, MacosRawFrameAttachments,
    MacosRuntimeCapability, MacosTahoeRuntimeProbes, MacosTahoeSelectionCapabilities,
    MacosTransferFunction,
};

const BGRA8: u32 = 0x4247_5241;

fn fixture_frame(epoch: u64, pixel: [u8; 4]) -> MacosCaptureFrame {
    let extent = MacosPixelExtent::new(4, 2).expect("fixture extent is valid");
    let stride = 16;
    let bytes = Arc::<[u8]>::from(pixel.repeat(8));
    let surface = MacosCaptureSurface::new_cpu_fixture(7, 32, epoch, vec![bytes])
        .expect("fixture surface is valid");
    let sample = MacosRawCaptureSample {
        frame: Some(MacosRawCompleteFrame {
            storage_extent: extent,
            planes: vec![MacosRawCapturePlane {
                index: 0,
                extent,
                bytes_per_row: stride,
                length_bytes: 32,
            }],
            pixel_format_fourcc: BGRA8,
            color: MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Srgb,
                transfer: MacosTransferFunction::Srgb,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            cursor_composed: true,
            surface,
        }),
        attachments: MacosRawFrameAttachments {
            status: MacosAttachment::Value(0),
            display_time: MacosAttachment::Value(epoch * 1_000),
            display_scale_factor: MacosAttachment::Value(1.0),
            content_scale: MacosAttachment::Value(1.0),
            content_rect: MacosAttachment::Value(
                MacosPointRect::new(0.0, 0.0, 4.0, 2.0).expect("content rect is valid"),
            ),
            dirty_rects: MacosAttachment::Missing,
            screen_rect: MacosAttachment::Missing,
            bounding_rect: MacosAttachment::Missing,
        },
    };
    let mut decoder = MacosFrameDecoder::new(epoch);
    let MacosFrameEvent::Frame(frame) = decoder.decode(sample).expect("fixture frame decodes")
    else {
        panic!("complete sample must decode as a frame");
    };
    assert_eq!(frame.pixel_format, MacosCapturePixelFormat::Bgra8);
    *frame
}

fn fixture_source(
    config: CaptureConfig,
) -> (
    hypercolor_core::input::screen::MacosScreenCaptureInput,
    MacosScreenCaptureFixture,
) {
    let admission =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
    MacosScreenCaptureFixture::source(config, admission)
}

fn wait_for_screen(source: &mut impl InputSource) -> hypercolor_core::input::ScreenData {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match source.sample().expect("fixture sample succeeds") {
            InputData::Screen(data) => return data,
            InputData::None if Instant::now() < deadline => thread::yield_now(),
            InputData::None => panic!("fixture worker did not publish before the deadline"),
            _ => panic!("macOS fixture published the wrong input kind"),
        }
    }
}

fn wait_for_grid_width(
    source: &mut impl InputSource,
    grid_width: u32,
) -> hypercolor_core::input::ScreenData {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match source.sample().expect("fixture sample succeeds") {
            InputData::Screen(data) if data.grid_width == grid_width => return data,
            InputData::Screen(_) | InputData::None if Instant::now() < deadline => {
                thread::yield_now();
            }
            InputData::Screen(_) | InputData::None => {
                panic!("fixture worker did not publish the expected grid before the deadline");
            }
            _ => panic!("macOS fixture published the wrong input kind"),
        }
    }
}

#[test]
fn fixture_capture_activates_only_for_live_demand() {
    let config = CaptureConfig {
        target_fps: 60,
        grid_cols: 2,
        grid_rows: 1,
        smoothing_alpha: 1.0,
        ..CaptureConfig::default()
    };
    let (mut source, fixture) = fixture_source(config);
    fixture.set_host_capabilities(MacosCaptureCapabilities::from_runtime(
        MacosHostArchitecture::Intel,
        false,
        MacosTahoeRuntimeProbes {
            content_tone_mapping_info_symbol: MacosRuntimeCapability::Absent,
            screenshot_configuration_class: MacosRuntimeCapability::Present,
            screenshot_dynamic_range_selector: MacosRuntimeCapability::Present,
            screenshot_capture_selector: MacosRuntimeCapability::Present,
        },
    ));

    assert_eq!(source.name(), "macos_screen_capture");
    assert_eq!(
        source.protected_state(),
        MacosProtectedSourceState::ReadyIdle
    );
    source
        .set_macos_daemon_ownership(
            MacosCapabilityOwner::AppSidecar,
            Some(MacosDaemonOwnerConflict {
                active: MacosCapabilityOwner::AppSidecar,
                contender: MacosCapabilityOwner::HomebrewService,
                observed_at_ms: 42,
            }),
            Some(Arc::from("designated-app-sidecar")),
        )
        .expect("fixture owner status updates");
    source
        .set_macos_metal4_capability(true)
        .expect("fixture Metal 4 status updates");
    source
        .source_status_reporter()
        .expect("macOS fixture exposes status reporting")
        .set_source_graph_generation(1);
    let status = source
        .source_status_handle()
        .expect("macOS fixture exposes status");
    let initial = status.snapshot();
    let Some(SourcePlatformStatus::MacosScreen(platform)) = initial.platform.as_deref() else {
        panic!("expected macOS screen platform status");
    };
    assert_eq!(platform.state, CoreProtectedSourceState::ReadyIdle);
    assert_eq!(platform.tcc, MacosAuthorizationState::Authorized);
    assert_eq!(platform.owner, MacosCapabilityOwner::AppSidecar);
    assert_eq!(
        platform.owner_conflict.as_deref(),
        Some(&MacosDaemonOwnerConflict {
            active: MacosCapabilityOwner::AppSidecar,
            contender: MacosCapabilityOwner::HomebrewService,
            observed_at_ms: 42,
        })
    );
    assert_eq!(platform.selection, MacosSelectionState::None);
    assert_eq!(platform.selection_diagnostic_label, None);
    assert_eq!(platform.tahoe.host_architecture, MacosArchitecture::Intel);
    assert!(!platform.tahoe.translated_process);
    assert!(!platform.tahoe.content_tone_mapping_info);
    assert!(platform.tahoe.metal4);
    assert_eq!(platform.stream_state.as_ref(), "inactive");
    assert_eq!(platform.queue_depth, 8);
    assert_eq!(platform.admitted_native_bytes, 0);
    assert_eq!(platform.frames_received, 0);
    assert_eq!(platform.frames_published, 0);
    assert_eq!(platform.publication_path, None);
    assert!(!fixture.is_active());
    source.start().expect("fixture source starts idle");
    assert!(matches!(source.sample(), Ok(InputData::None)));

    source
        .set_screen_capture_demand(ScreenCaptureDemand::try_active(4, 2).expect("valid demand"))
        .expect("fixture demand activates");
    assert!(fixture.is_active());
    let source_id = Arc::from("display:00000000-0000-0000-0000-000000000001");
    fixture.set_selection(MacosCaptureSelection::Display {
        source_id: Arc::clone(&source_id),
    });
    assert!(matches!(source.sample(), Ok(InputData::None)));
    let selected = status.snapshot();
    let Some(SourcePlatformStatus::MacosScreen(platform)) = selected.platform.as_deref() else {
        panic!("expected selected macOS screen platform status");
    };
    assert_eq!(platform.tahoe_selection, None);

    fixture.set_tahoe_selection_capabilities(Some(MacosTahoeSelectionCapabilities {
        source_id: Arc::clone(&source_id),
        capture_session_generation: 1,
        hdr_capture: true,
        dual_range_screenshots: false,
    }));
    let captured_at = Instant::now();
    fixture.publish_at(fixture_frame(1, [0, 0, 255, 255]), captured_at);
    let data = wait_for_screen(&mut source);
    assert_eq!(data.grid_width, 2);
    assert_eq!(data.grid_height, 1);
    assert_eq!(data.source_width, 4);
    assert_eq!(data.source_height, 2);
    assert_eq!(data.zone_colors.len(), 2);
    let live = status.snapshot();
    assert_eq!(live.last_sample_at, Some(captured_at));
    let Some(SourcePlatformStatus::MacosScreen(platform)) = live.platform.as_deref() else {
        panic!("expected live macOS screen platform status");
    };
    assert_eq!(platform.state, CoreProtectedSourceState::Live);
    assert_eq!(platform.stream_state.as_ref(), "active");
    assert_eq!(platform.capture_session_generation, Some(1));
    assert_eq!(platform.topology_generation, Some(1));
    assert_eq!(platform.resource_generation, Some(1));
    assert_eq!(platform.pixel_format.as_deref(), Some("bgra8"));
    assert_eq!(platform.dynamic_range.as_deref(), Some("standard"));
    assert_eq!(platform.color_space.as_deref(), Some("srgb"));
    assert_eq!(platform.transfer_function.as_deref(), Some("srgb"));
    assert_eq!(platform.native_width, Some(4));
    assert_eq!(platform.native_height, Some(2));
    assert!(platform.frames_received >= 1);
    assert!(platform.frames_published >= 1);
    assert_eq!(
        platform.selection,
        MacosSelectionState::Display {
            source_id: Arc::clone(&source_id),
        }
    );
    assert_eq!(
        platform.selection_diagnostic_label.as_deref(),
        Some("display")
    );
    let tahoe = platform
        .tahoe_selection
        .as_ref()
        .expect("confirmed stream should publish Tahoe selection capabilities");
    assert_eq!(tahoe.source_id, source_id);
    assert_eq!(tahoe.capture_session_generation, 1);
    assert!(tahoe.hdr_capture);
    assert!(!tahoe.dual_range_screenshots);

    let replacement_source_id = Arc::from("display:00000000-0000-0000-0000-000000000002");
    fixture.set_selection(MacosCaptureSelection::Display {
        source_id: Arc::clone(&replacement_source_id),
    });
    assert!(matches!(source.sample(), Ok(InputData::Screen(_))));
    let repicked = status.snapshot();
    let Some(SourcePlatformStatus::MacosScreen(platform)) = repicked.platform.as_deref() else {
        panic!("expected repicked macOS screen platform status");
    };
    assert_eq!(platform.tahoe_selection, None);

    fixture.set_tahoe_selection_capabilities(Some(MacosTahoeSelectionCapabilities {
        source_id: Arc::clone(&replacement_source_id),
        capture_session_generation: 2,
        hdr_capture: false,
        dual_range_screenshots: true,
    }));
    assert!(matches!(source.sample(), Ok(InputData::Screen(_))));
    let reconfirmed = status.snapshot();
    let Some(SourcePlatformStatus::MacosScreen(platform)) = reconfirmed.platform.as_deref() else {
        panic!("expected reconfirmed macOS screen platform status");
    };
    let tahoe = platform
        .tahoe_selection
        .as_ref()
        .expect("replacement stream should publish Tahoe selection capabilities");
    assert_eq!(tahoe.source_id, replacement_source_id);
    assert_eq!(tahoe.capture_session_generation, 2);
    assert!(!tahoe.hdr_capture);
    assert!(tahoe.dual_range_screenshots);

    source
        .set_screen_capture_demand(ScreenCaptureDemand::Inactive)
        .expect("fixture demand deactivates");
    assert!(!fixture.is_active());
    assert!(matches!(source.sample(), Ok(InputData::None)));
    let inactive = status.snapshot();
    let Some(SourcePlatformStatus::MacosScreen(platform)) = inactive.platform.as_deref() else {
        panic!("expected inactive macOS screen platform status");
    };
    assert_eq!(platform.state, CoreProtectedSourceState::ReadyIdle);
    assert_eq!(platform.tahoe_selection, None);
}

#[test]
fn stale_native_frame_never_enters_the_legacy_cpu_publication() {
    let (mut source, fixture) = fixture_source(CaptureConfig {
        target_fps: 60,
        ..CaptureConfig::default()
    });
    let status = source
        .source_status_handle()
        .expect("macOS fixture exposes status");
    source.start().expect("fixture source starts idle");
    source
        .set_screen_capture_demand(ScreenCaptureDemand::active(
            PixelExtent::new(4, 2).expect("fixture demand is valid"),
        ))
        .expect("fixture demand activates");
    fixture.set_selection(MacosCaptureSelection::Display {
        source_id: Arc::from("display:stale-fixture"),
    });
    fixture.publish_at(
        fixture_frame(1, [0, 0, 255, 255]),
        Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("fixture clock has one second of history"),
    );

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        assert!(matches!(source.sample(), Ok(InputData::None)));
        let snapshot = status.snapshot();
        let Some(SourcePlatformStatus::MacosScreen(platform)) = snapshot.platform.as_deref() else {
            panic!("expected macOS screen platform status");
        };
        if platform.frames_stale == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "stale frame was not observed");
        thread::yield_now();
    }
}

#[test]
fn reconfiguration_fences_the_previous_worker_generation() {
    let config = CaptureConfig {
        target_fps: 60,
        grid_cols: 2,
        grid_rows: 1,
        smoothing_alpha: 1.0,
        ..CaptureConfig::default()
    };
    let (mut source, fixture) = fixture_source(config.clone());
    source.start().expect("fixture source starts idle");
    source
        .set_screen_capture_demand(ScreenCaptureDemand::active(
            PixelExtent::new(4, 2).expect("fixture demand is valid"),
        ))
        .expect("fixture demand activates");
    fixture.publish(fixture_frame(1, [255, 0, 0, 255]));
    assert_eq!(wait_for_screen(&mut source).zone_colors.len(), 2);

    source
        .reconfigure_screen_capture(&CaptureConfig {
            grid_cols: 1,
            grid_rows: 1,
            ..config
        })
        .expect("fixture worker reconfigures");
    let retained = source.sample().expect("last-good frame remains readable");
    let InputData::Screen(retained) = retained else {
        panic!("expected retained screen data during reconfiguration");
    };
    assert_eq!(retained.grid_width, 2);
    fixture.publish_recoverable_error(MacosCaptureError::DisplayUuidUnavailable(7));
    let retained = source
        .sample()
        .expect("recoverable repick error preserves last-good data");
    let InputData::Screen(retained) = retained else {
        panic!("expected retained screen data after recoverable repick error");
    };
    assert_eq!(retained.grid_width, 2);

    fixture.publish(fixture_frame(2, [0, 255, 0, 255]));
    let data = wait_for_grid_width(&mut source, 1);
    assert_eq!(data.grid_width, 1);
    assert_eq!(data.grid_height, 1);
    assert_eq!(data.zone_colors.len(), 1);
}

#[test]
fn authorization_and_picker_actions_run_outside_graph_ownership() {
    let (mut source, _) = fixture_source(CaptureConfig::default());
    let status = source
        .source_status_handle()
        .expect("macOS fixture exposes status");
    let authorize = source
        .screen_authorization_action()
        .expect("screen source exposes authorization");
    let picker = source
        .screen_source_picker_action()
        .expect("screen source exposes picker action");

    assert!(authorize().expect("fixture authorization succeeds"));
    picker().expect("fixture picker succeeds");
    source.sample().expect("source refreshes platform status");

    let snapshot = status.snapshot();
    let Some(SourcePlatformStatus::MacosScreen(platform)) = snapshot.platform.as_deref() else {
        panic!("fixture should publish macOS screen status");
    };
    assert_eq!(platform.tcc, MacosAuthorizationState::Authorized);
    assert_eq!(platform.state, CoreProtectedSourceState::NeedsSelection);
}

#[test]
fn late_macos_capture_source_inherits_process_capabilities() {
    let (source, _) = fixture_source(CaptureConfig::default());
    let status = source
        .source_status_handle()
        .expect("macOS fixture exposes status");
    let conflict = MacosDaemonOwnerConflict {
        active: MacosCapabilityOwner::HomebrewService,
        contender: MacosCapabilityOwner::AppSidecar,
        observed_at_ms: 42,
    };
    let mut manager = InputManager::new();
    manager
        .set_macos_daemon_ownership(
            MacosCapabilityOwner::HomebrewService,
            Some(conflict.clone()),
            Some(Arc::from("designated-homebrew")),
        )
        .expect("manager retains ownership before source registration");
    manager
        .set_macos_metal4_capability(true)
        .expect("manager retains Metal 4 before source registration");

    manager.add_source(Box::new(source));

    let snapshot = status.snapshot();
    let Some(SourcePlatformStatus::MacosScreen(platform)) = snapshot.platform.as_deref() else {
        panic!("expected macOS screen platform status");
    };
    assert_eq!(platform.owner, MacosCapabilityOwner::HomebrewService);
    assert_eq!(platform.owner_conflict.as_deref(), Some(&conflict));
    assert_eq!(
        platform.owner_designated_requirement_hash.as_deref(),
        Some("designated-homebrew")
    );
    assert!(platform.tahoe.metal4);
}
