//! Previous-good generations of the daemon's durable state files.
//!
//! Every daemon-owned store that holds user state keeps its last few
//! versions under `$XDG_STATE_HOME/hypercolor/history/<store>/`, so a bad
//! write (a stray API request, a schema drift that loads as empty) can be
//! rolled back. Which stores are covered derives from
//! [`DURABLE_STORES`]: every daemon-owned single-file store is covered unless
//! [`HISTORY_EXCLUSIONS`] names it with a reason.
//!
//! The mechanics (when a copy is taken, the byte-identical and transient
//! skips, pruning, crash ordering) live in `hypercolor-persistence`; this
//! module maps stores to paths, turns history on at startup, and implements
//! the offline `hypercolor-daemon history` command:
//!
//! ```text
//! hypercolor-daemon history list [STORE] [--json]
//! hypercolor-daemon history restore STORE GENERATION
//! ```
//!
//! `restore` refuses to run while a daemon holds the single-instance guard,
//! because a running daemon would overwrite the restored file from memory.
//! The restore is itself a write through the store's history, so the content
//! it replaces becomes a new generation and the restore can be undone.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::persistence::{
    AtomicFileWriter, Equivalence, Generation, HistoryPolicy, list_generations, restore_generation,
};
use hypercolor_types::config::DaemonConfig;
use serde::Serialize;

use crate::durable_stores::{DURABLE_STORES, StoreLocation, StoreOwner, StoreRoot};

/// Directory under the state root that holds every store's generations.
pub const HISTORY_DIR: &str = "history";

/// Daemon-owned stores whose previous versions are deliberately not kept.
pub const HISTORY_EXCLUSIONS: &[(&str, &str)] = &[
    (
        "instance-id",
        "written once; an older identity would split the daemon's identity",
    ),
    (
        "asset-library",
        "an index rebuilt from the content-addressed objects it describes",
    ),
    (
        "user-effects",
        "a directory of user-authored files the daemon never rewrites",
    ),
    (
        "legacy-profiles",
        "read once and renamed aside; never written",
    ),
    (
        "driver-inventory",
        "a rebuildable discovery cache rewritten by background scans",
    ),
    (
        "credentials",
        "secret material; old generations would keep revoked secrets on disk",
    ),
    (
        "attachment-templates",
        "a directory of individually written template files",
    ),
    (
        "device-binding-journal",
        "a crash-recovery journal for an identity migration, not user state",
    ),
];

/// One daemon-owned store file and where it lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreFile {
    /// Inventory name, shared with `durable-stores.json`.
    pub name: &'static str,
    /// The file the store writes.
    pub path: PathBuf,
}

/// The roots the daemon resolves store paths against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRoots {
    /// The configuration file this daemon loads and saves.
    pub config_file: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl StoreRoots {
    /// The platform roots, with `config_file` as the loaded configuration.
    #[must_use]
    pub fn resolve(config_file: PathBuf) -> Self {
        Self {
            config_file,
            config_dir: ConfigManager::config_dir(),
            data_dir: ConfigManager::data_dir(),
            state_dir: ConfigManager::state_dir(),
        }
    }

    fn root(&self, root: StoreRoot) -> &Path {
        match root {
            StoreRoot::Config => &self.config_dir,
            StoreRoot::Data => &self.data_dir,
            StoreRoot::State => &self.state_dir,
        }
    }
}

/// Every daemon-owned store that is a single file, with its path.
#[must_use]
pub fn daemon_store_files(roots: &StoreRoots) -> Vec<StoreFile> {
    DURABLE_STORES
        .iter()
        .filter(|store| store.owner == StoreOwner::Daemon)
        .filter_map(|store| {
            let path = match store.location {
                StoreLocation::File(name) => roots.root(store.root).join(name),
                StoreLocation::ConfigFile => roots.config_file.clone(),
                StoreLocation::Directory(_) => return None,
            };
            Some(StoreFile {
                name: store.name,
                path,
            })
        })
        .collect()
}

