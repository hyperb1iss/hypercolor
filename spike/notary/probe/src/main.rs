//! Evaluates a code requirement carrying the `notarized` clause against a
//! bundle, then against each executable inside it, in that order.
//!
//! Usage: notary-probe <spec|relaxed> <team-id> <bundle> <all|bundle|nested>
//!
//! `spec` uses the full flag set. `relaxed` drops kSecCSNoNetworkAccess so a
//! row can compare the two. Every target is also checked against the same
//! requirement without the clause, so a failure is attributable to it.
//!
//! Exit status: 0 when every target satisfies the requirement, 1 when every
//! target fails it and only the `notarized` clause is to blame, 3 otherwise.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use core_foundation::url::CFURL;
use security_framework::os::macos::code_signing::{Flags, SecRequirement, SecStaticCode};

const APP_IDENTIFIER: &str = "tech.hyperbliss.hypercolor.notary-spike";

struct Target {
    label: &'static str,
    path: PathBuf,
    identifier: String,
}

fn targets(bundle: &Path, scope: &str) -> Option<Vec<Target>> {
    let executable = |label, file: &str, suffix: &str| Target {
        label,
        path: bundle.join("Contents/MacOS").join(file),
        identifier: format!("{APP_IDENTIFIER}{suffix}"),
    };
    let whole = Target {
        label: "bundle",
        path: bundle.to_path_buf(),
        identifier: APP_IDENTIFIER.to_owned(),
    };
    let main = executable("main executable", "notary-spike", "");
    let sidecar = executable("sidecar", "notary-spike-sidecar", ".sidecar");
    let cli = executable("cli", "notary-spike-cli", ".cli");
    match scope {
        "all" => Some(vec![whole, main, sidecar, cli]),
        "bundle" => Some(vec![whole]),
        "nested" => Some(vec![sidecar, cli]),
        _ => None,
    }
}

fn requirement(identifier: &str, team: &str, notarized: bool) -> String {
    let base = format!(
        "identifier \"{identifier}\" and anchor apple generic \
         and certificate 1[field.1.2.840.113635.100.6.2.6] \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] \
         and certificate leaf[subject.OU] = \"{team}\""
    );
    if notarized {
        format!("{base} and notarized")
    } else {
        base
    }
}

fn check(path: &Path, text: &str, flags: Flags) -> Result<(), String> {
    let url = CFURL::from_path(path, path.is_dir()).ok_or("path has no URL form")?;
    let code = SecStaticCode::from_path(&url, Flags::NONE)
        .map_err(|error| format!("create {}: {error}", error.code()))?;
    let parsed: SecRequirement = text
        .parse()
        .map_err(|error: security_framework::base::Error| {
            format!("parse {}: {error}", error.code())
        })?;
    code.check_validity(flags, &parsed)
        .map_err(|error| format!("{}: {error}", error.code()))
}

fn resolved(path: &Path) -> String {
    CFURL::from_path(path, path.is_dir())
        .and_then(|url| SecStaticCode::from_path(&url, Flags::NONE).ok())
        .and_then(|code| code.path(Flags::NONE).ok())
        .and_then(|url| url.to_path())
        .map_or_else(|| "unresolved".to_owned(), |path| path.display().to_string())
}

fn verdict(result: &Result<(), String>) -> String {
    match result {
        Ok(()) => "PASS".to_owned(),
        Err(error) => format!("FAIL ({error})"),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, team, bundle, scope] = args.as_slice() else {
        eprintln!("usage: notary-probe <spec|relaxed> <team-id> <bundle> <all|bundle|nested>");
        return ExitCode::from(2);
    };
    let mut flags =
        Flags::STRICT_VALIDATE | Flags::CHECK_ALL_ARCHITECTURES | Flags::CHECK_NESTED_CODE;
    match mode.as_str() {
        "spec" => flags |= Flags::NO_NETWORK_ACCESS,
        "relaxed" => {}
        _ => {
            eprintln!("unknown mode {mode}");
            return ExitCode::from(2);
        }
    }
    let Some(targets) = targets(Path::new(bundle), scope) else {
        eprintln!("unknown scope {scope}");
        return ExitCode::from(2);
    };

    println!("flags: {:#x}", flags.bits());
    let (mut satisfied, mut clause_only, mut other) = (0, 0, 0);
    for target in &targets {
        let text = requirement(&target.identifier, team, true);
        let full = check(&target.path, &text, flags);
        let control = check(
            &target.path,
            &requirement(&target.identifier, team, false),
            flags,
        );
        println!("{}:", target.label);
        println!("  path: {}", target.path.display());
        println!("  resolved: {}", resolved(&target.path));
        println!("  requirement: {text}");
        println!("  with notarized: {}", verdict(&full));
        println!("  without notarized: {}", verdict(&control));
        match (full.is_ok(), control.is_ok()) {
            (true, true) => satisfied += 1,
            (false, true) => clause_only += 1,
            _ => other += 1,
        }
    }

    if satisfied == targets.len() {
        ExitCode::SUCCESS
    } else if clause_only == targets.len() {
        ExitCode::from(1)
    } else {
        println!("mixed: {satisfied} satisfied, {clause_only} clause only, {other} other");
        ExitCode::from(3)
    }
}
