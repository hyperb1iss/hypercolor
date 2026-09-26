//! Principal lookups through the system NSS databases via `getent`.
//!
//! `getent` resolves users and groups through every configured NSS source in
//! a separate process, so this module needs no unsafe libc enumeration and
//! shares no global enumeration cursor with the rest of the process. The
//! databases are only trusted for enumeration when every configured source
//! can enumerate completely; directory services such as SSSD, LDAP, winbind
//! or NIS may hide principals from enumeration and are refused.

use std::fs;
use std::io::{self, Read as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{PrincipalDatabase, PrincipalGroup, PrincipalUser};

const GETENT_CANDIDATES: [&str; 2] = ["/usr/bin/getent", "/bin/getent"];
const TIMEOUT: &str = "/usr/bin/timeout";
const COMMAND_TIMEOUT: &str = "10s";
const MAX_GETENT_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;
const NSSWITCH_CANDIDATES: [&str; 2] = ["/etc/nsswitch.conf", "/usr/etc/nsswitch.conf"];
const MAX_NSSWITCH_BYTES: u64 = 256 * 1024;

/// Sources whose `getent` enumeration returns every record they can resolve.
const ENUMERABLE_SOURCES: [&str; 3] = ["files", "systemd", "altfiles"];

/// NSS-backed principal database used by ordinary Linux installs.
#[derive(Debug, Clone, Copy)]
pub(super) struct NssPrincipalDatabase;

impl PrincipalDatabase for NssPrincipalDatabase {
    fn user_by_uid(&self, uid: u32) -> io::Result<Option<PrincipalUser>> {
        let Some(bytes) = getent(&["passwd", &uid.to_string()])? else {
            return Ok(None);
        };
        let mut users = parse_passwd(&bytes)?;
        match (users.pop(), users.is_empty()) {
            (Some(user), true) => Ok(Some(user)),
            _ => Err(invalid(
                "keyed passwd lookup returned other than one record",
            )),
        }
    }

    fn group_by_gid(&self, gid: u32) -> io::Result<Option<PrincipalGroup>> {
        let Some(bytes) = getent(&["group", &gid.to_string()])? else {
            return Ok(None);
        };
        let mut groups = parse_group(&bytes)?;
        match (groups.pop(), groups.is_empty()) {
            (Some(group), true) => Ok(Some(group)),
            _ => Err(invalid("keyed group lookup returned other than one record")),
        }
    }

    fn all_users(&self) -> io::Result<Vec<PrincipalUser>> {
        require_enumerable_database("passwd")?;
        let bytes = getent(&["passwd"])?.ok_or_else(|| invalid("passwd enumeration failed"))?;
        parse_passwd(&bytes)
    }

    fn all_groups(&self) -> io::Result<Vec<PrincipalGroup>> {
        require_enumerable_database("group")?;
        // Login group membership comes from initgroups, which may consult a
        // directory service even when the group listing does not.
        require_enumerable_database("initgroups")?;
        let bytes = getent(&["group"])?.ok_or_else(|| invalid("group enumeration failed"))?;
        parse_group(&bytes)
    }
}

/// Run `getent` and return stdout, or `None` when a keyed entry is absent.
fn getent(args: &[&str]) -> io::Result<Option<Vec<u8>>> {
    let program = trusted_program(&GETENT_CANDIDATES)?;
    let timeout = trusted_program(&[TIMEOUT])?;
    let mut child = Command::new(timeout)
        .arg(COMMAND_TIMEOUT)
        .arg(program)
        .args(args)
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("getent stdout was not captured"))?
        .take(MAX_GETENT_OUTPUT_BYTES + 1)
        .read_to_end(&mut output)?;
    let status = child.wait()?;
    if output.len() as u64 > MAX_GETENT_OUTPUT_BYTES {
        return Err(invalid("getent output exceeds its byte bound"));
    }
    match status.code() {
        Some(0) => Ok(Some(output)),
        Some(2) if args.len() > 1 => Ok(None),
        Some(code) => Err(io::Error::other(format!(
            "getent {} exited with status {code}",
            args.join(" ")
        ))),
        None => Err(io::Error::other("getent was terminated by a signal")),
    }
}

