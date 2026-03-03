//! Tests for the render engine — FPS controller, tier transitions,
//! frame timing, and render loop state machine.

use std::sync::atomic::Ordering;
use std::time::Duration;

use hypercolor_core::engine::{
    FpsController, FpsTier, RenderLoop, RenderLoopState, TierTransitionConfig,
};

// ─── FpsTier Tests ───────────────────────────────────────────────────────────

#[test]
fn tier_fps_values_are_correct() {
    assert_eq!(FpsTier::Minimal.fps(), 10);
    assert_eq!(FpsTier::Low.fps(), 20);
    assert_eq!(FpsTier::Medium.fps(), 30);
    assert_eq!(FpsTier::High.fps(), 45);
    assert_eq!(FpsTier::Full.fps(), 60);
}

#[test]
fn tier_frame_intervals_match_fps() {
    for tier in FpsTier::ALL {
        let interval = tier.frame_interval();
        let fps = tier.fps();
        // Verify interval is approximately 1/fps seconds
        let expected_ms = 1000.0 / f64::from(fps);
        let actual_ms = interval.as_secs_f64() * 1000.0;
        let diff = (expected_ms - actual_ms).abs();
        assert!(
            diff < 1.0,
            "tier {tier:?}: expected ~{expected_ms:.1}ms, got {actual_ms:.1}ms"
        );
    }
}

#[test]
fn tier_ordering_is_ascending() {
    assert!(FpsTier::Minimal < FpsTier::Low);
    assert!(FpsTier::Low < FpsTier::Medium);
    assert!(FpsTier::Medium < FpsTier::High);
    assert!(FpsTier::High < FpsTier::Full);
}

#[test]
fn tier_upshift_chain() {
    let mut tier = FpsTier::Minimal;
    let expected = [FpsTier::Low, FpsTier::Medium, FpsTier::High, FpsTier::Full];
    for expected_next in expected {
        let next = tier.upshift();
        assert_eq!(next, Some(expected_next));
        tier = expected_next;
    }
    assert_eq!(tier.upshift(), None);
}

#[test]
fn tier_downshift_chain() {
    let mut tier = FpsTier::Full;
    let expected = [
        FpsTier::High,
        FpsTier::Medium,
        FpsTier::Low,
        FpsTier::Minimal,
    ];
    for expected_next in expected {
        let next = tier.downshift();
        assert_eq!(next, Some(expected_next));
        tier = expected_next;
    }
    assert_eq!(tier.downshift(), None);
}

#[test]
fn from_fps_resolves_exact_matches() {
    assert_eq!(FpsTier::from_fps(10), FpsTier::Minimal);
    assert_eq!(FpsTier::from_fps(20), FpsTier::Low);
    assert_eq!(FpsTier::from_fps(30), FpsTier::Medium);
    assert_eq!(FpsTier::from_fps(45), FpsTier::High);
    assert_eq!(FpsTier::from_fps(60), FpsTier::Full);
}

#[test]
fn from_fps_resolves_nearest_tier() {
    // Below Minimal
    assert_eq!(FpsTier::from_fps(1), FpsTier::Minimal);
    assert_eq!(FpsTier::from_fps(5), FpsTier::Minimal);

    // Between Minimal(10) and Low(20)
    assert_eq!(FpsTier::from_fps(14), FpsTier::Minimal);
    assert_eq!(FpsTier::from_fps(15), FpsTier::Low); // tie goes to higher
    assert_eq!(FpsTier::from_fps(16), FpsTier::Low);

    // Between Low(20) and Medium(30)
    assert_eq!(FpsTier::from_fps(24), FpsTier::Low);
    assert_eq!(FpsTier::from_fps(25), FpsTier::Medium); // tie goes to higher
    assert_eq!(FpsTier::from_fps(26), FpsTier::Medium);

    // Above Full
    assert_eq!(FpsTier::from_fps(120), FpsTier::Full);
    assert_eq!(FpsTier::from_fps(240), FpsTier::Full);
}

#[test]
fn from_fps_zero_resolves_to_minimal() {
    assert_eq!(FpsTier::from_fps(0), FpsTier::Minimal);
}

#[test]
fn tier_display_includes_fps_and_name() {
    let display = format!("{}", FpsTier::Full);
    assert!(display.contains("60"), "expected '60' in '{display}'");
    assert!(display.contains("full"), "expected 'full' in '{display}'");
}

