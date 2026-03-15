use hypercolor_core::device::hue::{GAMUT_A, GAMUT_C, rgb_to_cie_xyb};

#[test]
fn rgb_to_cie_xyb_returns_white_point_for_black() {
    let cie = rgb_to_cie_xyb(0, 0, 0, &GAMUT_C);

    assert!((cie.x - 0.3127).abs() < 0.0001);
    assert!((cie.y - 0.3290).abs() < 0.0001);
    assert!(cie.brightness.abs() < f64::EPSILON);
}

#[test]
fn rgb_to_cie_xyb_produces_in_gamut_red_for_modern_hue_lights() {
    let cie = rgb_to_cie_xyb(255, 0, 0, &GAMUT_C);

    assert!(cie.x >= 0.0 && cie.x <= 1.0);
    assert!(cie.y >= 0.0 && cie.y <= 1.0);
    assert!(cie.brightness > 0.0);
    assert!(cie.x > cie.y, "red should bias toward x chromaticity");
}

#[test]
fn rgb_to_cie_xyb_clamps_cyan_into_older_gamut() {
    let cie = rgb_to_cie_xyb(0, 255, 255, &GAMUT_A);

    assert!(cie.x >= 0.0 && cie.x <= 1.0);
    assert!(cie.y >= 0.0 && cie.y <= 1.0);
    assert!(cie.brightness > 0.0);
}
