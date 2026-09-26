use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::sync::Arc;

use hypercolor_platform_fs::ExclusiveDirectory;

use super::{
    DirectoryRefusal, DirectoryRole, OwnershipPolicy, PrincipalDatabase, PrincipalGroup,
    PrincipalUser, prove_private_group,
};

#[derive(Debug, Clone)]
struct Database {
    users: Vec<PrincipalUser>,
    groups: Vec<PrincipalGroup>,
    enumerate_users: bool,
    enumerate_groups: bool,
    keyed_failure: bool,
}

impl Database {
    fn private(uid: u32, gid: u32) -> Self {
        Self {
            users: vec![
                user("root", 0, 0),
                user("installer", uid, gid),
                user("neighbor", uid + 1, gid + 1),
            ],
            groups: vec![
                group("root", 0, &[]),
                group("installer", gid, &[]),
                group("neighbor", gid + 1, &[]),
                group("wheel", 10, &["installer", "neighbor"]),
            ],
            enumerate_users: true,
            enumerate_groups: true,
            keyed_failure: false,
        }
    }
}

impl PrincipalDatabase for Database {
    fn user_by_uid(&self, uid: u32) -> io::Result<Option<PrincipalUser>> {
        if self.keyed_failure {
            return Err(io::Error::other("sssd is unreachable"));
        }
        Ok(self.users.iter().find(|entry| entry.uid == uid).cloned())
    }

    fn group_by_gid(&self, gid: u32) -> io::Result<Option<PrincipalGroup>> {
        if self.keyed_failure {
            return Err(io::Error::other("sssd is unreachable"));
        }
        Ok(self.groups.iter().find(|entry| entry.gid == gid).cloned())
    }

    fn all_users(&self) -> io::Result<Vec<PrincipalUser>> {
        if !self.enumerate_users {
            return Err(io::Error::other("passwd source sss cannot enumerate"));
        }
        Ok(self.users.clone())
    }

    fn all_groups(&self) -> io::Result<Vec<PrincipalGroup>> {
        if !self.enumerate_groups {
            return Err(io::Error::other("group source ldap cannot enumerate"));
        }
        Ok(self.groups.clone())
    }
}

fn user(name: &str, uid: u32, gid: u32) -> PrincipalUser {
    PrincipalUser {
        name: name.to_owned(),
        uid,
        primary_gid: gid,
    }
}

fn group(name: &str, gid: u32, members: &[&str]) -> PrincipalGroup {
    PrincipalGroup {
        name: name.to_owned(),
        gid,
        members: members.iter().map(|member| (*member).to_owned()).collect(),
    }
}

#[test]
fn private_group_requires_primary_sole_membership_and_complete_enumeration() {
    let (uid, gid) = (1000, 1000);
    assert_eq!(
        prove_private_group(&Database::private(uid, gid), uid, gid),
        Ok(())
    );
    let mut listed_self = Database::private(uid, gid);
    listed_self.groups[1].members = vec!["installer".to_owned()];
    assert_eq!(prove_private_group(&listed_self, uid, gid), Ok(()));

    let mut cases: Vec<(&str, Database, u32)> = Vec::new();
    let mut member = Database::private(uid, gid);
    member.groups[1].members = vec!["installer".to_owned(), "neighbor".to_owned()];
    cases.push(("second member", member, gid));
    let mut shared_primary = Database::private(uid, gid);
    shared_primary.users[2].primary_gid = gid;
    cases.push(("shared primary gid", shared_primary, gid));
    cases.push(("secondary group", Database::private(uid, gid), 10));
    let mut duplicate = Database::private(uid, gid);
    duplicate
        .groups
        .push(group("shadow-installer", gid, &["neighbor"]));
    cases.push(("duplicate gid entry", duplicate, gid));
    let mut lookup = Database::private(uid, gid);
    lookup.keyed_failure = true;
    cases.push(("keyed lookup failure", lookup, gid));
    let mut users = Database::private(uid, gid);
    users.enumerate_users = false;
    cases.push(("user enumeration failure", users, gid));
    let mut groups = Database::private(uid, gid);
    groups.enumerate_groups = false;
    cases.push(("group enumeration failure", groups, gid));
    let mut hidden = Database::private(uid, gid);
    hidden.users.retain(|entry| entry.uid != uid);
    hidden.users.push(user("impostor", uid + 7, gid + 7));
    cases.push(("absent user record", hidden, gid));
    let mut incomplete = Database::private(uid, gid);
    let installer = incomplete.users[1].clone();
    incomplete.users.retain(|entry| entry.uid != uid);
    let keyed_only = Database {
        users: vec![installer],
        ..incomplete.clone()
    };
    struct SplitView {
        keyed: Database,
        enumerated: Database,
    }
    impl std::fmt::Debug for SplitView {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("SplitView")
        }
    }
    impl PrincipalDatabase for SplitView {
        fn user_by_uid(&self, uid: u32) -> io::Result<Option<PrincipalUser>> {
            self.keyed.user_by_uid(uid)
        }
        fn group_by_gid(&self, gid: u32) -> io::Result<Option<PrincipalGroup>> {
            self.keyed.group_by_gid(gid)
        }
        fn all_users(&self) -> io::Result<Vec<PrincipalUser>> {
            self.enumerated.all_users()
        }
        fn all_groups(&self) -> io::Result<Vec<PrincipalGroup>> {
            self.enumerated.all_groups()
        }
    }
    let split = SplitView {
        keyed: keyed_only,
        enumerated: incomplete,
    };
    assert!(
        prove_private_group(&split, uid, gid)
            .expect_err("a directory-service user hidden from enumeration is unproven")
            .contains("does not include installer")
    );
    let mut groupless = Database::private(uid, gid);
    groupless.groups.retain(|entry| entry.gid != gid);
    cases.push(("absent group record", groupless, gid));

    for (label, database, gid) in cases {
        assert!(
            prove_private_group(&database, uid, gid).is_err(),
            "{label} was accepted as a private group"
        );
    }
}

