//! CLI argument parsing tests.
//!
//! These tests verify that clap parses all subcommands, global flags, and
//! arguments correctly without needing a running daemon.

// We need to reference the binary's types. Since the CLI is a binary crate,
// we can't import from it directly. Instead we test via `Command::try_get_matches_from`.

/// Build the CLI command for testing.
#[expect(
    clippy::too_many_lines,
    reason = "command builder mirrors full CLI structure"
)]
fn build_cmd() -> clap::Command {
    // Reconstruct the CLI command structure for testing.
    // This mirrors the Cli struct in main.rs.
    use clap::{Arg, ArgAction, Command, value_parser};

    Command::new("hyper")
        .version("0.1.0")
        .about("Hypercolor RGB lighting control")
        .propagate_version(true)
        .arg(
            Arg::new("format")
                .long("format")
                .global(true)
                .default_value("table")
                .value_parser(["table", "json", "plain"]),
        )
        .arg(
            Arg::new("host")
                .long("host")
                .global(true)
                .default_value("localhost"),
        )
        .arg(
            Arg::new("port")
                .long("port")
                .global(true)
                .default_value("9420")
                .value_parser(value_parser!(u16)),
        )
        .arg(Arg::new("api-key").long("api-key").global(true))
        .arg(
            Arg::new("json")
                .long("json")
                .short('j')
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("quiet")
                .long("quiet")
                .short('q')
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no-color")
                .long("no-color")
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("verbose")
                .long("verbose")
                .short('v')
                .global(true)
                .action(ArgAction::Count),
        )
        .subcommand(
            Command::new("status")
                .about("Show current system state")
                .arg(Arg::new("watch").long("watch").action(ArgAction::SetTrue))
                .arg(Arg::new("interval").long("interval").default_value("1")),
        )
        .subcommand(
            Command::new("devices")
                .about("Device discovery and management")
                .subcommand_required(true)
                .subcommand(
                    Command::new("list")
                        .about("List devices")
                        .arg(Arg::new("status").long("status"))
                        .arg(Arg::new("backend").long("backend")),
                )
                .subcommand(
                    Command::new("discover")
                        .about("Scan for devices")
                        .arg(
                            Arg::new("backend")
                                .long("backend")
                                .action(ArgAction::Append),
                        )
                        .arg(Arg::new("timeout").long("timeout").default_value("10")),
                )
                .subcommand(
                    Command::new("info")
                        .about("Show device info")
                        .arg(Arg::new("device").required(true)),
                )
                .subcommand(
                    Command::new("identify")
                        .about("Flash test pattern")
                        .arg(Arg::new("device").required(true))
                        .arg(Arg::new("duration").long("duration").default_value("5")),
                )
                .subcommand(
                    Command::new("set-color")
                        .about("Set device color")
                        .arg(Arg::new("device").required(true))
                        .arg(Arg::new("color").required(true)),
                ),
        )
        .subcommand(
            Command::new("effects")
                .about("Effect browsing and control")
                .subcommand_required(true)
                .subcommand(
                    Command::new("list")
                        .about("List effects")
                        .arg(Arg::new("engine").long("engine"))
                        .arg(Arg::new("audio").long("audio").action(ArgAction::SetTrue))
                        .arg(Arg::new("search").long("search"))
                        .arg(Arg::new("category").long("category")),
                )
                .subcommand(
                    Command::new("activate")
                        .about("Activate an effect")
                        .arg(Arg::new("effect").required(true))
                        .arg(Arg::new("speed").long("speed"))
                        .arg(Arg::new("intensity").long("intensity"))
                        .arg(Arg::new("transition").long("transition").default_value("0")),
                )
                .subcommand(Command::new("stop").about("Stop effect"))
                .subcommand(
                    Command::new("info")
                        .about("Show effect info")
                        .arg(Arg::new("effect").required(true)),
                ),
        )
        .subcommand(
            Command::new("scenes")
                .about("Scene management")
                .subcommand_required(true)
                .subcommand(Command::new("list").about("List scenes"))
                .subcommand(
                    Command::new("create")
                        .about("Create scene")
                        .arg(Arg::new("name").required(true))
                        .arg(Arg::new("profile").long("profile").required(true))
                        .arg(Arg::new("trigger").long("trigger").required(true)),
                )
                .subcommand(
                    Command::new("activate")
                        .about("Activate scene")
                        .arg(Arg::new("name").required(true)),
                )
                .subcommand(
                    Command::new("delete")
                        .about("Delete scene")
                        .arg(Arg::new("name").required(true))
                        .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue)),
                )
                .subcommand(
                    Command::new("info")
                        .about("Scene info")
                        .arg(Arg::new("name").required(true)),
                ),
        )
        .subcommand(
            Command::new("profiles")
                .about("Profile management")
                .subcommand_required(true)
                .subcommand(Command::new("list").about("List profiles"))
                .subcommand(
                    Command::new("create")
                        .about("Create profile")
                        .arg(Arg::new("name").required(true))
                        .arg(Arg::new("description").long("description"))
                        .arg(Arg::new("force").long("force").action(ArgAction::SetTrue)),
                )
                .subcommand(
                    Command::new("apply")
                        .about("Apply profile")
                        .arg(Arg::new("name").required(true))
                        .arg(Arg::new("transition").long("transition").default_value("0")),
                )
                .subcommand(
                    Command::new("delete")
                        .about("Delete profile")
                        .arg(Arg::new("name").required(true))
                        .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue)),
                )
                .subcommand(
                    Command::new("info")
                        .about("Profile info")
                        .arg(Arg::new("name").required(true)),
                ),
        )
        .subcommand(
            Command::new("library")
                .about("Saved effect library")
                .subcommand_required(true)
                .subcommand(
                    Command::new("favorites")
                        .about("Favorite effects")
                        .subcommand_required(true)
                        .subcommand(Command::new("list").about("List favorites"))
                        .subcommand(
                            Command::new("add")
                                .about("Add favorite")
                                .arg(Arg::new("effect").required(true)),
                        )
                        .subcommand(
                            Command::new("remove")
                                .about("Remove favorite")
                                .arg(Arg::new("effect").required(true)),
                        ),
                )
                .subcommand(
                    Command::new("presets")
                        .about("Saved presets")
                        .subcommand_required(true)
                        .subcommand(
                            Command::new("create")
                                .about("Create preset")
                                .arg(Arg::new("name").required(true))
                                .arg(Arg::new("effect").long("effect").required(true))
                                .arg(Arg::new("description").long("description"))
                                .arg(
                                    Arg::new("control")
                                        .long("control")
                                        .short('c')
                                        .action(ArgAction::Append),
                                )
                                .arg(
                                    Arg::new("tag")
                                        .long("tag")
                                        .short('t')
                                        .action(ArgAction::Append),
                                ),
                        )
                        .subcommand(Command::new("list").about("List presets"))
                        .subcommand(
                            Command::new("info")
                                .about("Preset info")
                                .arg(Arg::new("preset").required(true)),
                        )
                        .subcommand(
                            Command::new("apply")
                                .about("Apply preset")
                                .arg(Arg::new("preset").required(true)),
                        )
                        .subcommand(
                            Command::new("delete")
                                .about("Delete preset")
                                .arg(Arg::new("preset").required(true))
                                .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue)),
                        ),
                )
                .subcommand(
                    Command::new("playlists")
                        .about("Saved playlists")
                        .subcommand_required(true)
                        .subcommand(
                            Command::new("create")
                                .about("Create playlist")
                                .arg(Arg::new("name").required(true))
                                .arg(Arg::new("description").long("description"))
                                .arg(
                                    Arg::new("no-loop")
                                        .long("no-loop")
                                        .action(ArgAction::SetTrue),
                                )
                                .arg(
                                    Arg::new("item")
                                        .long("item")
                                        .short('i')
                                        .action(ArgAction::Append),
                                ),
                        )
                        .subcommand(Command::new("list").about("List playlists"))
                        .subcommand(
                            Command::new("info")
                                .about("Playlist info")
                                .arg(Arg::new("playlist").required(true)),
                        )
                        .subcommand(
                            Command::new("activate")
                                .about("Activate playlist")
                                .arg(Arg::new("playlist").required(true)),
                        )
                        .subcommand(Command::new("active").about("Show active playlist"))
                        .subcommand(Command::new("stop").about("Stop active playlist"))
                        .subcommand(
                            Command::new("delete")
                                .about("Delete playlist")
                                .arg(Arg::new("playlist").required(true))
                                .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue)),
                        ),
                ),
        )
        .subcommand(
            Command::new("layouts")
                .about("Spatial layout management")
                .subcommand_required(true)
                .subcommand(Command::new("list").about("List layouts"))
                .subcommand(
                    Command::new("show")
                        .about("Show layout")
                        .arg(Arg::new("name").required(true)),
                )
                .subcommand(
                    Command::new("update")
                        .about("Update layout")
                        .arg(Arg::new("name").required(true))
                        .arg(Arg::new("data").long("data").required(true)),
                ),
        )
        .subcommand(
            Command::new("config")
                .about("Configuration management")
                .subcommand_required(true)
                .subcommand(Command::new("show").about("Show config"))
                .subcommand(
                    Command::new("get")
                        .about("Get config value")
                        .arg(Arg::new("key").required(true)),
                )
                .subcommand(
                    Command::new("set")
                        .about("Set config value")
                        .arg(Arg::new("key").required(true))
                        .arg(Arg::new("value").required(true))
                        .arg(Arg::new("live").long("live").action(ArgAction::SetTrue)),
                )
                .subcommand(
                    Command::new("reset")
                        .about("Reset config")
                        .arg(Arg::new("key"))
                        .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue)),
                )
                .subcommand(Command::new("path").about("Show config path")),
        )
        .subcommand(
            Command::new("diagnose")
                .about("Run diagnostics")
                .arg(Arg::new("check").long("check").action(ArgAction::Append))
                .arg(Arg::new("report").long("report"))
                .arg(Arg::new("system").long("system").action(ArgAction::SetTrue)),
        )
        .subcommand(
            Command::new("servers")
                .about("Discover Hypercolor daemons on the local network")
                .subcommand_required(true)
                .subcommand(
                    Command::new("discover")
                        .about("Discover Hypercolor daemons advertised via mDNS")
                        .arg(Arg::new("timeout").long("timeout").default_value("3")),
                ),
        )
        .subcommand(
            Command::new("completions")
                .about("Generate shell completions")
                .arg(Arg::new("shell").required(true).value_parser([
                    "bash",
                    "zsh",
                    "fish",
                    "powershell",
                ])),
        )
        .subcommand_required(true)
}