/// Resolve the first present root-owned, non-writable system executable.
fn trusted_program(candidates: &[&str]) -> io::Result<PathBuf> {
    for candidate in candidates {
        let metadata = match fs::metadata(candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !metadata.is_file() || metadata.uid() != 0 || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(invalid(&format!(
                "{candidate} is not a root-owned, non-writable executable"
            )));
        }
        return Ok(Path::new(candidate).to_path_buf());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("none of {} is installed", candidates.join(", ")),
    ))
}

fn require_enumerable_database(database: &str) -> io::Result<()> {
    for candidate in NSSWITCH_CANDIDATES {
        let file = match fs::File::open(candidate) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let mut contents = String::new();
        file.take(MAX_NSSWITCH_BYTES + 1)
            .read_to_string(&mut contents)?;
        if contents.len() as u64 > MAX_NSSWITCH_BYTES {
            return Err(invalid("nsswitch.conf exceeds its byte bound"));
        }
        return enumerable_sources(&contents, database).map_err(|reason| invalid(&reason));
    }
    // Without any nsswitch.conf, glibc and musl resolve users from files only.
    Ok(())
}

/// Accept one database line only when every source enumerates completely.
///
/// Action blocks such as `[NOTFOUND=return]` or `[SUCCESS=merge]` are
/// skipped wherever they appear, including glued to a source name as in
/// `sss[NOTFOUND=return]`, which glibc accepts. A missing line means the libc
/// default of `files`. Repeated lines are ambiguous and refused.
pub(super) fn enumerable_sources(contents: &str, database: &str) -> Result<(), String> {
    let mut seen = false;
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some((key, sources)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != database {
            continue;
        }
        if seen {
            return Err(format!(
                "nsswitch.conf configures {database} more than once"
            ));
        }
        seen = true;
        for source in source_names(sources)
            .ok_or_else(|| format!("nsswitch.conf {database} line has unbalanced actions"))?
        {
            if !ENUMERABLE_SOURCES.contains(&source.as_str()) {
                return Err(format!(
                    "{database} source {source} cannot prove a complete enumeration"
                ));
            }
        }
    }
    Ok(())
}

/// Split a database line into service names, dropping bracketed actions.
fn source_names(sources: &str) -> Option<Vec<String>> {
    let mut names = Vec::new();
    let mut name = String::new();
    let mut depth = 0_usize;
    for character in sources.chars() {
        match character {
            '[' => depth += 1,
            ']' => depth = depth.checked_sub(1)?,
            _ if depth > 0 => {}
            character if character.is_whitespace() => {
                if !name.is_empty() {
                    names.push(std::mem::take(&mut name));
                }
            }
            character => name.push(character),
        }
        if depth > 0 && !name.is_empty() {
            names.push(std::mem::take(&mut name));
        }
    }
    if depth != 0 {
        return None;
    }
    if !name.is_empty() {
        names.push(name);
    }
    Some(names)
}

pub(super) fn parse_passwd(bytes: &[u8]) -> io::Result<Vec<PrincipalUser>> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("passwd output is not UTF-8"))?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split(':').collect();
            if fields.len() != 7 || fields[0].is_empty() {
                return Err(invalid("malformed passwd record"));
            }
            Ok(PrincipalUser {
                name: fields[0].to_owned(),
                uid: parse_id(fields[2])?,
                primary_gid: parse_id(fields[3])?,
            })
        })
        .collect()
}

pub(super) fn parse_group(bytes: &[u8]) -> io::Result<Vec<PrincipalGroup>> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("group output is not UTF-8"))?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split(':').collect();
            if fields.len() != 4 || fields[0].is_empty() {
                return Err(invalid("malformed group record"));
            }
            Ok(PrincipalGroup {
                name: fields[0].to_owned(),
                gid: parse_id(fields[2])?,
                members: fields[3]
                    .split(',')
                    .filter(|member| !member.is_empty())
                    .map(ToOwned::to_owned)
                    .collect(),
            })
        })
        .collect()
}

fn parse_id(field: &str) -> io::Result<u32> {
    if field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid("principal ID is not a decimal number"));
    }
    field
        .parse()
        .map_err(|_| invalid("principal ID exceeds 32 bits"))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}