/// The stores whose previous generations are kept.
#[must_use]
pub fn history_stores(roots: &StoreRoots) -> Vec<StoreFile> {
    daemon_store_files(roots)
        .into_iter()
        .filter(|store| !is_excluded(store.name))
        .collect()
}

/// Where `store` keeps its generations.
#[must_use]
pub fn history_directory(state_dir: &Path, store: &str) -> PathBuf {
    state_dir.join(HISTORY_DIR).join(store)
}

/// The history policy the daemon's configuration asks for.
#[must_use]
pub fn policy_settings(config: &DaemonConfig) -> (usize, Duration) {
    (
        usize::try_from(config.state_history_generations).unwrap_or(usize::MAX),
        Duration::from_secs(config.state_history_min_interval_secs),
    )
}

/// Keep previous generations for every covered store, or for none when
/// `generations` is zero.
///
/// Returns the stores now covered. A store whose directory cannot be
/// prepared is logged and left without history; the store itself reports
/// the same failure when it opens.
pub fn enable(roots: &StoreRoots, generations: usize, min_interval: Duration) -> Vec<StoreFile> {
    let mut covered = Vec::new();
    for store in history_stores(roots) {
        let writer = match AtomicFileWriter::new(&store.path) {
            Ok(writer) => writer,
            Err(error) => {
                tracing::warn!(
                    store = store.name,
                    path = %store.path.display(),
                    %error,
                    "State history unavailable for this store"
                );
                continue;
            }
        };
        let mut policy = HistoryPolicy::new(
            history_directory(&roots.state_dir, store.name),
            generations,
            min_interval,
        );
        if let Some(equivalent) = equivalence(store.name) {
            policy = policy.with_equivalence(equivalent);
        }
        writer.enable_history(policy);
        if generations > 0 {
            covered.push(store);
        }
    }
    covered
}

/// Stores that stamp bookkeeping on saves that change no state. Without a
/// looser comparison, those stamps would count as changes and churn history.
fn equivalence(store: &str) -> Option<Equivalence> {
    match store {
        // Every discovery scan refreshes each alias's `last_seen_epoch_s`.
        "device-aliases" => Some(same_device_aliases),
        _ => None,
    }
}