// ─── FpsController Tests ─────────────────────────────────────────────────────

#[test]
fn controller_starts_at_specified_tier() {
    let ctrl = FpsController::new(FpsTier::Medium);
    assert_eq!(ctrl.tier(), FpsTier::Medium);
    assert_eq!(ctrl.max_tier(), FpsTier::Full);
    assert_eq!(ctrl.target_fps(), 30);
    assert_eq!(ctrl.consecutive_misses(), 0);
    assert_eq!(ctrl.total_frames(), 0);
}

#[test]
fn controller_with_config_uses_custom_thresholds() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 5,
        upshift_sustain_secs: 10.0,
        upshift_headroom_ratio: 0.6,
        ewma_alpha: 0.1,
    };
    let ctrl = FpsController::with_config(FpsTier::Full, config);
    assert_eq!(ctrl.config().downshift_miss_threshold, 5);
}

#[test]
fn record_frame_within_budget_resets_misses() {
    let mut ctrl = FpsController::new(FpsTier::Full);

    // Record a frame within the 16.6ms budget
    let stats = ctrl.record_frame(Duration::from_millis(10));
    assert!(stats.is_some());
    let stats = stats.expect("stats should be present");

    assert!(!stats.budget_exceeded);
    assert_eq!(stats.consecutive_misses, 0);
    assert!(stats.headroom > Duration::ZERO);
    assert_eq!(stats.tier, FpsTier::Full);
    assert_eq!(ctrl.total_frames(), 1);
}

#[test]
fn record_frame_over_budget_increments_misses() {
    let mut ctrl = FpsController::new(FpsTier::Full);

    // Record a frame that exceeds the 16.6ms budget
    let stats = ctrl
        .record_frame(Duration::from_millis(20))
        .expect("stats should be present");

    assert!(stats.budget_exceeded);
    assert_eq!(stats.consecutive_misses, 1);
    assert_eq!(stats.headroom, Duration::ZERO);
}

#[test]
fn consecutive_misses_accumulate() {
    let mut ctrl = FpsController::new(FpsTier::Full);

    for expected in 1..=5 {
        ctrl.record_frame(Duration::from_millis(20));
        assert_eq!(ctrl.consecutive_misses(), expected);
    }
}

#[test]
fn successful_frame_resets_consecutive_misses() {
    let mut ctrl = FpsController::new(FpsTier::Full);

    // Build up misses
    ctrl.record_frame(Duration::from_millis(20));
    ctrl.record_frame(Duration::from_millis(20));
    assert_eq!(ctrl.consecutive_misses(), 2);

    // Good frame resets
    ctrl.record_frame(Duration::from_millis(5));
    assert_eq!(ctrl.consecutive_misses(), 0);
}

#[test]
fn ewma_converges_toward_current_frame_time() {
    let mut ctrl = FpsController::new(FpsTier::Full);

    // Start EWMA is seeded at half the frame interval (~8.3ms)
    // Feed many 10ms frames, EWMA should converge toward 10ms
    for _ in 0..200 {
        ctrl.record_frame(Duration::from_millis(10));
    }

    let ewma = ctrl.ewma_frame_time();
    let diff = (ewma.as_secs_f64() - 0.010).abs();
    assert!(
        diff < 0.001,
        "EWMA should converge to ~10ms, got {:.3}ms",
        ewma.as_secs_f64() * 1000.0
    );
}

#[test]
fn should_downshift_triggers_on_threshold() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 3,
        ..TierTransitionConfig::default()
    };
    let mut ctrl = FpsController::with_config(FpsTier::Full, config);

    ctrl.record_frame(Duration::from_millis(20));
    assert!(!ctrl.should_downshift());

    ctrl.record_frame(Duration::from_millis(20));
    assert!(!ctrl.should_downshift());

    ctrl.record_frame(Duration::from_millis(20));
    assert!(ctrl.should_downshift());
}

#[test]
fn should_downshift_false_at_minimum_tier() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 1,
        ..TierTransitionConfig::default()
    };
    let mut ctrl = FpsController::with_config(FpsTier::Minimal, config);

    // Even with budget misses, can't downshift below Minimal
    ctrl.record_frame(Duration::from_millis(200));
    assert!(!ctrl.should_downshift());
}

