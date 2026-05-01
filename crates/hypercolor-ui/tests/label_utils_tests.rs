#[path = "../src/label_utils.rs"]
mod label_utils;

use label_utils::humanize_identifier_label;

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
