use std::num::NonZeroU32;
use std::time::Duration;

use hypercolor_core::input::screen::smooth::PreparedTemporalSmoother;
use hypercolor_core::input::screen::{
    CaptureConfig, CapturePixelFormat, CaptureTransferFunction, ColorTuning, ScreenColorTuning,
    ScreenContentBarsPolicy, ScreenCursorPolicy, ScreenGridPolicy, ScreenHdrPolicy,
    ScreenLetterboxFill, ScreenProcessingProfile, ScreenProcessingProfileConfig,
    ScreenReductionFilter, ScreenSceneCutPolicy, ScreenSmoothingPolicy, ScreenTargetColorimetry,
    ScreenToneMapOperator, ScreenUnknownColorPolicy,
};

fn requested_profile() -> ScreenProcessingProfile {
    ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
        letterbox_fill: ScreenLetterboxFill::EdgeExtend,
        cursor: ScreenCursorPolicy::Include,
        grid: ScreenGridPolicy::PointSample,
        reduction_filter: ScreenReductionFilter::Nearest,
        target_pixel_format: CapturePixelFormat::Bgra8,
        target_colorimetry: ScreenTargetColorimetry::PreserveSource,
        unknown_color: ScreenUnknownColorPolicy::PreserveEncodedSamples,
        algorithm_revision: NonZeroU32::new(41).expect("test revision is nonzero"),
        ..ScreenProcessingProfileConfig::default()
    })
}

#[test]
fn capture_config_derives_exact_managed_profile_without_overwriting_consumer_contracts() {
    let config = CaptureConfig {
        smoothing_alpha: 0.3,
        scene_cut_threshold: 100.0,
        letterbox_threshold: 0.04,
        letterbox_enabled: true,
        tuning: ColorTuning {
            saturation: 1.4,
            brightness: 0.8,
            gamma: 1.2,
        },
        target_led_white_x: 0.3000,
        target_led_white_y: 0.3200,
        target_led_reference_white_nits: 180.0,
        target_led_peak_nits: 500.0,
        exposure_ev: 1.25,
        ..CaptureConfig::default()
    };
    let requested = requested_profile();

    let profile = config
        .exact_processing_profile(&requested)
        .expect("valid runtime controls derive an exact profile");

    let ScreenContentBarsPolicy::DetectAndCrop {
        luminance_threshold,
    } = profile.content_bars()
    else {
        panic!("enabled content-bar detection remains enabled");
    };
    assert_eq!(luminance_threshold.value(), 0.04);
    let ScreenSmoothingPolicy::Exponential {
        time_constant,
        scene_cut,
    } = profile.smoothing()
    else {
        panic!("intermediate alpha derives an exponential profile");
    };
    let one_reference_frame = Duration::from_secs_f64(1.0 / 60.0);
    let derived_alpha =
        1.0 - (-(one_reference_frame.as_secs_f64() / time_constant.as_secs_f64())).exp();
    assert!((derived_alpha - 0.3).abs() < 1.0e-6);
    let ScreenSceneCutPolicy::MeanAbsoluteDelta { threshold } = scene_cut else {
        panic!("configured scene-cut threshold remains active");
    };
    assert!((threshold.value() - 100.0 / 765.0).abs() < f32::EPSILON);
    assert_eq!(
        profile.tuning(),
        ScreenColorTuning::try_new(1.4, 0.8, 1.2).expect("test tuning is finite")
    );
    assert_eq!(profile.cursor(), ScreenCursorPolicy::Include);
    assert_eq!(profile.grid(), ScreenGridPolicy::AreaWeighted);
    assert_eq!(profile.reduction_filter(), ScreenReductionFilter::Area);
    assert_eq!(profile.letterbox_fill(), ScreenLetterboxFill::EdgeExtend);
    assert_eq!(profile.target_pixel_format(), CapturePixelFormat::Bgra8);
    assert_eq!(
        profile.target_colorimetry(),
        ScreenTargetColorimetry::PreserveSource
    );
    assert_eq!(
        profile.unknown_color(),
        ScreenUnknownColorPolicy::PreserveEncodedSamples
    );
    assert_eq!(profile.algorithm_revision(), requested.algorithm_revision());
    let ScreenHdrPolicy::ToneMap(hdr) = profile.hdr() else {
        panic!("runtime LED calibration enables managed HDR tone mapping");
    };
    assert_eq!(hdr.operator(), ScreenToneMapOperator::Bt2390Eetf);
    assert_eq!(
        hdr.target_luminance(),
        profile.led_tone_map().target_luminance()
    );
    assert_eq!(profile.led_tone_map().target_white_x(), 0.3000);
    assert_eq!(profile.led_tone_map().target_white_y(), 0.3200);
    assert_eq!(profile.led_tone_map().target_reference_white_nits(), 180.0);
    assert_eq!(profile.led_tone_map().target_peak_nits(), 500.0);
    assert_eq!(profile.led_tone_map().exposure_ev(), 1.25);
}

#[test]
fn alpha_zero_freezes_exact_publication_history() {
    let config = CaptureConfig {
        smoothing_alpha: 0.0,
        scene_cut_threshold: 765.0,
        ..CaptureConfig::default()
    };
    let profile = config
        .exact_processing_profile(&ScreenProcessingProfile::default())
        .expect("frozen smoothing derives an exact profile");
    assert!(matches!(
        profile.smoothing(),
        ScreenSmoothingPolicy::Frozen {
            scene_cut: ScreenSceneCutPolicy::MeanAbsoluteDelta { threshold }
        } if threshold.value() == 1.0
    ));
    let mut smoother = PreparedTemporalSmoother::try_new(profile.smoothing(), 1, 1)
        .expect("frozen smoother reserves transactional history");
    let mut colors = [[0, 0, 0]];
    smoother
        .stage(
            &mut colors,
            1,
            1,
            CaptureTransferFunction::Srgb,
            Duration::ZERO,
            false,
            false,
        )
        .expect("baseline history stages");
    assert!(smoother.commit_staged());

    colors[0] = [255, 255, 255];
    smoother
        .stage(
            &mut colors,
            1,
            1,
            CaptureTransferFunction::Srgb,
            Duration::from_secs(10),
            false,
            false,
        )
        .expect("frozen response stages");

    assert_eq!(colors, [[0, 0, 0]]);
    assert!(smoother.commit_staged());
}

#[test]
fn frozen_smoothing_still_resets_on_configured_scene_cuts() {
    let config = CaptureConfig {
        smoothing_alpha: 0.0,
        scene_cut_threshold: 100.0,
        ..CaptureConfig::default()
    };
    let profile = config
        .exact_processing_profile(&ScreenProcessingProfile::default())
        .expect("frozen smoothing derives an exact profile");
    let mut smoother = PreparedTemporalSmoother::try_new(profile.smoothing(), 1, 1)
        .expect("frozen smoother reserves transactional history");
    let mut colors = [[0, 0, 0]];
    smoother
        .stage(
            &mut colors,
            1,
            1,
            CaptureTransferFunction::Srgb,
            Duration::ZERO,
            false,
            false,
        )
        .expect("baseline history stages");
    assert!(smoother.commit_staged());

    colors[0] = [255, 255, 255];
    smoother
        .stage(
            &mut colors,
            1,
            1,
            CaptureTransferFunction::Srgb,
            Duration::from_millis(16),
            false,
            false,
        )
        .expect("scene cut stages");

    assert_eq!(colors, [[255, 255, 255]]);
}
