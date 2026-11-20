use hypercolor_ui::toasts::toast_expiry_ms;

#[test]
fn a_short_toast_keeps_the_base_time() {
    assert_eq!(toast_expiry_ms("Saved"), 2_500);
    assert_eq!(toast_expiry_ms(&"x".repeat(40)), 2_500);
}

#[test]
fn a_longer_toast_stays_up_long_enough_to_read() {
    let refusal = "Audio device wasn't changed. Capture settings can only be changed from the Hypercolor app.";
    let expiry = toast_expiry_ms(refusal);
    assert!(expiry > 4_500, "{expiry}");
    assert!(expiry < 8_000, "{expiry}");
}

#[test]
fn no_toast_outstays_the_ceiling() {
    assert_eq!(toast_expiry_ms(&"x".repeat(5_000)), 8_000);
}

#[test]
fn length_counts_characters_not_bytes() {
    assert_eq!(toast_expiry_ms(&"é".repeat(40)), 2_500);
}
