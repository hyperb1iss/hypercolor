use std::path::Path;

use hypercolor_platform_fs::is_macos_translocated_path;

#[test]
fn translocation_is_recognized_only_under_the_per_user_temporary_root() {
    for translocated in [
        "/private/var/folders/ab/cdef/T/AppTranslocation/0F3A-11/d/Hypercolor.app",
        "/private/var/folders/ab/cdef/T/AppTranslocation/0F3A-11/d/Hypercolor.app/Contents/MacOS/hypercolor-app",
        "/var/folders/ab/cdef/T/AppTranslocation/0F3A-11/d/Hypercolor.app",
    ] {
        assert!(
            is_macos_translocated_path(Path::new(translocated)),
            "{translocated}"
        );
    }
    for ordinary in [
        "/Applications/Hypercolor.app",
        "/Users/player/Applications/Hypercolor.app",
        "/Users/player/AppTranslocation/Hypercolor.app",
        "/private/var/folders/ab/cdef/T/Hypercolor.app",
        "/Volumes/Hypercolor/Hypercolor.app",
    ] {
        assert!(
            !is_macos_translocated_path(Path::new(ordinary)),
            "{ordinary}"
        );
    }
}
