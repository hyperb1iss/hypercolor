//! What a slider's readout writes, what a typed entry means, and where a
//! value comes to rest.

use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::viewport::FitMode;
use hypercolor_ui::components::layer_panel::row::unshaped;
use hypercolor_ui::components::value_slider::{COARSE_STEPS, ValueFormat, ValueRange};

const ADJUST: ValueRange = ValueRange {
    min: 0.0,
    max: 4.0,
    step: 0.05,
};
const SCALE: ValueRange = ValueRange {
    min: 0.1,
    max: 4.0,
    step: 0.05,
};
const COUNT: ValueRange = ValueRange {
    min: 10.0,
    max: 120.0,
    step: 1.0,
};

fn close(left: f32, right: f32) -> bool {
    (left - right).abs() < 1e-4
}

#[test]
fn a_value_comes_to_rest_inside_the_range_and_on_a_step() {
    assert!(close(ADJUST.settle(1.13), 1.15));
    assert!(close(ADJUST.settle(1.12), 1.10));
    assert!(close(ADJUST.settle(9.0), 4.0));
    assert!(close(ADJUST.settle(-3.0), 0.0));
    // The grid starts at the minimum, not at zero.
    assert!(close(SCALE.settle(0.12), 0.10));
    assert!(close(SCALE.settle(1.0), 1.0));
    assert!(close(COUNT.settle(31.84), 32.0));
}

#[test]
fn a_range_with_no_step_only_clamps() {
    let free = ValueRange {
        min: 0.0,
        max: 1.0,
        step: 0.0,
    };
    assert!(close(free.settle(0.3337), 0.3337));
    assert!(close(free.settle(2.0), 1.0));
}

#[test]
fn stepping_moves_by_whole_steps_and_stops_at_the_ends() {
    assert!(close(ADJUST.stepped(1.0, 1), 1.05));
    assert!(close(ADJUST.stepped(1.0, -1), 0.95));
    assert!(close(ADJUST.stepped(1.0, COARSE_STEPS), 1.5));
    assert!(close(ADJUST.stepped(3.8, COARSE_STEPS), 4.0));
    assert!(close(ADJUST.stepped(0.2, -COARSE_STEPS), 0.0));
    assert!(close(COUNT.stepped(44.0, -1), 43.0));
}

#[test]
fn a_value_is_away_from_its_default_when_the_readout_says_so() {
    let format = ValueFormat::plain(2);
    assert!(!format.tells_apart(1.0, 1.0));
    assert!(!format.tells_apart(1.000_000_1, 1.0));
    assert!(!format.tells_apart(1.004, 1.0));
    assert!(format.tells_apart(1.01, 1.0));
    assert!(format.tells_apart(0.95, 1.0));
    assert!(!ValueFormat::percent().tells_apart(0.999, 1.0));
    assert!(ValueFormat::percent().tells_apart(0.99, 1.0));
}

#[test]
fn the_readout_matches_the_step() {
    assert_eq!(ValueFormat::for_step(1.0).write(44.0), "44");
    assert_eq!(ValueFormat::for_step(0.1).write(1.5), "1.5");
    assert_eq!(ValueFormat::for_step(0.05).write(1.15), "1.15");
    assert_eq!(ValueFormat::for_step(0.001).write(0.25), "0.250");
    assert_eq!(ValueFormat::plain(2).write(1.0), "1.00");
    assert_eq!(ValueFormat::percent().write(0.85), "85%");
    assert_eq!(
        ValueFormat::plain(2).suffixed("\u{d7}").write(1.5),
        "1.50\u{d7}"
    );
}

#[test]
fn the_entry_starts_with_the_number_alone() {
    assert_eq!(ValueFormat::percent().entry(0.85), "85");
    assert_eq!(ValueFormat::plain(2).suffixed("\u{d7}").entry(1.5), "1.50");
}

