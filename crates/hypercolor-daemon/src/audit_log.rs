//! Persistent audit trail of state-changing requests.
//!
//! Every POST, PUT, PATCH, or DELETE on the local API, every REST-equivalent
//! WebSocket `command` with one of those methods, and every MCP tool call
//! that is not read-only produces one [`AuditEntry`]. The entry goes to the
//! normal tracing output at `info` and, when the daemon installed an
//! [`AuditLog`], to a size-capped JSON Lines file:
//!
//! ```text
//! $XDG_STATE_HOME/hypercolor/logs/api-audit.jsonl      newest
//! $XDG_STATE_HOME/hypercolor/logs/api-audit.1.jsonl    rotated, and so on
//! ```
//!
//! One JSON object per line, the same shape `GET /api/v1/system/audit`
//! returns. The active file rotates at 1 MiB and four rotated files are
//! kept, so the trail never exceeds about 5 MiB.
//!
//! Entries record the method, the path without its query string, the
//! status, the client address and user agent, and which durable stores the
//! request changed. They never record request bodies, query strings,
//! credentials, or MCP tool arguments.
//!
//! Store attribution works through `hypercolor-persistence`'s replacement
//! observer: a request runs inside [`collect_changes`], and every store file
//! whose bytes change on that task (or in a blocking closure run through
//! [`with_change_scope`]) is attributed to it. Writes made later by
//! background workers, such as a retry after a failed write, are not.

use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use hypercolor_types::api::system::{AuditEntry, AuditTransport};

use crate::state_history::StoreFile;

/// Directory under the state root that holds the audit trail.
pub const AUDIT_LOG_DIR: &str = "logs";
/// The active audit file.
pub const AUDIT_LOG_FILE: &str = "api-audit.jsonl";
/// Size at which the active file rotates.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Rotated files kept beside the active one.
pub const ROTATED_FILES: usize = 4;
/// Largest page `GET /system/audit` returns.
pub const MAX_QUERY_LIMIT: usize = 1000;

tokio::task_local! {
    static CHANGES: Arc<ChangeScope>;
    static WS_PEER: Arc<AuditPeer>;
}

/// Store files changed while one request ran.
#[derive(Debug, Default)]
pub struct ChangeScope {
    paths: Mutex<Vec<PathBuf>>,
}

impl ChangeScope {
    fn push(&self, path: &Path) {
        let mut paths = self.paths.lock().unwrap_or_else(PoisonError::into_inner);
        if !paths.iter().any(|seen| seen == path) {
            paths.push(path.to_path_buf());
        }
    }