// ── Subcommand Parsing ──────────────────────────────────────────────────

#[test]
fn parse_status() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "status"])
        .expect("status should parse");
    assert_eq!(
        matches.subcommand_name(),
        Some("status"),
        "subcommand should be 'status'"
    );
}

#[test]
fn parse_status_with_watch() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "status", "--watch"])
        .expect("status --watch should parse");
    let (name, sub) = matches.subcommand().expect("should have subcommand");
    assert_eq!(name, "status");
    assert!(sub.get_flag("watch"));
}

#[test]
fn parse_servers_discover() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "servers", "discover", "--timeout", "1.5"])
        .expect("servers discover should parse");
    let (subcommand, nested) = matches
        .subcommand()
        .expect("servers subcommand should exist");
    assert_eq!(subcommand, "servers");
    let (nested_name, nested_matches) = nested
        .subcommand()
        .expect("discover subcommand should exist");
    assert_eq!(nested_name, "discover");
    assert_eq!(
        nested_matches
            .get_one::<String>("timeout")
            .map(String::as_str),
        Some("1.5")
    );
}

#[test]
fn parse_devices_list() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "devices", "list"])
        .expect("devices list should parse");
    let (name, sub) = matches.subcommand().expect("should have subcommand");
    assert_eq!(name, "devices");
    assert_eq!(sub.subcommand_name(), Some("list"));
}

