//! Effect console journal.
//!
//! HTML effects run inside Servo, where `console.*` output and uncaught script
//! errors otherwise reach only the daemon log. The journal keeps a bounded,
//! process-wide tail of that output, tagged with the effect that printed it,
//! so authoring tools can read it with a cursor. The Servo runtime is itself
//! process-wide, which is why the journal is too.
//!
//! Effect pages load from a runtime copy with an injected preamble, so script
//! locations point at the copy. [`RuntimeLineShift`] maps those lines back to
//! the author's file before an entry is recorded.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Entries retained before the oldest are evicted.
pub const CONSOLE_JOURNAL_CAPACITY: usize = 512;

/// Longest message the journal stores, in characters. Longer messages keep
/// their head and gain a truncation marker, which bounds the journal's memory.
pub const MAX_CONSOLE_MESSAGE_CHARS: usize = 2000;

static GLOBAL_JOURNAL: LazyLock<EffectConsoleJournal> =
    LazyLock::new(|| EffectConsoleJournal::with_capacity(CONSOLE_JOURNAL_CAPACITY));

/// Where a block of injected lines sits in a runtime copy of an effect page.
///
/// The copy holds the author's file with `inserted_lines` extra lines spliced
/// in right after line `insert_line`. An `insert_line` of zero means the block
/// precedes the whole file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLineShift {
    /// Author line the injected block follows, or zero for a prefix.
    pub insert_line: usize,
    /// Lines the injected block adds.
    pub inserted_lines: usize,
}

impl RuntimeLineShift {
    /// Map a 1-based line in the runtime copy back to the author's file.
    ///
    /// Returns `None` for lines inside the injected block, which the author
    /// never wrote.
    #[must_use]
    pub const fn original_line(self, runtime_line: usize) -> Option<usize> {
        if runtime_line == 0 {
            return None;
        }
        if runtime_line <= self.insert_line {
            return Some(runtime_line);
        }
        if runtime_line < self.insert_line + self.inserted_lines {
            return None;
        }
        // A prefix block's own last line lands on zero, which no author
        // line occupies.
        match runtime_line - self.inserted_lines {
            0 => None,
            line => Some(line),
        }
    }
}

/// Which effect a Servo page belongs to, for tagging its console output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectConsoleSource {
    /// Display name of the effect.
    pub effect_name: String,
    /// The author's HTML file.
    pub source_path: PathBuf,
    /// The file Servo actually loaded; equal to `source_path` when the page
    /// loads verbatim.
    pub runtime_path: PathBuf,
    /// How the runtime copy shifts the author's lines, when it differs.
    pub line_shift: Option<RuntimeLineShift>,
}

impl EffectConsoleSource {
    /// A page loaded straight from the author's file.
    #[must_use]
    pub fn verbatim(effect_name: impl Into<String>, source_path: impl Into<PathBuf>) -> Self {
        let source_path = source_path.into();
        Self {
            effect_name: effect_name.into(),
            runtime_path: source_path.clone(),
            source_path,
            line_shift: None,
        }
    }

    /// Map a line in the loaded page back to the author's file.
    #[must_use]
    pub fn original_line(&self, runtime_line: usize) -> Option<usize> {
        match self.line_shift {
            Some(shift) => shift.original_line(runtime_line),
            None => (runtime_line > 0).then_some(runtime_line),
        }
    }
}

/// One console message, ready to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectConsoleRecord {
    /// Console level (`log`, `info`, `warn`, `error`, `debug`, `trace`, `dir`).
    pub level: String,
    /// Message text with locations already mapped to the author's file.
    pub message: String,
    /// Display name of the effect that printed it.
    pub effect_name: String,
    /// The author's HTML file.
    pub source_path: PathBuf,
    /// Line in the author's file the message points at, when it names one.
    pub line: Option<usize>,
}

/// One recorded console message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectConsoleEntry {
    /// Monotonic sequence number, starting at 1.
    pub seq: u64,
    /// Wall-clock time the message was recorded, in Unix milliseconds.
    pub timestamp_ms: u64,
    /// Console level.
    pub level: String,
    /// Message text with locations mapped to the author's file.
    pub message: String,
    /// Display name of the effect that printed it.
    pub effect_name: String,
    /// The author's HTML file.
    pub source_path: PathBuf,
    /// Line in the author's file the message points at, when it names one.
    pub line: Option<usize>,
}

/// One read from the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectConsolePage {
    /// Matching entries after the cursor, oldest first.
    pub entries: Vec<EffectConsoleEntry>,
    /// Cursor for the next read: pass it back as `after` to continue.
    pub next_seq: u64,
    /// Whether entries after the cursor were evicted before this read, so
    /// the caller missed some output.
    pub truncated: bool,
}

/// Bounded journal of effect console output.
#[derive(Debug)]
pub struct EffectConsoleJournal {
    capacity: usize,
    state: Mutex<JournalState>,
}

#[derive(Debug, Default)]
struct JournalState {
    entries: VecDeque<EffectConsoleEntry>,
    latest_seq: u64,
}

impl EffectConsoleJournal {
    /// The process-wide journal the Servo runtime records into.
    #[must_use]
    pub fn global() -> &'static Self {
        &GLOBAL_JOURNAL
    }

    /// A standalone journal, for tests and embedders.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new(JournalState::default()),
        }
    }

    /// Record one message and return its sequence number.
    pub fn record(&self, record: EffectConsoleRecord) -> u64 {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            });
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.latest_seq += 1;
        let seq = state.latest_seq;
        if state.entries.len() == self.capacity {
            state.entries.pop_front();
        }
        state.entries.push_back(EffectConsoleEntry {
            seq,
            timestamp_ms,
            level: record.level,
            message: truncate_message(record.message),
            effect_name: record.effect_name,
            source_path: record.source_path,
            line: record.line,
        });
        seq
    }

    /// Read up to `limit` entries recorded after `after` that satisfy
    /// `filter`, oldest first.
    ///
    /// The returned cursor skips entries the filter rejected, so a filtered
    /// reader does not see them again on its next call.
    pub fn read(
        &self,
        after: u64,
        limit: usize,
        filter: impl Fn(&EffectConsoleEntry) -> bool,
    ) -> EffectConsolePage {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        // A cursor past the newest entry came from before a daemon restart
        // reset the sequence, so everything retained is new to the caller.
        let after = if after > state.latest_seq { 0 } else { after };
        let oldest_seq = state
            .entries
            .front()
            .map_or(state.latest_seq + 1, |entry| entry.seq);
        let truncated = after.saturating_add(1) < oldest_seq && after < state.latest_seq;

        let mut entries = Vec::new();
        let mut next_seq = after.max(oldest_seq.saturating_sub(1));
        for entry in state.entries.iter().filter(|entry| entry.seq > after) {
            if entries.len() == limit {
                break;
            }
            next_seq = entry.seq;
            if filter(entry) {
                entries.push(entry.clone());
            }
        }
        if entries.len() < limit {
            next_seq = next_seq.max(state.latest_seq);
        }
        EffectConsolePage {
            entries,
            next_seq,
            truncated,
        }
    }

    /// Sequence number of the newest entry, or zero when nothing was
    /// recorded yet. Passing it as `after` reads only new output.
    #[must_use]
    pub fn latest_seq(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .latest_seq
    }
}

fn truncate_message(message: String) -> String {
    if message.chars().count() <= MAX_CONSOLE_MESSAGE_CHARS {
        return message;
    }
    let mut truncated: String = message.chars().take(MAX_CONSOLE_MESSAGE_CHARS).collect();
    truncated.push_str(" [truncated]");
    truncated
}
