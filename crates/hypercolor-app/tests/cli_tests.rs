use hypercolor_app::cli::AppArgs;

#[test]
fn parse_empty_args_uses_defaults() {
    assert_eq!(
        AppArgs::parse(std::iter::empty::<&str>()),
        AppArgs::default()
    );
}

#[test]
fn parse_lifecycle_flags() {
    let args = AppArgs::parse(["hypercolor-app", "--minimized", "--show", "--quit"]);

    assert!(args.start_minimized);
    assert!(args.show);
    assert!(args.quit);
}

#[test]
fn parse_hidden_alias_as_minimized() {
    let args = AppArgs::parse(["--hidden"]);

    assert!(args.start_minimized);
}

#[test]
fn parse_ignores_unknown_forwarded_args() {
    let args = AppArgs::parse(["--tauri-runtime-flag", "--show"]);

    assert_eq!(
        args,
        AppArgs {
            start_minimized: false,
            show: true,
            quit: false,
        }
    );
}
