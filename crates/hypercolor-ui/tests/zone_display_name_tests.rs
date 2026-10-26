//! The one rule for the name a zone shows.

use hypercolor_types::scene::ZoneRole;
use hypercolor_ui::zones::surface::zone_display_name;

#[test]
fn an_unnamed_primary_zone_reads_as_main() {
    for stored in [
        "Primary",
        "primary",
        "PRIMARY",
        "  Primary  ",
        "",
        "   ",
        "Default zone",
        "default ZONE",
    ] {
        assert_eq!(
            zone_display_name(stored, ZoneRole::Primary),
            "Main",
            "{stored:?}"
        );
    }
}

#[test]
fn a_named_primary_zone_keeps_its_trimmed_name() {
    assert_eq!(
        zone_display_name("  Desk Strips ", ZoneRole::Primary),
        "Desk Strips"
    );
    assert_eq!(
        zone_display_name("Primary lights", ZoneRole::Primary),
        "Primary lights"
    );
}

#[test]
fn only_the_primary_zone_is_relabelled() {
    assert_eq!(zone_display_name("Primary", ZoneRole::Custom), "Primary");
    assert_eq!(
        zone_display_name(" Keyboard ", ZoneRole::Custom),
        "Keyboard"
    );
    assert_eq!(zone_display_name("", ZoneRole::Display), "");
    assert_eq!(
        zone_display_name("AIO Screen", ZoneRole::Display),
        "AIO Screen"
    );
}