#[test]
fn a_typed_entry_is_read_with_or_without_its_unit() {
    let plain = ValueFormat::plain(2);
    assert!(close(plain.read("1.15").expect("a number"), 1.15));
    assert!(close(plain.read("  2 ").expect("a number"), 2.0));
    assert!(close(plain.read("1,5").expect("a number"), 1.5));
    assert!(close(plain.read("-0.5").expect("a number"), -0.5));

    let percent = ValueFormat::percent();
    assert!(close(percent.read("85").expect("a number"), 0.85));
    assert!(close(percent.read("85%").expect("a number"), 0.85));
    assert!(close(percent.read("85 %").expect("a number"), 0.85));

    let times = ValueFormat::plain(2).suffixed("\u{d7}");
    assert!(close(times.read("1.5\u{d7}").expect("a number"), 1.5));
}

#[test]
fn an_entry_that_is_not_a_number_is_refused() {
    let plain = ValueFormat::plain(2);
    for garbage in ["", "   ", "abc", "1.2.3", "--1", "1e", "NaN", "inf", "12px"] {
        assert_eq!(plain.read(garbage), None, "{garbage:?}");
    }
    assert_eq!(ValueFormat::percent().read("%"), None);
}

#[test]
fn a_typed_value_is_held_in_range_and_kept_as_fine_as_the_readout() {
    // Finer than the track's 0.05 steps: typing is how such a value is set.
    let plain = ValueFormat::plain(2);
    let typed = plain.read("1.137").expect("a number");
    assert!(close(plain.rounded(ADJUST.hold(typed)), 1.14));
    assert!(close(plain.rounded(ADJUST.hold(99.0)), 4.0));
    assert!(close(plain.rounded(ADJUST.hold(-1.0)), 0.0));

    // A whole-number control stays whole.
    let count = ValueFormat::for_step(COUNT.step);
    assert!(close(count.rounded(COUNT.hold(31.84)), 32.0));

    let percent = ValueFormat::percent();
    let typed = percent.read("250").expect("a number");
    let opacity = ValueRange {
        min: 0.0,
        max: 1.0,
        step: 0.01,
    };
    assert!(close(percent.rounded(opacity.hold(typed)), 1.0));
    assert!(close(percent.rounded(0.853), 0.85));
}

#[test]
fn resetting_the_disclosure_restores_what_it_shows_and_nothing_else() {
    let layer = SceneLayer {
        id: SceneLayerId::new(),
        name: Some("Wash".to_owned()),
        source: LayerSource::ColorFill {
            rgba: [0.2, 0.4, 0.6, 1.0],
        },
        blend: BlendMode::Screen,
        opacity: 0.4,
        transform: LayerTransform {
            scale: [1.5, 0.5],
            fit: FitMode::Contain,
            rotation: 0.3,
            ..LayerTransform::default()
        },
        adjust: LayerAdjust {
            brightness: 2.0,
            saturation: 1.15,
            tint_strength: 0.4,
            hue_shift: 12.0,
            contrast: 0.2,
            ..LayerAdjust::default()
        },
        bindings: Vec::new(),
        enabled: true,
        scope: None,
    };

    let reset = unshaped(layer.clone());

    assert!(close(reset.adjust.brightness, 1.0));
    assert!(close(reset.adjust.saturation, 1.0));
    assert!(close(reset.adjust.tint_strength, 0.0));
    assert_eq!(reset.transform.scale, [1.0, 1.0]);
    assert_eq!(reset.transform.fit, LayerTransform::default().fit);
    // Not shown in the disclosure, so not its to reset.
    assert!(close(reset.adjust.hue_shift, 12.0));
    assert!(close(reset.adjust.contrast, 0.2));
    assert!(close(reset.transform.rotation, 0.3));
    assert!(close(reset.opacity, 0.4));
    assert_eq!(reset.blend, BlendMode::Screen);
    assert_eq!(reset.id, layer.id);
    assert_eq!(unshaped(reset.clone()), reset);
}
