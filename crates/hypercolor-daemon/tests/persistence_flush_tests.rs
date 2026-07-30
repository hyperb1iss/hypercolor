use std::time::{Duration, Instant};

use hypercolor_daemon::persistence::{AtomicFileWriter, flush_all};

#[test]
fn all_destinations_share_one_bounded_flush_deadline() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let writers = (0..3)
        .map(|index| {
            let path = directory.path().join(format!("state-{index}.json"));
            let writer = AtomicFileWriter::new(&path).expect("atomic writer");
            writer.set_injected_replace_failures(usize::MAX);
            writer.write(b"dirty").expect_err("injected failure");
            writer
        })
        .collect::<Vec<_>>();
    let started = Instant::now();

    let report = flush_all(Duration::from_millis(200));

    assert_eq!(report.errors().len(), writers.len());
    assert!(started.elapsed() < Duration::from_millis(450));

    for writer in &writers {
        writer.set_injected_replace_failures(0);
        writer.kick();
    }
    assert!(flush_all(Duration::from_secs(5)).is_complete());
}
