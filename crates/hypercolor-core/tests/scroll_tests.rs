use hypercolor_core::input::{LegacyWheelProjector, Q16_16_SCALE, ScrollAggregate, q16_16_to_f64};
use hypercolor_types::event::PointerScrollUnit;

#[test]
fn legacy_projection_carries_signed_fractional_remainders() {
    let mut projector = LegacyWheelProjector::default();

    assert_eq!(projector.project(Q16_16_SCALE / 3), 0);
    assert_eq!(projector.project(Q16_16_SCALE / 3), 0);
    assert_eq!(projector.project(Q16_16_SCALE / 3 + 1), 1);
    assert_eq!(projector.remainder_q16_16(), 0);

    assert_eq!(projector.project(-Q16_16_SCALE / 2), 0);
    assert_eq!(projector.project(-Q16_16_SCALE / 2), -1);
    assert_eq!(projector.remainder_q16_16(), 0);
}

#[test]
fn legacy_projection_reset_discards_pre_gap_fraction() {
    let mut projector = LegacyWheelProjector::default();
    assert_eq!(projector.project(Q16_16_SCALE - 1), 0);
    projector.reset();
    assert_eq!(projector.project(1), 0);
}

#[test]
fn scroll_aggregate_keeps_units_and_axes_independent() {
    let mut aggregate = ScrollAggregate::default();
    aggregate.accumulate(PointerScrollUnit::Line120, 1, 2);
    aggregate.accumulate(PointerScrollUnit::Pixels, 3, 4);
    aggregate.absorb(ScrollAggregate {
        line120_x_q16_16: 5,
        line120_y_q16_16: 6,
        pixel_x_q16_16: 7,
        pixel_y_q16_16: 8,
    });

    assert_eq!(aggregate.line120_x_q16_16, 6);
    assert_eq!(aggregate.line120_y_q16_16, 8);
    assert_eq!(aggregate.pixel_x_q16_16, 10);
    assert_eq!(aggregate.pixel_y_q16_16, 12);
}

#[test]
fn q16_16_conversion_preserves_fractional_sign() {
    assert_eq!(q16_16_to_f64(Q16_16_SCALE / 2), 0.5);
    assert_eq!(q16_16_to_f64(-Q16_16_SCALE / 4), -0.25);
}
