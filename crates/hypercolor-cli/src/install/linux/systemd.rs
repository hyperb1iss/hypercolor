use std::collections::BTreeMap;
use std::path::Path;

use super::super::InstallPlatformError;
use super::model::error;

const MAX_EXEC_WORDS: usize = 64;
const MAX_EXEC_TEXT_BYTES: usize = 8 * 1024;

pub(super) struct ParsedSystemdExec {
    pub(super) canonical_argv: String,
    pub(super) runtime_pid: u32,
}

pub(super) fn parse_systemd_exec(value: &str) -> Result<ParsedSystemdExec, InstallPlatformError> {
    if value.len() > MAX_EXEC_TEXT_BYTES || !value.starts_with('{') || !value.ends_with('}') {
        return Err(error(
            "systemd ExecStart is not one bounded structured command",
        ));
    }
    let inner = value[1..value.len() - 1].trim();
    if inner.contains('{') || inner.contains('}') {
        return Err(error("systemd ExecStart contains multiple commands"));
    }
    let mut fields = BTreeMap::new();
    for raw in inner.split(';') {
        let (name, field) = raw
            .trim()
            .split_once('=')
            .ok_or_else(|| error("systemd ExecStart field is malformed"))?;
        if !matches!(
            name,
            "path"
                | "argv[]"
                | "ignore_errors"
                | "start_time"
                | "stop_time"
                | "pid"
                | "code"
                | "status"
        ) {
            return Err(error("systemd ExecStart contains an unknown field"));
        }
        if fields.insert(name, field.trim()).is_some() {
            return Err(error("systemd ExecStart contains a duplicate field"));
        }
    }
    if fields.len() != 8 {
        return Err(error("systemd ExecStart is missing a required field"));
    }
    if fields["ignore_errors"] != "no"
        || fields["start_time"].is_empty()
        || fields["stop_time"].is_empty()
        || fields["code"].is_empty()
        || fields["status"].is_empty()
    {
        return Err(error("systemd ExecStart runtime fields are ambiguous"));
    }
    let path = parse_command_words(fields["path"])?;
    if path.len() != 1 || !Path::new(&path[0]).is_absolute() {
        return Err(error(
            "systemd ExecStart path is not one absolute executable",
        ));
    }
    let argv = parse_command_words(fields["argv[]"])?;
    if argv.first() != path.first() {
        return Err(error("systemd ExecStart executable and argv disagree"));
    }
    let runtime_pid = canonical_u32(fields["pid"], "systemd ExecStart pid")?;
    Ok(ParsedSystemdExec {
        canonical_argv: canonical_words(&argv)?,
        runtime_pid,
    })
}

pub(super) fn canonical_launcher_exec(value: &str) -> Result<String, InstallPlatformError> {
    let words = parse_command_words(value)?;
    if words.is_empty() || !Path::new(&words[0]).is_absolute() {
        return Err(error("launcher ExecStart lacks one absolute executable"));
    }
    canonical_words(&words)
}

pub(super) fn canonical_executable(canonical: &str) -> Result<String, InstallPlatformError> {
    let words: Vec<String> = serde_json::from_str(canonical)
        .map_err(|_| error("canonical ExecStart argument vector is malformed"))?;
    words
        .into_iter()
        .next()
        .filter(|executable| Path::new(executable).is_absolute())
        .ok_or_else(|| error("canonical ExecStart lacks an absolute executable"))
}

fn parse_command_words(value: &str) -> Result<Vec<String>, InstallPlatformError> {
    if value.is_empty() || value.len() > MAX_EXEC_TEXT_BYTES || value.contains('\0') {
        return Err(error(
            "ExecStart command text is empty, unbounded, or contains NUL",
        ));
    }
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for character in value.chars() {
        if escaped {
            if character.is_control() {
                return Err(error("ExecStart escape contains a control character"));
            }
            word.push(character);
            escaped = false;
            started = true;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            started = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else if character.is_control() {
                return Err(error("ExecStart quoted word contains a control character"));
            } else {
                word.push(character);
            }
            started = true;
            continue;
        }
        match character {
            '\'' | '"' => {
                quote = Some(character);
                started = true;
            }
            character if character.is_ascii_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                    if words.len() > MAX_EXEC_WORDS {
                        return Err(error("ExecStart has too many arguments"));
                    }
                }
            }
            character if character.is_control() => {
                return Err(error("ExecStart contains a control character"));
            }
            character => {
                word.push(character);
                started = true;
            }
        }
    }
    if escaped || quote.is_some() {
        return Err(error("ExecStart has an incomplete quote or escape"));
    }
    if started {
        words.push(word);
    }
    if words.is_empty() || words.len() > MAX_EXEC_WORDS || words.iter().any(String::is_empty) {
        return Err(error("ExecStart has an invalid argument vector"));
    }
    Ok(words)
}

fn canonical_words(words: &[String]) -> Result<String, InstallPlatformError> {
    serde_json::to_string(words).map_err(|source| error(source.to_string()))
}

fn canonical_u32(value: &str, description: &str) -> Result<u32, InstallPlatformError> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| error(format!("invalid canonical {description}")))?;
    if value != parsed.to_string() {
        return Err(error(format!("invalid canonical {description}")));
    }
    Ok(parsed)
}