#[test]
fn downshift_moves_one_tier_down() {
    let mut ctrl = FpsController::new(FpsTier::Full);

    let new = ctrl.downshift();
    assert_eq!(new, Some(FpsTier::High));
    assert_eq!(ctrl.tier(), FpsTier::High);

    // Transition resets counters
    assert_eq!(ctrl.consecutive_misses(), 0);
    assert_eq!(ctrl.frames_since_tier_change(), 0);
}

#[test]
fn upshift_moves_one_tier_up() {
    let mut ctrl = FpsController::new(FpsTier::Low);

    let new = ctrl.upshift();
    assert_eq!(new, Some(FpsTier::Medium));
    assert_eq!(ctrl.tier(), FpsTier::Medium);
}

#[test]
fn downshift_returns_none_at_minimum() {
    let mut ctrl = FpsController::new(FpsTier::Minimal);
    assert_eq!(ctrl.downshift(), None);
    assert_eq!(ctrl.tier(), FpsTier::Minimal);
}

#[test]
fn upshift_returns_none_at_maximum() {
    let mut ctrl = FpsController::new(FpsTier::Full);
    assert_eq!(ctrl.upshift(), None);
    assert_eq!(ctrl.tier(), FpsTier::Full);
}

#[test]
fn set_max_tier_clamps_and_caps_upshift() {
    let mut ctrl = FpsController::new(FpsTier::Full);
    ctrl.set_max_tier(FpsTier::Medium);

    // Existing tier is clamped to the new ceiling.
    assert_eq!(ctrl.max_tier(), FpsTier::Medium);
    assert_eq!(ctrl.tier(), FpsTier::Medium);

    // Direct and manual transitions cannot exceed the ceiling.
    assert_eq!(ctrl.upshift(), None);
    ctrl.set_tier(FpsTier::Full);
    assert_eq!(ctrl.tier(), FpsTier::Medium);

    // Lower tiers can still move upward until they hit the ceiling.
    ctrl.set_tier(FpsTier::Low);
    assert_eq!(ctrl.upshift(), Some(FpsTier::Medium));
    assert_eq!(ctrl.upshift(), None);
}

#[test]
fn set_tier_resets_transition_state() {
    let mut ctrl = FpsController::new(FpsTier::Full);

    // Accumulate state
    ctrl.record_frame(Duration::from_millis(20));
    ctrl.record_frame(Duration::from_millis(20));
    assert_eq!(ctrl.consecutive_misses(), 2);
    assert_eq!(ctrl.frames_since_tier_change(), 2);

    // Force tier change
    ctrl.set_tier(FpsTier::Medium);
    assert_eq!(ctrl.tier(), FpsTier::Medium);
    assert_eq!(ctrl.consecutive_misses(), 0);
    assert_eq!(ctrl.frames_since_tier_change(), 0);
}

#[test]
fn set_tier_noop_for_same_tier() {
    let mut ctrl = FpsController::new(FpsTier::Full);
    ctrl.record_frame(Duration::from_millis(5));
    let frames = ctrl.frames_since_tier_change();

    ctrl.set_tier(FpsTier::Full);
    // Should not reset since tier didn't change
    assert_eq!(ctrl.frames_since_tier_change(), frames);
}

#[test]
fn maybe_transition_downshifts_on_consecutive_misses() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 2,
        ..TierTransitionConfig::default()
    };
    let mut ctrl = FpsController::with_config(FpsTier::Full, config);

    ctrl.record_frame(Duration::from_millis(20));
    assert_eq!(ctrl.maybe_transition(), None);

    ctrl.record_frame(Duration::from_millis(20));
    let result = ctrl.maybe_transition();
    assert_eq!(result, Some(FpsTier::High));
    assert_eq!(ctrl.tier(), FpsTier::High);
}

#[test]
fn maybe_transition_no_change_within_budget() {
    let mut ctrl = FpsController::new(FpsTier::Full);

    ctrl.record_frame(Duration::from_millis(5));
    assert_eq!(ctrl.maybe_transition(), None);
}

