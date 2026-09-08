//! Guided setup CLI parsing and configuration preservation.

use clap::{CommandFactory, Parser};
use hypercolor_cli::commands::openrgb::{OpenRgbCommand, lifecycle_status_message, resize_patch};
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

#[test]
fn managed_lifecycle_commands_parse_and_owner_stays_hidden() {
    for verb in ["start", "stop"] {
        let cli = Cli::try_parse_from(["hypercolor", "openrgb", verb]).expect("lifecycle verb");
        assert!(matches!(cli.command, Commands::Openrgb(_)));
    }
    let cli = Cli::try_parse_from([
        "hypercolor",
        "openrgb-owner",
        "--data-dir",
        "/tmp/test-owner",
    ])
    .expect("hidden owner invocation");
    assert!(matches!(cli.command, Commands::OpenrgbOwner { .. }));
    assert!(
        !Cli::command()
            .render_long_help()
            .to_string()
            .contains("openrgb-owner")
    );
}

#[test]
fn lifecycle_output_distinguishes_starting_ready_and_unowned_stop() {
    assert_eq!(
        lifecycle_status_message(&serde_json::json!({"managed_pid": 42}), false),
        "OpenRGB startup is in progress"
    );
    assert_eq!(
        lifecycle_status_message(
            &serde_json::json!({"probe": {"reachable": true}, "adopted": true}),
            false
        ),
        "Using the existing OpenRGB SDK server"
    );
    assert_eq!(
        lifecycle_status_message(&serde_json::json!({"stopped": false}), true),
        "No Hypercolor-managed OpenRGB server was running"
    );
    assert_eq!(
        lifecycle_status_message(
            &serde_json::json!({"last_error": "Still enumerating"}),
            false
        ),
        "Still enumerating"
    );
}
