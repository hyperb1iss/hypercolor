use clap::CommandFactory;
use clap_complete::Shell;
use hypercolor_cli::Cli;

#[test]
fn derived_command_tree_generates_every_supported_completion_shell() {
    for shell in [
        Shell::Bash,
        Shell::Elvish,
        Shell::Fish,
        Shell::PowerShell,
        Shell::Zsh,
    ] {
        let mut command = Cli::command();
        let mut output = Vec::new();

        clap_complete::generate(shell, &mut command, "hypercolor", &mut output);

        assert!(
            !output.is_empty(),
            "completion script for {shell:?} should not be empty"
        );
        let output = String::from_utf8(output).expect("completion script should be UTF-8");
        assert!(
            !output.contains("__install-release"),
            "completion script for {shell:?} exposed the installer protocol"
        );
    }
}