    fn take(&self) -> Vec<PathBuf> {
        std::mem::take(&mut *self.paths.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Who sent a request, for transports without a per-request socket address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditPeer {
    pub remote: String,
    pub user_agent: String,
}

impl AuditPeer {
    /// The peer for an in-process trusted socket, which has no address.
    #[must_use]
    pub fn in_process() -> Self {
        Self {
            remote: "in-process".to_owned(),
            user_agent: String::new(),
        }
    }
}

/// Route every store replacement that changes bytes into the change scope of
/// the task that made it. Idempotent.
pub fn install_store_observer() {
    let _ = crate::persistence::set_replacement_observer(record_change);
}

fn record_change(path: &Path) {
    let _ = CHANGES.try_with(|scope| scope.push(path));
}

/// Run `future` and return the store files whose bytes it changed.
pub async fn collect_changes<F: Future>(future: F) -> (F::Output, Vec<PathBuf>) {
    let scope = Arc::new(ChangeScope::default());
    let output = CHANGES.scope(Arc::clone(&scope), future).await;
    (output, scope.take())
}

/// The change scope of the current task, to carry into a blocking closure.
#[must_use]
pub fn current_change_scope() -> Option<Arc<ChangeScope>> {
    CHANGES.try_with(Arc::clone).ok()
}

/// Run `operation` inside `scope`, so the store writes it makes on a
/// blocking thread are attributed to the request that started it.
pub fn with_change_scope<R>(scope: Option<Arc<ChangeScope>>, operation: impl FnOnce() -> R) -> R {
    match scope {
        Some(scope) => CHANGES.sync_scope(scope, operation),
        None => operation(),
    }
}

/// Run a WebSocket session with `peer` as the sender of its commands.
pub async fn with_ws_peer<F: Future>(peer: AuditPeer, future: F) -> F::Output {
    WS_PEER.scope(Arc::new(peer), future).await
}

/// The peer of the WebSocket session running on this task.
#[must_use]
pub fn current_ws_peer() -> Option<Arc<AuditPeer>> {
    WS_PEER.try_with(Arc::clone).ok()
}

/// Whether `method` is one the audit trail records.
#[must_use]
pub fn is_mutating(method: &axum::http::Method) -> bool {
    use axum::http::Method;
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// The current instant as the trail spells timestamps.
#[must_use]
pub fn timestamp_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Emit `entry` to tracing, and to `log` when the daemon installed one.
pub fn record(log: Option<&AuditLog>, entry: &AuditEntry) {
    tracing::info!(
        transport = ?entry.transport,
        method = %entry.method,
        path = %entry.path,
        tool = entry.tool.as_deref().unwrap_or(""),
        status = entry.status,
        remote = %entry.remote,
        user_agent = %entry.user_agent,
        stores = %entry.stores.join(","),
        "State-changing request"
    );
    if let Some(log) = log
        && let Err(error) = log.append(entry)
    {
        tracing::warn!(
            directory = %log.directory.display(),
            %error,
            "Failed to append to the audit log"
        );
    }
}

/// The rotating audit file and the store names it reports.
#[derive(Debug)]
pub struct AuditLog {
    directory: PathBuf,
    stores: Vec<NamedStore>,
    max_file_bytes: u64,
    rotated_files: usize,
    file: Mutex<Option<OpenLog>>,
}

#[derive(Debug)]
struct NamedStore {
    name: &'static str,
    path: PathBuf,
    /// `path` with its directory resolved when the log was created. The
    /// persistence layer reports resolved paths, so this is the fast match.
    resolved: PathBuf,
}

impl NamedStore {
    fn matches(&self, changed: &Path) -> bool {
        // A directory created after startup only resolves now.
        self.resolved == changed || canonical(&self.path) == changed
    }
}

#[derive(Debug)]
struct OpenLog {
    file: File,
    size: u64,
}

impl AuditLog {
    /// An audit trail in `directory` that names changes to `stores`.
    ///
    /// Nothing is created until the first entry is written.
    #[must_use]
    pub fn new(directory: PathBuf, stores: &[StoreFile]) -> Self {
        let stores = stores
            .iter()
            .map(|store| NamedStore {
                name: store.name,
                path: store.path.clone(),
                resolved: canonical(&store.path),
            })
            .collect();
        Self {
            directory,
            stores,
            max_file_bytes: MAX_FILE_BYTES,
            rotated_files: ROTATED_FILES,
            file: Mutex::new(None),
        }
    }

    /// Override the rotation limits.
    #[must_use]
    pub fn with_limits(mut self, max_file_bytes: u64, rotated_files: usize) -> Self {
        self.max_file_bytes = max_file_bytes;
        self.rotated_files = rotated_files;
        self
    }

    /// Directory holding the trail.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Inventory names for changed store files; files outside the inventory
    /// report their file name.
    #[must_use]
    pub fn store_names(&self, paths: &[PathBuf]) -> Vec<String> {
        let mut names: Vec<String> = paths
            .iter()
            .map(|path| {
                self.stores
                    .iter()
                    .find(|store| store.resolved == *path)
                    .or_else(|| self.stores.iter().find(|store| store.matches(path)))
                    .map_or_else(
                        || {
                            path.file_name().map_or_else(String::new, |name| {
                                name.to_string_lossy().into_owned()
                            })
                        },
                        |store| store.name.to_owned(),
                    )
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Append one entry, rotating first when it would overflow the file.
    ///
    /// # Errors
    ///
    /// Returns the filesystem error when the trail cannot be written.
    pub fn append(&self, entry: &AuditEntry) -> io::Result<()> {
        let mut line = serde_json::to_vec(entry).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut open = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        if open.is_none() {
            *open = Some(self.open_active()?);
        }
        if let Some(active) = open.as_ref()
            && active.size > 0
            && active.size + line.len() as u64 > self.max_file_bytes
        {
            *open = None;
            self.rotate()?;
            *open = Some(self.open_active()?);
        }
        let active = open.as_mut().expect("active audit file opened above");
        active.file.write_all(&line)?;
        active.size += line.len() as u64;
        Ok(())
    }

    /// Up to `limit` entries, newest first.
    ///
    /// # Errors
    ///
    /// Returns the filesystem error when a trail file exists but cannot be
    /// read. Lines that do not parse are skipped.
    pub fn recent(&self, limit: usize) -> io::Result<Vec<AuditEntry>> {
        let _writer = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        let mut entries = Vec::new();
        for index in 0..=self.rotated_files {
            if entries.len() >= limit {
                break;
            }
            let path = self.file_path(index);
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let mut lines = Vec::new();
            for line in BufReader::new(file).lines() {
                let line = line?;
                if let Ok(entry) = serde_json::from_str::<AuditEntry>(&line) {
                    lines.push(entry);
                }
            }
            let wanted = limit - entries.len();
            entries.extend(lines.into_iter().rev().take(wanted));
        }
        Ok(entries)
    }

    fn file_path(&self, index: usize) -> PathBuf {
        if index == 0 {
            self.directory.join(AUDIT_LOG_FILE)
        } else {
            self.directory.join(format!("api-audit.{index}.jsonl"))
        }
    }

    fn open_active(&self) -> io::Result<OpenLog> {
        fs::create_dir_all(&self.directory)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.file_path(0))?;
        let size = file.metadata()?.len();
        Ok(OpenLog { file, size })
    }

    fn rotate(&self) -> io::Result<()> {
        if self.rotated_files == 0 {
            return remove_if_present(&self.file_path(0));
        }
        for index in (1..self.rotated_files).rev() {
            rename_if_present(&self.file_path(index), &self.file_path(index + 1))?;
        }
        rename_if_present(&self.file_path(0), &self.file_path(1))
    }
}

/// Build an entry for a finished request.
#[must_use]
pub fn entry(
    log: Option<&AuditLog>,
    transport: AuditTransport,
    request: RequestLine<'_>,
    status: u16,
    changed: &[PathBuf],
    latency_ms: f64,
) -> AuditEntry {
    AuditEntry {
        timestamp: timestamp_now(),
        transport,
        method: request.method.to_owned(),
        path: request.path.to_owned(),
        tool: request.tool.map(str::to_owned),
        status,
        remote: request.remote.to_owned(),
        user_agent: request.user_agent.to_owned(),
        stores: log.map_or_else(
            || {
                changed
                    .iter()
                    .filter_map(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .collect()
            },
            |log| log.store_names(changed),
        ),
        latency_ms,
    }
}

/// The identifying fields of one request.
#[derive(Debug, Clone, Copy)]
pub struct RequestLine<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub tool: Option<&'a str>,
    pub remote: &'a str,
    pub user_agent: &'a str,
}

fn canonical(path: &Path) -> PathBuf {
    let Some(file_name) = path.file_name() else {
        return path.to_path_buf();
    };
    path.parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .map_or_else(|| path.to_path_buf(), |parent| parent.join(file_name))
}

fn rename_if_present(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