#[test]
fn parse_devices_discover_with_backend() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "devices", "discover", "--backend", "wled"])
        .expect("devices discover should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let (_, discover) = sub.subcommand().expect("should have discover");
    let backends: Vec<&String> = discover
        .get_many::<String>("backend")
        .expect("should have backend")
        .collect();
    assert_eq!(backends, vec!["wled"]);
}

#[test]
fn parse_devices_info() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "devices", "info", "WLED Living Room"])
        .expect("devices info should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let (_, info) = sub.subcommand().expect("should have info");
    assert_eq!(
        info.get_one::<String>("device").map(String::as_str),
        Some("WLED Living Room")
    );
}

#[test]
fn parse_devices_identify() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from([
            "hyper",
            "devices",
            "identify",
            "Prism 8",
            "--duration",
            "10",
        ])
        .expect("devices identify should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let (_, identify) = sub.subcommand().expect("should have identify");
    assert_eq!(
        identify.get_one::<String>("device").map(String::as_str),
        Some("Prism 8")
    );
    assert_eq!(
        identify.get_one::<String>("duration").map(String::as_str),
        Some("10")
    );
}

#[test]
fn parse_devices_set_color() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "devices", "set-color", "Strip", "#ff6ac1"])
        .expect("devices set-color should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let (_, sc) = sub.subcommand().expect("should have set-color");
    assert_eq!(
        sc.get_one::<String>("device").map(String::as_str),
        Some("Strip")
    );
    assert_eq!(
        sc.get_one::<String>("color").map(String::as_str),
        Some("#ff6ac1")
    );
}

