//! Effect console reads for authoring tools.
//!
//! REST and MCP both read the engine's effect console journal through
//! [`read_console`], which applies the effect and severity filters and maps
//! each message back to a catalog id.

use std::path::{Path, PathBuf};

use hypercolor_core::effect::console::{EffectConsoleEntry as JournalEntry, EffectConsoleJournal};
use hypercolor_core::effect::resolve_html_source_path;
use hypercolor_types::api::effects::{
    EffectConsoleEntry, EffectConsoleLevel, EffectConsoleResponse,
};
use hypercolor_types::effect::{EffectId, EffectMetadata, EffectSource};

use super::EffectContext;
use crate::domain::DomainError;

/// Messages one read returns when the caller does not say.
pub const DEFAULT_CONSOLE_LIMIT: usize = 100;
/// Most messages one read may return.
pub const MAX_CONSOLE_LIMIT: usize = 500;

/// One read of the effect console.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsoleRead {
    /// Return only messages recorded after this cursor; `0` reads everything
    /// still retained.
    pub after: u64,
    /// Most messages to return, `1..=MAX_CONSOLE_LIMIT`.
    pub limit: usize,
    /// Effect id or name to narrow to.
    pub effect: Option<String>,
    /// Lowest severity to include.
    pub level: EffectConsoleLevel,
}

/// The journal cursor that skips everything recorded so far, for callers
/// that want only output produced after an action they are about to take.
#[must_use]
pub fn console_cursor(journal: &EffectConsoleJournal) -> u64 {
    journal.latest_seq()
}

/// Read effect console output after a cursor.
///
/// # Errors
///
/// Returns a validation error when `limit` is out of range.
pub async fn read_console(
    effects: &EffectContext,
    journal: &EffectConsoleJournal,
    read: ConsoleRead,
) -> Result<EffectConsoleResponse, DomainError> {
    if !(1..=MAX_CONSOLE_LIMIT).contains(&read.limit) {
        return Err(DomainError::validation_field(
            "limit",
            format!("limit must be between 1 and {MAX_CONSOLE_LIMIT}"),
        ));
    }

    let catalog = html_sources(&effects.all_metadata().await);
    let target = match read.effect.as_deref() {
        None => None,
        Some(selector) => Some(match effects.resolve_metadata(selector).await {
            Some(metadata) => EffectTarget::Effect(metadata.id),
            None => EffectTarget::Name(selector.to_owned()),
        }),
    };

    let page = journal.read(read.after, read.limit, |entry| {
        EffectConsoleLevel::of_label(&entry.level) >= read.level
            && target
                .as_ref()
                .is_none_or(|target| target.matches(entry, &catalog))
    });
    Ok(EffectConsoleResponse {
        entries: page
            .entries
            .into_iter()
            .map(|entry| wire_entry(entry, &catalog))
            .collect(),
        next_seq: page.next_seq,
        truncated: page.truncated,
    })
}

enum EffectTarget {
    /// An installed effect: match output from its source file only, so a
    /// copy that shares its title stays separate.
    Effect(EffectId),
    /// A selector the catalog does not know: match recorded names.
    Name(String),
}

impl EffectTarget {
    fn matches(&self, entry: &JournalEntry, catalog: &[(PathBuf, EffectId)]) -> bool {
        match self {
            Self::Effect(id) => catalog_id(&entry.source_path, catalog) == Some(*id),
            Self::Name(name) => entry.effect_name.eq_ignore_ascii_case(name),
        }
    }
}

/// Resolved source paths of every HTML effect in the catalog.
fn html_sources(catalog: &[EffectMetadata]) -> Vec<(PathBuf, EffectId)> {
    catalog
        .iter()
        .filter_map(|metadata| match &metadata.source {
            EffectSource::Html { path } => resolve_html_source_path(path)
                .ok()
                .map(|resolved| (resolved, metadata.id)),
            EffectSource::Native { .. } | EffectSource::Shader { .. } => None,
        })
        .collect()
}

fn catalog_id(source_path: &Path, catalog: &[(PathBuf, EffectId)]) -> Option<EffectId> {
    catalog
        .iter()
        .find(|(path, _)| path == source_path)
        .map(|(_, id)| *id)
}

fn wire_entry(entry: JournalEntry, catalog: &[(PathBuf, EffectId)]) -> EffectConsoleEntry {
    EffectConsoleEntry {
        seq: entry.seq,
        timestamp_ms: entry.timestamp_ms,
        effect_id: catalog_id(&entry.source_path, catalog).map(|id| id.to_string()),
        source_file: entry.source_path.file_name().map_or_else(
            || entry.source_path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        level: entry.level,
        message: entry.message,
        effect_name: entry.effect_name,
        line: entry.line.and_then(|line| u32::try_from(line).ok()),
    }
}