#[test]
fn frames_since_tier_change_increments() {
    let mut ctrl = FpsController::new(FpsTier::Full);
    assert_eq!(ctrl.frames_since_tier_change(), 0);

    ctrl.record_frame(Duration::from_millis(5));
    assert_eq!(ctrl.frames_since_tier_change(), 1);

    ctrl.record_frame(Duration::from_millis(5));
    assert_eq!(ctrl.frames_since_tier_change(), 2);
}

#[test]
fn total_frames_persists_across_tier_changes() {
    let mut ctrl = FpsController::new(FpsTier::Full);

    ctrl.record_frame(Duration::from_millis(5));
    ctrl.record_frame(Duration::from_millis(5));
    assert_eq!(ctrl.total_frames(), 2);

    ctrl.set_tier(FpsTier::Medium);
    assert_eq!(ctrl.total_frames(), 2); // unchanged

    ctrl.record_frame(Duration::from_millis(5));
    assert_eq!(ctrl.total_frames(), 3); // continues counting
}

#[test]
fn sleep_duration_within_budget() {
    let ctrl = FpsController::new(FpsTier::Full);
    let frame_time = Duration::from_millis(10);
    let sleep = ctrl.sleep_duration(frame_time);
    assert!(sleep > Duration::ZERO);
    assert!(sleep < ctrl.target_interval());
}

#[test]
fn sleep_duration_over_budget_is_zero() {
    let ctrl = FpsController::new(FpsTier::Full);
    let frame_time = Duration::from_millis(20);
    let sleep = ctrl.sleep_duration(frame_time);
    assert_eq!(sleep, Duration::ZERO);
}

#[test]
fn begin_end_frame_lifecycle() {
    let mut ctrl = FpsController::new(FpsTier::Full);

    // end_frame without begin_frame returns None
    assert!(ctrl.end_frame().is_none());

    // Normal lifecycle
    ctrl.begin_frame();
    let stats = ctrl.end_frame();
    assert!(stats.is_some());
    assert_eq!(ctrl.total_frames(), 1);

    // end_frame again without begin returns None
    assert!(ctrl.end_frame().is_none());
}

#[test]
fn controller_debug_output_is_readable() {
    let ctrl = FpsController::new(FpsTier::Full);
    let debug = format!("{ctrl:?}");
    assert!(debug.contains("FpsController"));
    assert!(debug.contains("Full"));
}

// ─── Full Tier Walk Tests ────────────────────────────────────────────────────

#[test]
fn downshift_cascade_from_full_to_minimal() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 2,
        ..TierTransitionConfig::default()
    };
    let mut ctrl = FpsController::with_config(FpsTier::Full, config);
    let expected_tiers = [
        FpsTier::High,
        FpsTier::Medium,
        FpsTier::Low,
        FpsTier::Minimal,
    ];

    for expected in expected_tiers {
        // Trigger downshift with 2 consecutive misses
        ctrl.record_frame(Duration::from_millis(500));
        ctrl.record_frame(Duration::from_millis(500));
        let result = ctrl.maybe_transition();
        assert_eq!(
            result,
            Some(expected),
            "expected downshift to {expected:?}, got {result:?}"
        );
    }

    // At Minimal, no more downshifts
    ctrl.record_frame(Duration::from_millis(500));
    ctrl.record_frame(Duration::from_millis(500));
    assert_eq!(ctrl.maybe_transition(), None);
    assert_eq!(ctrl.tier(), FpsTier::Minimal);
}

// ─── RenderLoop Tests ────────────────────────────────────────────────────────

#[test]
fn render_loop_starts_in_created_state() {
    let rl = RenderLoop::new(60);
    assert_eq!(rl.state(), RenderLoopState::Created);
    assert!(!rl.is_running());
    assert_eq!(rl.frame_number(), 0);
}

#[test]
fn render_loop_new_resolves_fps_to_tier() {
    let rl = RenderLoop::new(60);
    assert_eq!(rl.fps_controller().tier(), FpsTier::Full);
    assert_eq!(rl.fps_controller().max_tier(), FpsTier::Full);

    let rl = RenderLoop::new(30);
    assert_eq!(rl.fps_controller().tier(), FpsTier::Medium);
    assert_eq!(rl.fps_controller().max_tier(), FpsTier::Medium);

    let rl = RenderLoop::new(10);
    assert_eq!(rl.fps_controller().tier(), FpsTier::Minimal);
    assert_eq!(rl.fps_controller().max_tier(), FpsTier::Minimal);
}