/// Whether two device alias files pin the same identities, ignoring when
/// each was last seen.
#[must_use]
pub fn same_device_aliases(left: &[u8], right: &[u8]) -> bool {
    fn pins(bytes: &[u8]) -> Option<serde_json::Value> {
        let mut document: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        if let Some(aliases) = document
            .get_mut("aliases")
            .and_then(serde_json::Value::as_object_mut)
        {
            for record in aliases.values_mut() {
                if let Some(record) = record.as_object_mut() {
                    record.remove("last_seen_epoch_s");
                }
            }
        }
        Some(document)
    }
    match (pins(left), pins(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn is_excluded(name: &str) -> bool {
    HISTORY_EXCLUSIONS
        .iter()
        .any(|(excluded, _)| *excluded == name)
}

/// `hypercolor-daemon history`: inspect or roll back state files offline.
#[derive(Debug, Clone, Subcommand)]
pub enum HistoryCommand {
    /// List kept generations, oldest first.
    List {
        /// Only this store (for example `runtime-state` or `scenes`).
        store: Option<String>,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Replace a store's file with one of its generations. The daemon must
    /// be stopped; the replaced content is kept as a new generation.
    Restore {
        /// Store to restore, as `list` names it.
        store: String,
        /// Generation id from `list`.
        generation: u64,
    },
}

impl HistoryCommand {
    /// Whether the command writes store files, and so needs the daemon down.
    #[must_use]
    pub const fn writes(&self) -> bool {
        matches!(self, Self::Restore { .. })
    }
}

#[derive(Debug, Serialize)]
struct StoreListing {
    store: &'static str,
    path: PathBuf,
    generations: Vec<GenerationListing>,
}

#[derive(Debug, Serialize)]
struct GenerationListing {
    id: u64,
    replaced_at: String,
    saved_at: Option<String>,
    size: u64,
    path: PathBuf,
}

impl From<&Generation> for GenerationListing {
    fn from(generation: &Generation) -> Self {
        Self {
            id: generation.id,
            replaced_at: rfc3339(generation.replaced_at),
            saved_at: generation.saved_at.map(rfc3339),
            size: generation.size,
            path: generation.path.clone(),
        }
    }
}

fn rfc3339(time: chrono::DateTime<chrono::Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Run one `history` subcommand against `roots`, writing to `out`.
///
/// The caller holds the daemon's single-instance guard for commands that
/// [`write`](HistoryCommand::writes).
///
/// # Errors
///
/// Returns an error for an unknown store or generation, or when history
/// cannot be read or restored.
pub fn run_command(
    command: HistoryCommand,
    roots: &StoreRoots,
    daemon: &DaemonConfig,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    match command {
        HistoryCommand::List { store, json } => {
            let stores = select_stores(roots, store.as_deref())?;
            let listings = stores
                .into_iter()
                .map(|store| {
                    let generations =
                        list_generations(&history_directory(&roots.state_dir, store.name))?;
                    Ok(StoreListing {
                        store: store.name,
                        path: store.path,
                        generations: generations.iter().map(GenerationListing::from).collect(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            if json {
                writeln!(out, "{}", serde_json::to_string_pretty(&listings)?)?;
            } else {
                write_table(out, &listings)?;
            }
        }
        HistoryCommand::Restore { store, generation } => {
            let [store] = select_stores(roots, Some(&store))?
                .try_into()
                .map_err(|_| anyhow::anyhow!("one store expected"))?;
            // With history turned off, still restore from what an earlier
            // configuration kept, and prune nothing while doing it.
            let (generations, min_interval) = policy_settings(daemon);
            let keep = if generations == 0 {
                usize::MAX
            } else {
                generations
            };
            enable(roots, keep, min_interval);
            let restored = restore_generation(&store.path, generation).with_context(|| {
                format!("failed to restore {} generation {generation}", store.name)
            })?;
            writeln!(
                out,
                "Restored {} ({}) to generation {generation}.",
                store.name,
                store.path.display()
            )?;
            match restored.previous_generation {
                Some(previous) => writeln!(
                    out,
                    "The content it replaced is generation {previous}; restore that to undo."
                )?,
                None => writeln!(
                    out,
                    "The content it replaced was already kept (or identical), so nothing new was saved."
                )?,
            }
        }
    }
    Ok(())
}

fn select_stores(roots: &StoreRoots, store: Option<&str>) -> Result<Vec<StoreFile>> {
    let stores = history_stores(roots);
    let Some(name) = store else {
        return Ok(stores);
    };
    if let Some(store) = stores.iter().find(|candidate| candidate.name == name) {
        return Ok(vec![store.clone()]);
    }
    if let Some((_, reason)) = HISTORY_EXCLUSIONS
        .iter()
        .find(|(excluded, _)| *excluded == name)
    {
        bail!("no history is kept for {name}: {reason}");
    }
    let known = stores
        .iter()
        .map(|store| store.name)
        .collect::<Vec<_>>()
        .join(", ");
    bail!("unknown store {name}; stores with history: {known}")
}

fn write_table(out: &mut dyn std::io::Write, listings: &[StoreListing]) -> Result<()> {
    for listing in listings {
        writeln!(out, "{} ({})", listing.store, listing.path.display())?;
        if listing.generations.is_empty() {
            writeln!(out, "  no generations kept yet")?;
            continue;
        }
        writeln!(
            out,
            "  {:>6}  {:<24}  {:<24}  {:>9}",
            "id", "replaced at (UTC)", "saved at (UTC)", "bytes"
        )?;
        for generation in &listing.generations {
            writeln!(
                out,
                "  {:>6}  {:<24}  {:<24}  {:>9}",
                generation.id,
                generation.replaced_at,
                generation.saved_at.as_deref().unwrap_or("unknown"),
                generation.size
            )?;
        }
    }
    Ok(())
}
