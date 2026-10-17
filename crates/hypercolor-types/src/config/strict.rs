//! Strict decoding for authoritative config sources.
//!
//! The config types skip keys they do not know so clients tolerate newer
//! daemons. Sources of truth decode through here instead, which collects
//! every key the derive skipped and refuses the document by dotted path.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

/// Decode one config section from an authoritative source, refusing any
/// key its type does not model.
///
/// The section-scoped twin of
/// [`HypercolorConfig::deserialize_strict`](super::HypercolorConfig::deserialize_strict):
/// unknown keys are named by their dotted path under `section`, so a
/// retired `monitor` key in a capture document reads `capture.monitor`.
///
/// # Errors
///
/// Returns the deserializer's own error when the value does not fit the
/// section type, and an error naming every unknown key when it fits but
/// carries keys this build does not read.
pub fn deserialize_section_strict<'de, T, D>(section: &str, deserializer: D) -> Result<T, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    deserialize_strict_under(Some(section), deserializer)
}

/// Strictly decode `T`, naming unknown keys under `prefix` when one is
/// given and from the document root otherwise.
pub(super) fn deserialize_strict_under<'de, T, D>(
    prefix: Option<&str>,
    deserializer: D,
) -> Result<T, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    let mut unknown = Vec::new();
    let value = serde_ignored::deserialize(deserializer, |path| {
        unknown.push(match prefix {
            Some(prefix) => format!("{prefix}.{path}"),
            None => path.to_string(),
        });
    })?;
    if unknown.is_empty() {
        Ok(value)
    } else {
        Err(D::Error::custom(unknown_keys_message(&unknown)))
    }
}

fn unknown_keys_message(keys: &[String]) -> String {
    let listed = keys
        .iter()
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let noun = if keys.len() == 1 { "key" } else { "keys" };
    format!("unknown config {noun} {listed}")
}
