use hypercolor_types::display::{
    DISPLAY_DESCRIPTOR_API_VERSION, DisplayClass, DisplayDescriptor, DisplayPixelFormat,
    DisplayRect, DisplayShape,
};

fn derive(width: u32, height: u32, circular: bool) -> DisplayDescriptor {
    DisplayDescriptor::derive(width, height, circular, None, 30, DisplayPixelFormat::Rgb)
}

#[test]
fn round_480_derives_round_with_inscribed_safe_area() {
    let descriptor = derive(480, 480, true);

    assert_eq!(descriptor.api_version, DISPLAY_DESCRIPTOR_API_VERSION);
    assert_eq!(descriptor.shape, DisplayShape::Round);
    assert_eq!(descriptor.class, DisplayClass::PumpLcd);

    // Inscribed square: floor(480 / sqrt(2)) = 339, centered.
    assert_eq!(descriptor.safe_area, DisplayRect::new(70, 70, 339, 339));

    // The safe area must fit inside the circle: its corners lie on or
    // within the inscribed-square diagonal by construction.
    assert!(descriptor.safe_area.x + descriptor.safe_area.width <= 480);
    assert!(descriptor.safe_area.y + descriptor.safe_area.height <= 480);
}

#[test]
fn push2_strip_derives_wide_with_full_safe_area() {
    let descriptor = derive(960, 160, false);

    assert_eq!(descriptor.shape, DisplayShape::Wide);
    assert_eq!(descriptor.class, DisplayClass::Strip);
    assert_eq!(descriptor.safe_area, DisplayRect::full(960, 160));
}

#[test]
fn square_panel_derives_square() {
    let descriptor = derive(240, 240, false);

    assert_eq!(descriptor.shape, DisplayShape::Square);
    assert_eq!(descriptor.class, DisplayClass::Panel);
    assert_eq!(descriptor.safe_area, DisplayRect::full(240, 240));
}

#[test]
fn tall_panel_derives_tall() {
    let descriptor = derive(160, 960, false);

    assert_eq!(descriptor.shape, DisplayShape::Tall);
    assert_eq!(descriptor.class, DisplayClass::Strip);
    assert_eq!(descriptor.safe_area, DisplayRect::full(160, 960));
}

#[test]
fn aspect_boundaries_split_at_two_to_one() {
    assert_eq!(derive(480, 240, false).shape, DisplayShape::Wide);
    assert_eq!(derive(479, 240, false).shape, DisplayShape::Square);
    assert_eq!(derive(240, 480, false).shape, DisplayShape::Tall);
    assert_eq!(derive(240, 479, false).shape, DisplayShape::Square);
}

#[test]
fn class_hint_overrides_shape_default() {
    let descriptor = DisplayDescriptor::derive(
        480,
        480,
        true,
        Some(DisplayClass::Panel),
        30,
        DisplayPixelFormat::Rgb,
    );

    assert_eq!(descriptor.class, DisplayClass::Panel);
}

#[test]
fn zero_dimensions_are_clamped() {
    let descriptor = derive(0, 0, false);

    assert_eq!(descriptor.width, 1);
    assert_eq!(descriptor.height, 1);
    assert_eq!(descriptor.safe_area, DisplayRect::full(1, 1));
}

#[test]
fn descriptor_round_trips_through_json() {
    let descriptor = DisplayDescriptor::derive(
        960,
        160,
        false,
        Some(DisplayClass::Strip),
        60,
        DisplayPixelFormat::Yuv420,
    );

    let json = serde_json::to_string(&descriptor).expect("descriptor should serialize");
    let decoded: DisplayDescriptor =
        serde_json::from_str(&json).expect("descriptor should deserialize");

    assert_eq!(decoded, descriptor);
}

#[test]
fn enums_serialize_as_snake_case() {
    let shape = serde_json::to_value(DisplayShape::Round).expect("shape serializes");
    let class = serde_json::to_value(DisplayClass::PumpLcd).expect("class serializes");
    let format = serde_json::to_value(DisplayPixelFormat::Yuv420).expect("format serializes");

    assert_eq!(shape, "round");
    assert_eq!(class, "pump_lcd");
    assert_eq!(format, "yuv420");
}

#[test]
fn bootstrap_json_uses_frozen_camel_case_contract() {
    let descriptor = derive(960, 160, false);
    let value = descriptor.bootstrap_json();

    assert_eq!(value["apiVersion"], 1);
    assert_eq!(value["width"], 960);
    assert_eq!(value["height"], 160);
    assert_eq!(value["circular"], false);
    assert_eq!(value["shape"], "wide");
    assert_eq!(value["class"], "strip");
    assert_eq!(value["safeArea"]["x"], 0);
    assert_eq!(value["safeArea"]["width"], 960);
    assert_eq!(value["targetFps"], 30);
    assert_eq!(value["pixelFormat"], "rgb");
}

#[test]
fn bootstrap_json_class_tokens_are_kebab_case() {
    let round = derive(480, 480, true);

    assert_eq!(round.bootstrap_json()["class"], "pump-lcd");
}