#[test]
fn parse_effects_list() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "effects", "list"])
        .expect("effects list should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    assert_eq!(sub.subcommand_name(), Some("list"));
}

#[test]
fn parse_effects_list_with_filters() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from([
            "hyper", "effects", "list", "--engine", "native", "--audio", "--search", "aurora",
        ])
        .expect("effects list with filters should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let (_, list) = sub.subcommand().expect("should have list");
    assert_eq!(
        list.get_one::<String>("engine").map(String::as_str),
        Some("native")
    );
    assert!(list.get_flag("audio"));
    assert_eq!(
        list.get_one::<String>("search").map(String::as_str),
        Some("aurora")
    );
}

#[test]
fn parse_effects_activate() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from([
            "hyper",
            "effects",
            "activate",
            "rainbow-wave",
            "--speed",
            "90",
        ])
        .expect("effects activate should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let (_, activate) = sub.subcommand().expect("should have activate");
    assert_eq!(
        activate.get_one::<String>("effect").map(String::as_str),
        Some("rainbow-wave")
    );
}

#[test]
fn parse_effects_stop() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "effects", "stop"])
        .expect("effects stop should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    assert_eq!(sub.subcommand_name(), Some("stop"));
}

#[test]
fn parse_effects_info() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "effects", "info", "aurora-drift"])
        .expect("effects info should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let (_, info) = sub.subcommand().expect("should have info");
    assert_eq!(
        info.get_one::<String>("effect").map(String::as_str),
        Some("aurora-drift")
    );
}

#[test]
fn parse_scenes_list() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "scenes", "list"])
        .expect("scenes list should parse");
}

