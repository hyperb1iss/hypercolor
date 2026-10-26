use std::cmp::Ordering;

use hypercolor_ui::label_utils::{humanize_identifier_label, natural_cmp};

#[test]
fn humanizes_identifier_labels_without_shouting_short_words() {
    let cases = [
        ("open-link-hub", "Open Link Hub"),
        ("hue--api", "Hue API"),
        ("USB-BRIDGE", "USB Bridge"),
        ("open_link-hub", "Open Link Hub"),
        ("spi_device", "SPI Device"),
        ("", ""),
    ];

    for (input, expected) in cases {
        assert_eq!(humanize_identifier_label(input), expected);
    }
}

#[test]
fn numbered_labels_sort_by_value() {
    let mut labels = vec!["TL LCD 3", "TL LCD 10", "TL LCD 1", "TL LCD 2"];
    labels.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(labels, ["TL LCD 1", "TL LCD 2", "TL LCD 3", "TL LCD 10"]);
}

#[test]
fn natural_order_ignores_case_and_compares_text_before_numbers() {
    let mut labels = vec!["corsair Pump LCD", "TL LCD 2", "Corsair AIO", "tl lcd 1"];
    labels.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(
        labels,
        ["Corsair AIO", "corsair Pump LCD", "tl lcd 1", "TL LCD 2"]
    );
}

#[test]
fn natural_order_is_total_for_labels_that_read_alike() {
    assert_eq!(natural_cmp("LCD 2", "LCD 02"), "LCD 2".cmp("LCD 02"));
    assert_ne!(natural_cmp("lcd", "LCD"), Ordering::Equal);
    assert_eq!(natural_cmp("LCD", "LCD"), Ordering::Equal);
    assert_eq!(natural_cmp("LCD", "LCD 1"), Ordering::Less);
    assert_eq!(
        natural_cmp("panel 99999999999999999999", "panel 100000000000000000000"),
        Ordering::Less
    );
}
