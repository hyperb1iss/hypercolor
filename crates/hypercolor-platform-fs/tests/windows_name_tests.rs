//! The Windows child-name rule holds on every host, so a manifest checked
//! on Linux refuses exactly the names the Windows directory authority does.

use hypercolor_platform_fs::windows_file_name_is_valid;

#[test]
fn names_that_win32_reads_differently_are_refused_on_every_host() {
    for name in [
        "",
        ".",
        "..",
        "a\\b",
        "a/b",
        "a:stream",
        "trailing.",
        "trailing ",
        "CON",
        "con.txt",
        "LPT1",
        "Com9.log",
        "star*",
        "quote\"",
        "pipe|",
        "angle<",
        "question?",
        "tab\t",
    ] {
        assert!(
            !windows_file_name_is_valid(name),
            "{name:?} must be refused"
        );
    }
    assert!(!windows_file_name_is_valid(&"a".repeat(256)));
}

#[test]
fn plain_names_are_accepted() {
    for name in [
        "hypercolor-app.exe",
        "vcruntime140_1.dll",
        "PawnIO_setup.exe",
        "index-3f2a.js",
        ".hidden",
        "CONSOLE",
        "COM10",
        "名前.html",
    ] {
        assert!(
            windows_file_name_is_valid(name),
            "{name:?} must be accepted"
        );
    }
    assert!(windows_file_name_is_valid(&"a".repeat(255)));
}