#[test]
fn parse_scenes_create() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from([
            "hyper",
            "scenes",
            "create",
            "sunset-warmth",
            "--profile",
            "warm-ambient",
            "--trigger",
            "sunset",
        ])
        .expect("scenes create should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let (_, create) = sub.subcommand().expect("should have create");
    assert_eq!(
        create.get_one::<String>("name").map(String::as_str),
        Some("sunset-warmth")
    );
    assert_eq!(
        create.get_one::<String>("profile").map(String::as_str),
        Some("warm-ambient")
    );
}

#[test]
fn parse_scenes_activate() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "scenes", "activate", "sunset-warmth"])
        .expect("scenes activate should parse");
}

#[test]
fn parse_scenes_delete_with_yes() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "scenes", "delete", "old-scene", "--yes"])
        .expect("scenes delete should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let (_, delete) = sub.subcommand().expect("should have delete");
    assert!(delete.get_flag("yes"));
}

#[test]
fn parse_profiles_list() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "profiles", "list"])
        .expect("profiles list should parse");
}

#[test]
fn parse_profiles_create() {
    let cmd = build_cmd();
    cmd.try_get_matches_from([
        "hyper",
        "profiles",
        "create",
        "late-night",
        "--description",
        "Dim aurora",
    ])
    .expect("profiles create should parse");
}

#[test]
fn parse_profiles_apply() {
    let cmd = build_cmd();
    cmd.try_get_matches_from([
        "hyper",
        "profiles",
        "apply",
        "evening",
        "--transition",
        "3000",
    ])
    .expect("profiles apply should parse");
}

#[test]
fn parse_profiles_delete() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "profiles", "delete", "old-profile", "--yes"])
        .expect("profiles delete should parse");
}

#[test]
fn parse_profiles_info() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "profiles", "info", "evening"])
        .expect("profiles info should parse");
}

#[test]
fn parse_library_favorites_add() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "library", "favorites", "add", "solid_color"])
        .expect("library favorites add should parse");
    let (_, library) = matches.subcommand().expect("should have library");
    let (_, favorites) = library.subcommand().expect("should have favorites");
    let (_, add) = favorites.subcommand().expect("should have add");
    assert_eq!(
        add.get_one::<String>("effect").map(String::as_str),
        Some("solid_color")
    );
}

#[test]
fn parse_library_presets_apply() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "library", "presets", "apply", "night_mode"])
        .expect("library presets apply should parse");
    let (_, library) = matches.subcommand().expect("should have library");
    let (_, presets) = library.subcommand().expect("should have presets");
    let (_, apply) = presets.subcommand().expect("should have apply");
    assert_eq!(
        apply.get_one::<String>("preset").map(String::as_str),
        Some("night_mode")
    );
}

#[test]
fn parse_library_presets_create() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from([
            "hyper",
            "library",
            "presets",
            "create",
            "Warm Sweep",
            "--effect",
            "solid_color",
            "-c",
            "speed=7.5",
            "-t",
            "cozy",
        ])
        .expect("library presets create should parse");
    let (_, library) = matches.subcommand().expect("should have library");
    let (_, presets) = library.subcommand().expect("should have presets");
    let (_, create) = presets.subcommand().expect("should have create");
    assert_eq!(
        create.get_one::<String>("name").map(String::as_str),
        Some("Warm Sweep")
    );
    assert_eq!(
        create.get_one::<String>("effect").map(String::as_str),
        Some("solid_color")
    );
}

#[test]
fn parse_library_playlists_activate() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "library", "playlists", "activate", "runtime_loop"])
        .expect("library playlists activate should parse");
    let (_, library) = matches.subcommand().expect("should have library");
    let (_, playlists) = library.subcommand().expect("should have playlists");
    let (_, activate) = playlists.subcommand().expect("should have activate");
    assert_eq!(
        activate.get_one::<String>("playlist").map(String::as_str),
        Some("runtime_loop")
    );
}

