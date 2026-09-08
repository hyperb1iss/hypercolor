//! Guided setup CLI parsing and configuration preservation.

use clap::Parser;
use hypercolor_cli::commands::openrgb::{OpenRgbCommand, resize_patch};
use hypercolor_cli::{Cli, Commands};

#[test]
fn resize_parses_explicit_device_zone_and_size() {
    let cli = Cli::try_parse_from([
        "hypercolor",
        "openrgb",
        "resize",
        "device-id",
        "Channel 1",
        "20",
    ])
    .expect("valid resize");
    let Commands::Openrgb(args) = cli.command else {
        panic!("OpenRGB command")
    };
    assert!(matches!(
        args.command,
        OpenRgbCommand::Resize { size: 20, .. }
    ));
    assert!(
        Cli::try_parse_from([
            "hypercolor",
            "openrgb",
            "resize",
            "device-id",
            "Channel 1",
            "-1"
        ])
        .is_err()
    );
}

#[test]
fn resize_patch_contains_only_the_requested_zone() {
    let fingerprint = "bridge:127.0.0.1:6742:serial:a";
    let patch = resize_patch(fingerprint, "Fan.1", 20).expect("resize patch");
    assert_eq!(
        serde_json::to_value(patch).expect("JSON"),
        serde_json::json!({fingerprint: {"Fan.1": 20}})
    );
    assert!(resize_patch("", "Fan", 20).is_err());
    assert!(resize_patch(fingerprint, "", 20).is_err());
}