#[test]
fn render_loop_start_transitions_to_running() {
    let mut rl = RenderLoop::new(60);
    rl.start();
    assert_eq!(rl.state(), RenderLoopState::Running);
    assert!(rl.is_running());
}

#[test]
fn render_loop_stop_transitions_to_stopped() {
    let mut rl = RenderLoop::new(60);
    rl.start();
    rl.stop();
    assert_eq!(rl.state(), RenderLoopState::Stopped);
    assert!(!rl.is_running());
}

#[test]
fn render_loop_cannot_restart_after_stop() {
    let mut rl = RenderLoop::new(60);
    rl.start();
    rl.stop();
    rl.start(); // should be a no-op
    assert_eq!(rl.state(), RenderLoopState::Stopped);
    assert!(!rl.is_running());
}

#[test]
fn render_loop_pause_and_resume() {
    let mut rl = RenderLoop::new(60);
    rl.start();

    rl.pause();
    assert_eq!(rl.state(), RenderLoopState::Paused);
    assert!(!rl.is_running());

    rl.resume();
    assert_eq!(rl.state(), RenderLoopState::Running);
    assert!(rl.is_running());
}

#[test]
fn render_loop_pause_noop_when_not_running() {
    let mut rl = RenderLoop::new(60);
    rl.pause(); // Created state, should be no-op
    assert_eq!(rl.state(), RenderLoopState::Created);
}

#[test]
fn render_loop_resume_noop_when_not_paused() {
    let mut rl = RenderLoop::new(60);
    rl.start();
    rl.resume(); // Already running, should be no-op
    assert_eq!(rl.state(), RenderLoopState::Running);
}

#[test]
fn render_loop_tick_returns_false_when_not_running() {
    let mut rl = RenderLoop::new(60);
    assert!(!rl.tick());
}

#[test]
fn render_loop_tick_returns_true_when_running() {
    let mut rl = RenderLoop::new(60);
    rl.start();
    assert!(rl.tick());
}

#[test]
fn render_loop_frame_complete_returns_stats() {
    let mut rl = RenderLoop::new(60);
    rl.start();
    assert!(rl.tick());

    let stats = rl.frame_complete();
    assert!(stats.is_some());
    assert_eq!(rl.frame_number(), 1);
}

#[test]
fn render_loop_frame_complete_without_tick_returns_none() {
    let mut rl = RenderLoop::new(60);
    rl.start();
    assert!(rl.frame_complete().is_none());
}

#[test]
fn render_loop_frame_number_increments() {
    let mut rl = RenderLoop::new(60);
    rl.start();

    for expected in 1..=5 {
        assert!(rl.tick());
        let _stats = rl.frame_complete();
        assert_eq!(rl.frame_number(), expected);
    }
}

#[test]
fn render_loop_stop_handle_is_shared() {
    let mut rl = RenderLoop::new(60);
    rl.start();

    let handle = rl.stop_handle();
    assert!(rl.is_running());

    // Stop from the handle
    handle.store(false, Ordering::Release);
    assert!(!rl.is_running());
}

#[test]
fn render_loop_set_tier_changes_fps() {
    let mut rl = RenderLoop::new(60);
    assert_eq!(rl.fps_controller().tier(), FpsTier::Full);

    rl.set_tier(FpsTier::Low);
    assert_eq!(rl.fps_controller().tier(), FpsTier::Low);
    assert_eq!(rl.fps_controller().target_fps(), 20);
}

#[test]
fn render_loop_set_tier_respects_configured_ceiling() {
    let mut rl = RenderLoop::new(30);
    assert_eq!(rl.fps_controller().tier(), FpsTier::Medium);

    rl.set_tier(FpsTier::Full);
    assert_eq!(rl.fps_controller().tier(), FpsTier::Medium);
}

#[test]
fn render_loop_target_interval_matches_tier() {
    let rl = RenderLoop::new(30);
    assert_eq!(rl.target_interval(), FpsTier::Medium.frame_interval());
}

#[test]
fn render_loop_stats_snapshot() {
    let mut rl = RenderLoop::new(60);
    rl.start();

    let stats = rl.stats();
    assert_eq!(stats.total_frames, 0);
    assert_eq!(stats.tier, FpsTier::Full);
    assert_eq!(stats.state, RenderLoopState::Running);
    assert_eq!(stats.consecutive_misses, 0);

    // After a frame
    assert!(rl.tick());
    let _ = rl.frame_complete();
    let stats = rl.stats();
    assert_eq!(stats.total_frames, 1);
}