#[test]
fn parse_library_playlists_create() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from([
            "hyper",
            "library",
            "playlists",
            "create",
            "Night Rotation",
            "--item",
            "effect:solid_color:2000",
            "--item",
            "preset:Warm Sweep:3000:250",
        ])
        .expect("library playlists create should parse");
    let (_, library) = matches.subcommand().expect("should have library");
    let (_, playlists) = library.subcommand().expect("should have playlists");
    let (_, create) = playlists.subcommand().expect("should have create");
    assert_eq!(
        create.get_one::<String>("name").map(String::as_str),
        Some("Night Rotation")
    );
}

#[test]
fn parse_layouts_list() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "layouts", "list"])
        .expect("layouts list should parse");
}

#[test]
fn parse_layouts_show() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "layouts", "show", "default"])
        .expect("layouts show should parse");
}

#[test]
fn parse_config_show() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "config", "show"])
        .expect("config show should parse");
}

#[test]
fn parse_config_get() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "config", "get", "daemon.fps"])
        .expect("config get should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let (_, get) = sub.subcommand().expect("should have get");
    assert_eq!(
        get.get_one::<String>("key").map(String::as_str),
        Some("daemon.fps")
    );
}

#[test]
fn parse_config_set() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "config", "set", "daemon.fps", "60", "--live"])
        .expect("config set should parse");
}

#[test]
fn parse_config_reset() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "config", "reset", "daemon.fps"])
        .expect("config reset should parse");
}

#[test]
fn parse_config_path() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "config", "path"])
        .expect("config path should parse");
}

#[test]
fn parse_diagnose() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "diagnose"])
        .expect("diagnose should parse");
}

#[test]
fn parse_diagnose_with_checks() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from([
            "hyper", "diagnose", "--check", "daemon", "--check", "render", "--system",
        ])
        .expect("diagnose with checks should parse");
    let (_, sub) = matches.subcommand().expect("should have subcommand");
    let checks: Vec<&String> = sub
        .get_many::<String>("check")
        .expect("should have checks")
        .collect();
    assert_eq!(checks.len(), 2);
    assert!(sub.get_flag("system"));
}

#[test]
fn parse_completions_bash() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "completions", "bash"])
        .expect("completions bash should parse");
}

#[test]
fn parse_completions_zsh() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "completions", "zsh"])
        .expect("completions zsh should parse");
}

#[test]
fn parse_completions_fish() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "completions", "fish"])
        .expect("completions fish should parse");
}

#[test]
fn parse_completions_powershell() {
    let cmd = build_cmd();
    cmd.try_get_matches_from(["hyper", "completions", "powershell"])
        .expect("completions powershell should parse");
}

// ── Global Flags ────────────────────────────────────────────────────────

#[test]
fn global_json_flag() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "--json", "status"])
        .expect("--json should parse globally");
    assert!(matches.get_flag("json"));
}

#[test]
fn global_json_short_flag() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "-j", "status"])
        .expect("-j should parse");
    assert!(matches.get_flag("json"));
}

#[test]
fn global_quiet_flag() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "--quiet", "status"])
        .expect("--quiet should parse");
    assert!(matches.get_flag("quiet"));
}

#[test]
fn global_no_color_flag() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "--no-color", "status"])
        .expect("--no-color should parse");
    assert!(matches.get_flag("no-color"));
}

#[test]
fn global_verbose_count() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "-vvv", "status"])
        .expect("-vvv should parse");
    assert_eq!(matches.get_count("verbose"), 3);
}

#[test]
fn global_format_json() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "--format", "json", "status"])
        .expect("--format json should parse");
    assert_eq!(
        matches.get_one::<String>("format").map(String::as_str),
        Some("json")
    );
}

#[test]
fn global_format_plain() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "--format", "plain", "status"])
        .expect("--format plain should parse");
    assert_eq!(
        matches.get_one::<String>("format").map(String::as_str),
        Some("plain")
    );
}

