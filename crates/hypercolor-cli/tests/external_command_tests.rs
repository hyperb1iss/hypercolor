use clap::Parser;
use hypercolor_cli::{Cli, Commands};

#[test]
fn cli_captures_external_subcommands_for_internal_dispatch() {
    let cli = Cli::try_parse_from(["hypercolor", "studio", "open", "--mode", "full"])
        .expect("external subcommand should parse");

    let Commands::External(args) = cli.command else {
        panic!("expected external command capture");
    };

    assert_eq!(args, ["studio", "open", "--mode", "full"]);
}
