use std::{env, path::PathBuf, process::ExitCode};

#[cfg(target_os = "macos")]
use std::{fs::OpenOptions, io::Write, path::Path, process::Command};

#[cfg(target_os = "macos")]
use hypercolor_macos_input::{
    current_process_audit_token_identity, input_monitoring_granted, request_input_monitoring,
};

#[cfg(any(target_os = "macos", test))]
const MAX_TOOL_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_IDENTITY_FIELD_BYTES: usize = 8 * 1024;

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    safe fn CGPreflightScreenCaptureAccess() -> bool;
    safe fn CGRequestScreenCaptureAccess() -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Topology {
    AppSidecar,
    DirectLaunchd,
    Homebrew,
    Standalone,
}

impl Topology {
    #[cfg(target_os = "macos")]
    const fn as_str(self) -> &'static str {
        match self {
            Self::AppSidecar => "app_sidecar",
            Self::DirectLaunchd => "direct_launchd",
            Self::Homebrew => "homebrew",
            Self::Standalone => "standalone",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    topology: Topology,
    authorize_input: bool,
    authorize_screen: bool,
    prompt_text: Option<String>,
    system_settings_entry: Option<String>,
    output: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("probe_macos_tcc_owner: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let Some(args) = parse_args(env::args().skip(1))? else {
        print_help();
        return Ok(());
    };

    #[cfg(not(target_os = "macos"))]
    {
        let Args {
            topology,
            authorize_input,
            authorize_screen,
            prompt_text,
            system_settings_entry,
            output,
        } = args;
        let _ = (
            topology,
            authorize_input,
            authorize_screen,
            prompt_text,
            system_settings_entry,
            output,
        );
        Err("requires macOS 15.2 or newer".to_owned())
    }

    #[cfg(target_os = "macos")]
    {
        let executable = env::current_exe()
            .map_err(|error| format!("failed to resolve current executable: {error}"))?;
        let codesign = inspect_codesign(&executable)?;
        let input_before = input_monitoring_granted();
        let screen_before = CGPreflightScreenCaptureAccess();
        let input_request_result = args.authorize_input.then(request_input_monitoring);
        let screen_request_result = args
            .authorize_screen
            .then(|| CGRequestScreenCaptureAccess());
        let input_after = input_monitoring_granted();
        let screen_after = CGPreflightScreenCaptureAccess();
        let spctl = assess_notarization(&executable)?;

        let evidence = format!(
            "schema_version=1\n\
             topology={}\n\
             pid={}\n\
             audit_token={}\n\
             executable_path={}\n\
             executable_slice={}\n\
             host_architecture={}\n\
             translated_process={}\n\
             bundle_identifier={}\n\
             team_identifier={}\n\
             designated_requirement={}\n\
             codesign_valid={}\n\
             notarization_accepted={}\n\
             input_monitoring_before={}\n\
             input_monitoring_request={}\n\
             input_monitoring_after={}\n\
             screen_recording_before={}\n\
             screen_recording_request={}\n\
             screen_recording_after={}\n\
             prompt_text={}\n\
             system_settings_entry={}\n",
            args.topology.as_str(),
            std::process::id(),
            sanitize_field(
                &current_process_audit_token_identity().map_err(|error| error.to_string())?
            ),
            sanitize_field(&executable.display().to_string()),
            env::consts::ARCH,
            host_architecture()?,
            sysctl_flag("sysctl.proc_translated")?,
            sanitize_field(&codesign.identifier),
            sanitize_field(codesign.team_identifier.as_deref().unwrap_or("absent")),
            sanitize_field(&codesign.designated_requirement),
            codesign.valid,
            spctl,
            input_before,
            optional_bool(input_request_result),
            input_after,
            screen_before,
            optional_bool(screen_request_result),
            screen_after,
            sanitize_field(args.prompt_text.as_deref().unwrap_or("not_observed")),
            sanitize_field(
                args.system_settings_entry
                    .as_deref()
                    .unwrap_or("not_observed")
            ),
        );

        print!("{evidence}");
        if let Some(path) = args.output {
            eprintln!(
                "PRIVACY WARNING: writing signed process identity and TCC state to {}",
                path.display()
            );
            write_new(&path, evidence.as_bytes())?;
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct CodesignEvidence {
    identifier: String,
    team_identifier: Option<String>,
    designated_requirement: String,
    valid: bool,
}

#[cfg(target_os = "macos")]
fn inspect_codesign(executable: &Path) -> Result<CodesignEvidence, String> {
    let details = bounded_command(
        "/usr/bin/codesign",
        &["-d", "--verbose=4"],
        Some(executable),
    )?;
    let requirement = bounded_command("/usr/bin/codesign", &["-d", "-r-"], Some(executable))?;
    let verification = bounded_command(
        "/usr/bin/codesign",
        &["--verify", "--strict", "--verbose=4"],
        Some(executable),
    )?;
    let identifier = parse_value(&details.stderr, "Identifier=")?;
    let team_identifier = parse_optional_value(&details.stderr, "TeamIdentifier=")?;
    let designated_requirement = parse_designated_requirement(&requirement.stdout)?;
    Ok(CodesignEvidence {
        identifier,
        team_identifier,
        designated_requirement,
        valid: details.success && requirement.success && verification.success,
    })
}

#[cfg(target_os = "macos")]
fn assess_notarization(executable: &Path) -> Result<bool, String> {
    bounded_command(
        "/usr/sbin/spctl",
        &["--assess", "--type", "execute", "--verbose=4"],
        Some(executable),
    )
    .map(|output| output.success)
}

#[cfg(target_os = "macos")]
fn host_architecture() -> Result<&'static str, String> {
    if sysctl_flag("hw.optional.arm64")? || sysctl_flag("sysctl.proc_translated")? {
        Ok("apple_silicon")
    } else {
        Ok("intel")
    }
}

#[cfg(target_os = "macos")]
fn sysctl_flag(name: &str) -> Result<bool, String> {
    let output = bounded_command("/usr/sbin/sysctl", &["-in", name], None)?;
    parse_sysctl_flag(name, output.success, &output.stdout)
}

#[cfg(any(target_os = "macos", test))]
fn parse_sysctl_flag(name: &str, success: bool, stdout: &[u8]) -> Result<bool, String> {
    let value = std::str::from_utf8(stdout)
        .map_err(|_| format!("sysctl {name} returned non-UTF-8 output"))?
        .trim();
    if !success || value.is_empty() {
        return Ok(false);
    }
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("sysctl {name} returned unexpected value {value:?}")),
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct BoundedCommandOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[cfg(target_os = "macos")]
fn bounded_command(
    program: &str,
    args: &[&str],
    trailing_path: Option<&Path>,
) -> Result<BoundedCommandOutput, String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(path) = trailing_path {
        command.arg(path);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to execute {program}: {error}"))?;
    if output.stdout.len() > MAX_TOOL_OUTPUT_BYTES || output.stderr.len() > MAX_TOOL_OUTPUT_BYTES {
        return Err(format!("{program} output exceeds 16 KiB"));
    }
    Ok(BoundedCommandOutput {
        success: output.status.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[cfg(any(target_os = "macos", test))]
fn parse_designated_requirement(stdout: &[u8]) -> Result<String, String> {
    let stdout = bounded_utf8(stdout, "codesign designated requirement")?;
    let value = stdout.lines().find_map(|line| {
        line.strip_prefix("designated => ")
            .or_else(|| line.strip_prefix("# designated => "))
    });
    validate_identity_field(
        value.ok_or_else(|| "codesign omitted its designated requirement".to_owned())?,
        "designated requirement",
    )
}

#[cfg(any(target_os = "macos", test))]
fn parse_value(bytes: &[u8], prefix: &str) -> Result<String, String> {
    parse_optional_value(bytes, prefix)?.ok_or_else(|| format!("codesign omitted {prefix}"))
}

#[cfg(any(target_os = "macos", test))]
fn parse_optional_value(bytes: &[u8], prefix: &str) -> Result<Option<String>, String> {
    let text = bounded_utf8(bytes, "codesign details")?;
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(|value| validate_identity_field(value, prefix))
        .transpose()
}

#[cfg(any(target_os = "macos", test))]
fn bounded_utf8<'a>(bytes: &'a [u8], label: &str) -> Result<&'a str, String> {
    if bytes.len() > MAX_TOOL_OUTPUT_BYTES {
        return Err(format!("{label} exceeds 16 KiB"));
    }
    std::str::from_utf8(bytes).map_err(|_| format!("{label} is not UTF-8"))
}

fn validate_identity_field(value: &str, label: &str) -> Result<String, String> {
    if value.is_empty() || value.len() > MAX_IDENTITY_FIELD_BYTES {
        return Err(format!("{label} is empty or exceeds 8 KiB"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{label} contains control characters"));
    }
    Ok(value.to_owned())
}

#[cfg(target_os = "macos")]
fn sanitize_field(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn optional_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "granted",
        Some(false) => "denied",
        None => "not_requested",
    }
}

#[cfg(target_os = "macos")]
fn write_new(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let mut topology = None;
    let mut authorize_input = false;
    let mut authorize_screen = false;
    let mut prompt_text = None;
    let mut system_settings_entry = None;
    let mut output = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--topology" => {
                topology = Some(parse_topology(&next_arg(&mut args, "--topology")?)?);
            }
            "--authorize-input" => authorize_input = true,
            "--authorize-screen" => authorize_screen = true,
            "--prompt-text" => {
                prompt_text = Some(validate_identity_field(
                    &next_arg(&mut args, "--prompt-text")?,
                    "prompt text",
                )?);
            }
            "--system-settings-entry" => {
                system_settings_entry = Some(validate_identity_field(
                    &next_arg(&mut args, "--system-settings-entry")?,
                    "System Settings entry",
                )?);
            }
            "--output" => output = Some(PathBuf::from(next_arg(&mut args, "--output")?)),
            _ => return Err(format!("unknown argument {arg:?}; use --help")),
        }
    }
    Ok(Some(Args {
        topology: topology.ok_or_else(|| "--topology is required".to_owned())?,
        authorize_input,
        authorize_screen,
        prompt_text,
        system_settings_entry,
        output,
    }))
}

fn next_arg(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn parse_topology(value: &str) -> Result<Topology, String> {
    match value {
        "app-sidecar" => Ok(Topology::AppSidecar),
        "direct-launchd" => Ok(Topology::DirectLaunchd),
        "homebrew" => Ok(Topology::Homebrew),
        "standalone" => Ok(Topology::Standalone),
        _ => Err(format!(
            "unknown topology {value:?}; expected app-sidecar, direct-launchd, homebrew, or standalone"
        )),
    }
}

fn print_help() {
    println!(
        "probe_macos_tcc_owner --topology TOPOLOGY [--authorize-input] [--authorize-screen]\n\
         \x20                      [--prompt-text TEXT] [--system-settings-entry LABEL]\n\
         \x20                      [--output PATH]\n\
         \n\
         Records bounded current-process audit, code-signing, architecture,\n\
         notarization, and TCC evidence. No prompt appears unless an explicit\n\
         --authorize-input or --authorize-screen flag is present.\n\
         \n\
         TOPOLOGY is app-sidecar, direct-launchd, homebrew, or standalone.\n\
         --output creates a new file and never overwrites."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_is_required_and_closed() {
        assert!(parse_args([]).is_err());
        assert!(parse_args(["--topology".to_owned(), "future".to_owned()]).is_err());
        assert!(parse_args(["--topology".to_owned(), "capture-broker".to_owned()]).is_err());
        assert_eq!(
            parse_args(["--topology".to_owned(), "app-sidecar".to_owned()])
                .expect("arguments should parse")
                .expect("not help")
                .topology,
            Topology::AppSidecar
        );
    }

    #[test]
    fn authorization_is_explicit() {
        let args = parse_args([
            "--topology".to_owned(),
            "standalone".to_owned(),
            "--authorize-screen".to_owned(),
        ])
        .expect("arguments should parse")
        .expect("not help");
        assert!(!args.authorize_input);
        assert!(args.authorize_screen);
    }

    #[test]
    fn codesign_parsers_accept_signed_and_adhoc_shapes() {
        assert_eq!(
            parse_designated_requirement(b"designated => identifier \"tech.hyperbliss\"\n")
                .expect("signed requirement should parse"),
            "identifier \"tech.hyperbliss\""
        );
        assert_eq!(
            parse_designated_requirement(b"# designated => cdhash H\"0123\"\n")
                .expect("ad-hoc requirement should parse"),
            "cdhash H\"0123\""
        );
        assert_eq!(
            parse_value(b"Identifier=tech.hyperbliss.hypercolor\n", "Identifier=")
                .expect("identifier should parse"),
            "tech.hyperbliss.hypercolor"
        );
    }

    #[test]
    fn identity_fields_reject_control_characters_and_oversize() {
        assert!(validate_identity_field("line\nbreak", "field").is_err());
        assert!(
            validate_identity_field(&"x".repeat(MAX_IDENTITY_FIELD_BYTES + 1), "field").is_err()
        );
    }

    #[test]
    fn absent_architecture_flags_mean_false_on_intel() {
        assert!(!parse_sysctl_flag("hw.optional.arm64", true, b"").expect("empty OID"));
        assert!(!parse_sysctl_flag("sysctl.proc_translated", false, b"").expect("missing OID"));
        assert!(parse_sysctl_flag("hw.optional.arm64", true, b"1\n").expect("set flag"));
    }
}