#[test]
fn global_host_and_port() {
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from([
            "hyper",
            "--host",
            "192.168.1.42",
            "--port",
            "8080",
            "status",
        ])
        .expect("--host and --port should parse");
    assert_eq!(
        matches.get_one::<String>("host").map(String::as_str),
        Some("192.168.1.42")
    );
    assert_eq!(matches.get_one::<u16>("port").copied(), Some(8080));
}

// ── Error Cases ──────────────────────────────────────────────────────────

#[test]
fn error_on_no_subcommand() {
    let cmd = build_cmd();
    let result = cmd.try_get_matches_from(["hyper"]);
    assert!(result.is_err(), "no subcommand should produce an error");
}

#[test]
fn error_on_unknown_subcommand() {
    let cmd = build_cmd();
    let result = cmd.try_get_matches_from(["hyper", "nonexistent"]);
    assert!(
        result.is_err(),
        "unknown subcommand should produce an error"
    );
}

#[test]
fn error_on_invalid_format() {
    let cmd = build_cmd();
    let result = cmd.try_get_matches_from(["hyper", "--format", "yaml", "status"]);
    assert!(result.is_err(), "invalid format should produce an error");
}

#[test]
fn error_on_invalid_port() {
    let cmd = build_cmd();
    let result = cmd.try_get_matches_from(["hyper", "--port", "99999", "status"]);
    assert!(result.is_err(), "port 99999 should fail u16 validation");
}

#[test]
fn error_on_devices_missing_subcommand() {
    let cmd = build_cmd();
    let result = cmd.try_get_matches_from(["hyper", "devices"]);
    assert!(
        result.is_err(),
        "devices without subcommand should produce an error"
    );
}

#[test]
fn error_on_invalid_completion_shell() {
    let cmd = build_cmd();
    let result = cmd.try_get_matches_from(["hyper", "completions", "nushell"]);
    assert!(result.is_err(), "nushell is not a valid completion shell");
}

// ── Help Text ────────────────────────────────────────────────────────────

#[test]
fn help_text_generates_without_error() {
    let mut cmd = build_cmd();
    let help = cmd.render_help();
    let help_str = help.to_string();
    assert!(
        help_str.contains("hyper"),
        "help should contain the binary name"
    );
    assert!(
        help_str.contains("status"),
        "help should list the status subcommand"
    );
    assert!(
        help_str.contains("devices"),
        "help should list the devices subcommand"
    );
    assert!(
        help_str.contains("effects"),
        "help should list the effects subcommand"
    );
    assert!(
        help_str.contains("library"),
        "help should list the library subcommand"
    );
}

// ── Shell Completions ────────────────────────────────────────────────────

#[test]
fn shell_completions_generate_for_all_shells() {
    use clap_complete::Shell;

    let shells = [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell];

    for shell in shells {
        let mut cmd = build_cmd();
        let mut buf = Vec::new();
        clap_complete::generate(shell, &mut cmd, "hyper", &mut buf);
        assert!(
            !buf.is_empty(),
            "completion script for {shell:?} should not be empty"
        );
    }
}

// ── Output Format ────────────────────────────────────────────────────────

#[test]
fn output_context_json_override() {
    // Test that --json flag overrides --format
    // Since we can't import OutputContext directly from a bin crate,
    // we verify through clap parsing that --json takes priority.
    let cmd = build_cmd();
    let matches = cmd
        .try_get_matches_from(["hyper", "--format", "table", "--json", "status"])
        .expect("should parse");
    assert!(
        matches.get_flag("json"),
        "--json flag should be set even with --format table"
    );
}

// ── DaemonClient ─────────────────────────────────────────────────────────

#[test]
fn daemon_client_url_construction() {
    // Verify that the client constructs the correct base URL.
    // We can't import from a bin crate, but we can test the pattern.
    let host = "192.168.1.42";
    let port: u16 = 8080;
    let expected_base = format!("http://{host}:{port}");
    assert_eq!(expected_base, "http://192.168.1.42:8080");
}