struct Fixture {
    // One gate per fixture: re-acquiring a flock while sibling tests spawn
    // processes races with descriptors inherited until their exec.
    gate: ExclusiveDirectory,
    _root: tempfile::TempDir,
    directory: std::path::PathBuf,
    uid: u32,
    gid: u32,
}

impl Fixture {
    fn new(mode: u32) -> Self {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture root");
        let directory = root.path().join("ancestor");
        fs::create_dir(&directory).expect("directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(mode)).expect("mode");
        let metadata = fs::metadata(&directory).expect("metadata");
        let gate = ExclusiveDirectory::try_acquire(root.path(), std::path::Path::new("gate"))
            .expect("gate")
            .expect("uncontended gate");
        Self {
            gate,
            uid: metadata.uid(),
            gid: metadata.gid(),
            directory,
            _root: root,
        }
    }

    fn check(
        &self,
        policy: &OwnershipPolicy,
        role: DirectoryRole,
        acl: bool,
    ) -> Result<(), DirectoryRefusal> {
        let authority = self
            .gate
            .open_public_directory(&self.directory)
            .expect("authority");
        policy.require_writable_only_by_owner(authority.metadata().expect("metadata"), role, || {
            Ok(acl)
        })
    }
}

#[test]
fn group_write_is_accepted_only_for_private_ancestors() {
    let fixture = Fixture::new(0o775);
    let private =
        OwnershipPolicy::with_private_groups(Arc::new(Database::private(fixture.uid, fixture.gid)));
    assert_eq!(
        fixture.check(&private, DirectoryRole::Ancestor, false),
        Ok(())
    );
    assert_eq!(
        fixture.check(&private, DirectoryRole::InstallerOwned, false),
        Err(DirectoryRefusal::InstallerOwnedGroupWritable)
    );
    assert_eq!(
        fixture.check(&private, DirectoryRole::Ancestor, true),
        Err(DirectoryRefusal::ExtendedAcl)
    );
    assert!(matches!(
        fixture.check(&OwnershipPolicy::strict(), DirectoryRole::Ancestor, false),
        Err(DirectoryRefusal::SharedGroup { .. })
    ));

    let mut shared = Database::private(fixture.uid, fixture.gid);
    shared.groups[1].members = vec!["neighbor".to_owned()];
    let shared = OwnershipPolicy::with_private_groups(Arc::new(shared));
    assert!(matches!(
        fixture.check(&shared, DirectoryRole::Ancestor, false),
        Err(DirectoryRefusal::SharedGroup { reason, .. }) if reason.contains("neighbor")
    ));
}

#[test]
fn world_writable_sticky_and_owner_inaccessible_directories_always_fail() {
    for (mode, expected) in [
        (0o777, DirectoryRefusal::WorldWritable),
        (0o757, DirectoryRefusal::WorldWritable),
        (0o575, DirectoryRefusal::OwnerAccess),
    ] {
        let fixture = Fixture::new(mode);
        let private = OwnershipPolicy::with_private_groups(Arc::new(Database::private(
            fixture.uid,
            fixture.gid,
        )));
        assert_eq!(
            fixture.check(&private, DirectoryRole::Ancestor, false),
            Err(expected),
            "mode {mode:o}"
        );
        fs::set_permissions(&fixture.directory, fs::Permissions::from_mode(0o755))
            .expect("cleanup mode");
    }
    // Sticky and set-ID directories never become authorities at all: retained
    // traversal refuses any mode outside the ordinary permission bits.
    for mode in [0o1777, 0o1775, 0o2775] {
        let fixture = Fixture::new(mode);
        assert!(
            fixture
                .gate
                .open_public_directory(&fixture.directory)
                .is_err(),
            "mode {mode:o} became a directory authority"
        );
        fs::set_permissions(&fixture.directory, fs::Permissions::from_mode(0o755))
            .expect("cleanup mode");
    }
    let fixture = Fixture::new(0o755);
    assert_eq!(
        fixture.check(
            &OwnershipPolicy::strict(),
            DirectoryRole::InstallerOwned,
            true
        ),
        Ok(()),
        "an ACL cannot grant effective write through a non-writable group mask"
    );
}

#[test]
fn private_group_decision_is_cached_per_owner_and_group() {
    #[derive(Debug)]
    struct Counting(std::sync::atomic::AtomicUsize, Database);
    impl PrincipalDatabase for Counting {
        fn user_by_uid(&self, uid: u32) -> io::Result<Option<PrincipalUser>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.1.user_by_uid(uid)
        }
        fn group_by_gid(&self, gid: u32) -> io::Result<Option<PrincipalGroup>> {
            self.1.group_by_gid(gid)
        }
        fn all_users(&self) -> io::Result<Vec<PrincipalUser>> {
            self.1.all_users()
        }
        fn all_groups(&self) -> io::Result<Vec<PrincipalGroup>> {
            self.1.all_groups()
        }
    }
    let fixture = Fixture::new(0o775);
    let database = Arc::new(Counting(
        std::sync::atomic::AtomicUsize::new(0),
        Database::private(fixture.uid, fixture.gid),
    ));
    let policy = OwnershipPolicy::with_private_groups(database.clone());
    for _ in 0..3 {
        assert_eq!(
            fixture.check(&policy, DirectoryRole::Ancestor, false),
            Ok(())
        );
    }
    assert_eq!(database.0.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[cfg(target_os = "linux")]
mod nss {
    use super::super::nss::{NssPrincipalDatabase, enumerable_sources, parse_group, parse_passwd};
    use super::super::{PrincipalDatabase as _, PrincipalGroup, PrincipalUser};

    #[test]
    fn nsswitch_accepts_only_completely_enumerable_sources() {
        for accepted in [
            "",
            "# comment only\n",
            "passwd: files\ngroup: files\n",
            "passwd:         files systemd\ngroup:          files [SUCCESS=merge] systemd\n",
            "passwd: files altfiles systemd # fedora atomic\n",
            "hosts: files dns sss\npasswd: files\n",
            "passwd: files [ NOTFOUND=return ] systemd\n",
        ] {
            assert_eq!(
                enumerable_sources(accepted, "passwd"),
                Ok(()),
                "{accepted:?}"
            );
        }
        for refused in [
            "passwd: files sss\n",
            "passwd: sss files systemd\n",
            "passwd: files ldap\n",
            "passwd: compat\n",
            "passwd: files winbind\n",
            "passwd: files\npasswd: files\n",
            "passwd: files [NOTFOUND=return\n",
            "passwd: files ]\n",
        ] {
            assert!(
                enumerable_sources(refused, "passwd").is_err(),
                "{refused:?} was accepted"
            );
        }
        assert_eq!(enumerable_sources("passwd: files sss\n", "group"), Ok(()));
    }

    #[test]
    fn getent_records_parse_strictly() {
        assert_eq!(
            parse_passwd(
                b"root:x:0:0:root:/root:/bin/bash\nbliss:x:1000:1000::/home/bliss:/bin/zsh\n"
            )
            .expect("passwd"),
            vec![
                PrincipalUser {
                    name: "root".to_owned(),
                    uid: 0,
                    primary_gid: 0
                },
                PrincipalUser {
                    name: "bliss".to_owned(),
                    uid: 1000,
                    primary_gid: 1000
                },
            ]
        );
        assert_eq!(
            parse_group(b"wheel:x:10:bliss,alice\nbliss:x:1000:\n").expect("group"),
            vec![
                PrincipalGroup {
                    name: "wheel".to_owned(),
                    gid: 10,
                    members: vec!["bliss".to_owned(), "alice".to_owned()]
                },
                PrincipalGroup {
                    name: "bliss".to_owned(),
                    gid: 1000,
                    members: Vec::new()
                },
            ]
        );
        for malformed in [
            b"root:x:0:0:root:/root".as_slice(),
            b"root:x:-1:0:root:/root:/bin/sh",
            b"root:x:4294967296:0:root:/root:/bin/sh",
            b":x:0:0:root:/root:/bin/sh",
            b"root:x:0:0:root:/root:/bin/sh:extra",
            b"r\xffoot:x:0:0:root:/root:/bin/sh",
        ] {
            assert!(parse_passwd(malformed).is_err(), "{malformed:?}");
        }
        for malformed in [
            b"wheel:x:10".as_slice(),
            b"wheel:x:ten:bliss",
            b"wheel:x:10:bliss:extra",
        ] {
            assert!(parse_group(malformed).is_err(), "{malformed:?}");
        }
    }

    #[test]
    fn host_nss_resolves_the_current_user_or_reports_an_error() {
        let fixture = super::Fixture::new(0o755);
        match NssPrincipalDatabase.user_by_uid(fixture.uid) {
            Ok(Some(user)) => assert_eq!(user.uid, fixture.uid),
            Ok(None) => panic!("the running user must resolve through NSS"),
            Err(error) => eprintln!("host NSS lookup unavailable: {error}"),
        }
        assert!(
            NssPrincipalDatabase
                .user_by_uid(u32::MAX - 7)
                .map_or(true, |user| user.is_none()),
            "an unassigned uid resolved"
        );
    }
}