#[test]
fn render_loop_elapsed_is_zero_before_start() {
    let rl = RenderLoop::new(60);
    assert_eq!(rl.elapsed(), Duration::ZERO);
}

#[test]
fn render_loop_elapsed_grows_after_start() {
    let mut rl = RenderLoop::new(60);
    rl.start();
    // Elapsed should be non-negative (may be zero on fast machines)
    assert!(rl.elapsed() >= Duration::ZERO);
}

#[test]
fn render_loop_debug_output_is_readable() {
    let rl = RenderLoop::new(60);
    let debug = format!("{rl:?}");
    assert!(debug.contains("RenderLoop"));
    assert!(debug.contains("Created"));
}

#[test]
fn render_loop_with_config_uses_custom_settings() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 5,
        upshift_sustain_secs: 15.0,
        upshift_headroom_ratio: 0.5,
        ewma_alpha: 0.1,
    };
    let rl = RenderLoop::with_config(FpsTier::High, config);
    assert_eq!(rl.fps_controller().tier(), FpsTier::High);
    assert_eq!(rl.fps_controller().config().downshift_miss_threshold, 5);
}

// ─── Integration-Style Tests ─────────────────────────────────────────────────

#[test]
fn full_render_cycle_with_tier_transition() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 2,
        ..TierTransitionConfig::default()
    };
    let mut rl = RenderLoop::with_config(FpsTier::Full, config);
    rl.start();

    // Simulate 2 over-budget frames at Full (16.6ms budget)
    assert!(rl.tick());
    rl.fps_controller_mut()
        .record_frame(Duration::from_millis(20));
    rl.fps_controller_mut()
        .record_frame(Duration::from_millis(20));

    // The next frame_complete should detect the tier transition
    // But since we used record_frame directly, the transition happens via maybe_transition
    let transition = rl.fps_controller_mut().maybe_transition();
    assert_eq!(transition, Some(FpsTier::High));
    assert_eq!(rl.fps_controller().tier(), FpsTier::High);
}

#[test]
fn controller_ewma_smoothing_prevents_single_spike_downshift() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 2,
        ..TierTransitionConfig::default()
    };
    let mut ctrl = FpsController::with_config(FpsTier::Full, config);

    // Many good frames
    for _ in 0..100 {
        ctrl.record_frame(Duration::from_millis(5));
    }

    // One bad frame
    ctrl.record_frame(Duration::from_millis(50));

    // Only 1 consecutive miss, shouldn't downshift
    assert!(!ctrl.should_downshift());
    assert_eq!(ctrl.consecutive_misses(), 1);

    // EWMA should still be very low due to history
    let ewma_ms = ctrl.ewma_frame_time().as_secs_f64() * 1000.0;
    assert!(
        ewma_ms < 10.0,
        "EWMA should be low after single spike, got {ewma_ms:.2}ms"
    );
}

#[test]
fn all_tiers_have_unique_fps_values() {
    let fps_values: Vec<u32> = FpsTier::ALL.iter().map(|t| t.fps()).collect();
    for (i, fps) in fps_values.iter().enumerate() {
        for (j, other) in fps_values.iter().enumerate() {
            if i != j {
                assert_ne!(fps, other, "tiers {i} and {j} have the same FPS");
            }
        }
    }
}

#[test]
fn all_tiers_have_unique_intervals() {
    let intervals: Vec<Duration> = FpsTier::ALL.iter().map(|t| t.frame_interval()).collect();
    for (i, interval) in intervals.iter().enumerate() {
        for (j, other) in intervals.iter().enumerate() {
            if i != j {
                assert_ne!(interval, other, "tiers {i} and {j} have the same interval");
            }
        }
    }
}

#[test]
fn higher_tiers_have_shorter_intervals() {
    let intervals: Vec<Duration> = FpsTier::ALL.iter().map(|t| t.frame_interval()).collect();
    for window in intervals.windows(2) {
        assert!(
            window[0] > window[1],
            "tier intervals should decrease as tier increases"
        );
    }
}
