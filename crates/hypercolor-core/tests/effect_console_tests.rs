//! Effect console journal: cursors, filtering, eviction, and line mapping.

use std::path::PathBuf;

use hypercolor_core::effect::console::{
    EffectConsoleEntry, EffectConsoleJournal, EffectConsoleRecord, EffectConsoleSource,
    MAX_CONSOLE_MESSAGE_CHARS, RuntimeLineShift,
};

fn record(journal: &EffectConsoleJournal, effect: &str, message: &str) -> u64 {
    journal.record(EffectConsoleRecord {
        level: "log".to_owned(),
        message: message.to_owned(),
        effect_name: effect.to_owned(),
        source_path: PathBuf::from(format!("/fx/{effect}.html")),
        line: None,
    })
}

fn everything(_: &EffectConsoleEntry) -> bool {
    true
}

#[test]
fn reads_continue_from_the_returned_cursor() {
    let journal = EffectConsoleJournal::with_capacity(16);
    for index in 1..=5 {
        record(&journal, "ember", &format!("tick {index}"));
    }

    let first = journal.read(0, 2, everything);
    assert_eq!(
        first
            .entries
            .iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>(),
        ["tick 1", "tick 2"]
    );
    assert_eq!(first.next_seq, 2);
    assert!(!first.truncated);

    let rest = journal.read(first.next_seq, 10, everything);
    assert_eq!(rest.entries.len(), 3);
    assert_eq!(rest.entries[0].seq, 3);
    assert_eq!(rest.next_seq, 5);

    let idle = journal.read(rest.next_seq, 10, everything);
    assert!(idle.entries.is_empty());
    assert_eq!(idle.next_seq, 5, "an idle read keeps the cursor in place");
}

#[test]
fn filtered_reads_advance_past_rejected_entries() {
    let journal = EffectConsoleJournal::with_capacity(16);
    record(&journal, "ember", "a");
    record(&journal, "aurora", "b");
    record(&journal, "ember", "c");
    record(&journal, "aurora", "d");

    let ember = |entry: &EffectConsoleEntry| entry.effect_name == "ember";
    let page = journal.read(0, 1, ember);
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].message, "a");
    assert_eq!(page.next_seq, 1);

    let page = journal.read(page.next_seq, 10, ember);
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].message, "c");
    assert_eq!(
        page.next_seq, 4,
        "the cursor passes entries the filter rejected"
    );
}

#[test]
fn eviction_is_reported_to_readers_that_fell_behind() {
    let journal = EffectConsoleJournal::with_capacity(3);
    for index in 1..=5 {
        record(&journal, "ember", &format!("tick {index}"));
    }

    let behind = journal.read(1, 10, everything);
    assert!(behind.truncated, "entry 2 was evicted before this read");
    assert_eq!(behind.entries.first().map(|entry| entry.seq), Some(3));
    assert_eq!(behind.next_seq, 5);

    let current = journal.read(2, 10, everything);
    assert!(!current.truncated, "entry 3 onward is still retained");
}

#[test]
fn cursors_from_before_a_restart_read_from_the_start() {
    let journal = EffectConsoleJournal::with_capacity(8);
    record(&journal, "ember", "fresh");

    let page = journal.read(900, 10, everything);
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].message, "fresh");
    assert_eq!(page.next_seq, 1);
}

#[test]
fn long_messages_are_truncated_with_a_marker() {
    let journal = EffectConsoleJournal::with_capacity(4);
    record(
        &journal,
        "ember",
        &"x".repeat(MAX_CONSOLE_MESSAGE_CHARS + 50),
    );

    let entry = &journal.read(0, 1, everything).entries[0];
    assert!(entry.message.ends_with(" [truncated]"));
    assert_eq!(
        entry.message.chars().filter(|ch| *ch == 'x').count(),
        MAX_CONSOLE_MESSAGE_CHARS
    );
}

#[test]
fn latest_seq_marks_where_new_output_starts() {
    let journal = EffectConsoleJournal::with_capacity(8);
    assert_eq!(journal.latest_seq(), 0);
    record(&journal, "ember", "old");
    let mark = journal.latest_seq();
    record(&journal, "ember", "new");

    let page = journal.read(mark, 10, everything);
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].message, "new");
}

#[test]
fn line_shift_skips_the_injected_block() {
    let shift = RuntimeLineShift {
        insert_line: 4,
        inserted_lines: 6,
    };
    assert_eq!(shift.original_line(0), None);
    assert_eq!(shift.original_line(4), Some(4), "the split insertion line");
    assert_eq!(shift.original_line(5), None, "first injected line");
    assert_eq!(shift.original_line(9), None, "last injected line");
    assert_eq!(
        shift.original_line(10),
        Some(4),
        "the rest of the insertion line"
    );
    assert_eq!(shift.original_line(11), Some(5));

    let prefix = RuntimeLineShift {
        insert_line: 0,
        inserted_lines: 3,
    };
    assert_eq!(prefix.original_line(3), None);
    assert_eq!(prefix.original_line(4), Some(1));
}

#[test]
fn verbatim_sources_keep_their_lines() {
    let source = EffectConsoleSource::verbatim("Page", "/fx/page.html");
    assert_eq!(source.runtime_path, source.source_path);
    assert_eq!(source.original_line(7), Some(7));
    assert_eq!(source.original_line(0), None);
}
