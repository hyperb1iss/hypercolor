use std::path::Path;

use hypercolor_macos_owner::{
    LaunchdAdapter, MACOS_DIRECT_LAUNCHD_LABEL, launch_agent_plist, launchctl_service_disabled,
    parse_launchctl_print_pid,
};

#[test]
fn targets_address_the_users_gui_domain() {
    let adapter = LaunchdAdapter::with_uid("501");
    let target = adapter.target(MACOS_DIRECT_LAUNCHD_LABEL);
    assert_eq!(adapter.uid(), "501");
    assert_eq!(target.domain(), "gui/501");
    assert_eq!(target.service(), "gui/501/tech.hyperbliss.hypercolor");
    assert_eq!(target.label(), "tech.hyperbliss.hypercolor");
    assert_eq!(target.plist_file_name(), "tech.hyperbliss.hypercolor.plist");
    assert_eq!(
        launch_agent_plist(Path::new("/Users/u/Library/LaunchAgents"), &target),
        Path::new("/Users/u/Library/LaunchAgents/tech.hyperbliss.hypercolor.plist")
    );
}

#[test]
fn disabled_set_parser_matches_exact_labels() {
    let output = "disabled services = {\n\t\"tech.hyperbliss.hypercolor\" => true\n\t\"homebrew.mxcl.hypercolor\" => false\n}\n";
    assert!(launchctl_service_disabled(
        output,
        "tech.hyperbliss.hypercolor"
    ));
    assert!(!launchctl_service_disabled(
        output,
        "homebrew.mxcl.hypercolor"
    ));
    assert!(!launchctl_service_disabled(output, "Hypercolor"));
}

#[test]
fn print_pid_parser_reads_the_pid_line() {
    let output = "gui/501/tech.hyperbliss.hypercolor = {\n\tactive count = 1\n\tstate = running\n\tpid = 4242\n}\n";
    assert_eq!(parse_launchctl_print_pid(output), Some(4242));
    assert_eq!(parse_launchctl_print_pid("state = waiting\n"), None);
    assert_eq!(parse_launchctl_print_pid("pid = lots\n"), None);
}
