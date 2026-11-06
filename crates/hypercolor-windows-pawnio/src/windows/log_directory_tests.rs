use std::ffi::OsStr;
use std::fs;

use hypercolor_platform_fs::{
    WindowsAce, WindowsAceKind, WindowsDirectory, WindowsDirectoryAccess, WindowsSecurity,
    WindowsSid,
};

use super::{LOG_DIRECTORY_SDDL, Ownership, machine_log_directory, refusal};

const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
const USERS: &str = "S-1-5-32-545";
const CREATOR_OWNER: &str = "S-1-3-0";
const SOME_USER: &str = "S-1-5-21-1000-1000-1000-1001";
/// Read, list and traverse.
const READ_EXECUTE: u32 = 0x0012_00a9;
/// Add a file and add a subdirectory, as `%ProgramData%` grants Users.
const ADD_CHILDREN: u32 = 0x0000_0006;
const FULL_CONTROL: u32 = 0x001f_01ff;
const DELETE: u32 = 0x0001_0000;

fn sid(text: &str) -> WindowsSid {
    WindowsSid::parse(text).expect("SID")
}

fn ace(text: &str, mask: u32, applies_to_self: bool, inherits: bool) -> WindowsAce {
    WindowsAce {
        kind: WindowsAceKind::Allow,
        sid: sid(text),
        mask,
        applies_to_self,
        inherits_to_files: inherits,
        inherits_to_directories: inherits,
    }
}

fn security(owner: &str, aces: Vec<WindowsAce>) -> WindowsSecurity {
    WindowsSecurity {
        owner: sid(owner),
        null_dacl: false,
        dacl_protected: true,
        aces,
    }
}

fn locked_down() -> Vec<WindowsAce> {
    vec![
        ace(SYSTEM, FULL_CONTROL, true, true),
        ace(ADMINISTRATORS, FULL_CONTROL, true, true),
        ace(USERS, READ_EXECUTE, true, true),
    ]
}

#[test]
fn a_locked_down_directory_holds_logs_and_update_state() {
    for ownership in [Ownership::Shared, Ownership::Exclusive] {
        assert_eq!(
            refusal(&security(SYSTEM, locked_down()), ownership),
            None,
            "{ownership:?}"
        );
        assert_eq!(
            refusal(&security(ADMINISTRATORS, locked_down()), ownership),
            None
        );
    }
    let mut creator = locked_down();
    creator.push(ace(CREATOR_OWNER, FULL_CONTROL, false, true));
    assert_eq!(
        refusal(&security(SYSTEM, creator), Ownership::Exclusive),
        None,
        "CREATOR OWNER resolves to the trusted principal creating a child"
    );
}

#[test]
fn a_directory_a_standard_user_owns_or_can_change_is_refused() {
    for ownership in [Ownership::Shared, Ownership::Exclusive] {
        assert!(refusal(&security(SOME_USER, locked_down()), ownership).is_some());
        let mut deletable = locked_down();
        deletable.push(ace(USERS, DELETE, true, false));
        assert!(refusal(&security(SYSTEM, deletable), ownership).is_some());
        let mut null = security(SYSTEM, Vec::new());
        null.null_dacl = true;
        assert!(refusal(&null, ownership).is_some());
    }

    let mut adds = locked_down();
    adds.push(ace(USERS, ADD_CHILDREN, true, true));
    assert_eq!(
        refusal(&security(SYSTEM, adds.clone()), Ownership::Shared),
        None,
        "a shared parent may let users add children, as ProgramData does"
    );
    assert!(
        refusal(&security(SYSTEM, adds), Ownership::Exclusive).is_some(),
        "the log directory may not"
    );
    let mut passed_on = locked_down();
    passed_on.push(ace(SOME_USER, FULL_CONTROL, false, true));
    assert!(
        refusal(&security(SYSTEM, passed_on), Ownership::Exclusive).is_some(),
        "nor pass anyone else a right over what is created inside it"
    );
}

#[test]
fn the_created_directory_carries_a_descriptor_the_writers_accept() {
    let guard = tempfile::tempdir().expect("temporary directory");
    let root = fs::canonicalize(guard.path()).expect("canonical root");
    let root = std::path::PathBuf::from(
        root.to_str()
            .expect("UTF-8 path")
            .trim_start_matches(r"\\?\"),
    );
    let parent = WindowsDirectory::open(&root, WindowsDirectoryAccess::Write).expect("parent");
    let created = parent
        .create_child_directory_with_security(OsStr::new("logs"), LOG_DIRECTORY_SDDL)
        .expect("create with the log directory descriptor");
    let mut security = created.security().expect("security");
    assert!(security.dacl_protected, "nothing inherits from ProgramData");
    assert!(
        security.grants_read(&[sid(USERS)]),
        "standard users can read the logs"
    );
    // The test runs as whoever the runner is; the descriptor's entries are
    // what is under test.
    security.owner = sid(SYSTEM);
    assert_eq!(refusal(&security, Ownership::Exclusive), None);
    assert_eq!(refusal(&security, Ownership::Shared), None);
}

#[test]
fn the_machine_log_directory_is_under_program_data() {
    let program_data = std::env::var_os("ProgramData").expect("ProgramData is set on Windows");
    assert_eq!(
        machine_log_directory().expect("machine log directory"),
        std::path::Path::new(&program_data)
            .join("Hypercolor")
            .join("logs")
    );
}
