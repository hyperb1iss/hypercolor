//! Resident memory sampling for the current process.

use hypercolor_macos_session::process_resident_memory_mb;

#[test]
fn resident_memory_is_reported_only_on_macos() {
    let resident = process_resident_memory_mb();
    if cfg!(target_os = "macos") {
        let resident = resident.expect("macOS should report resident memory");
        assert!(
            resident > 0.0,
            "resident memory should be positive, got {resident}"
        );
    } else {
        assert!(
            resident.is_none(),
            "only macOS reports resident memory here"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn resident_memory_grows_when_pages_are_touched() {
    const TOUCHED_MIB: u32 = 64;
    let touched_bytes = usize::try_from(TOUCHED_MIB).expect("small constant") * 1024 * 1024;

    let before = process_resident_memory_mb().expect("resident memory before touching");
    let block = std::hint::black_box(vec![1_u8; touched_bytes]);
    let after = process_resident_memory_mb().expect("resident memory after touching");
    drop(block);

    let grown = after - before;
    assert!(
        grown >= f64::from(TOUCHED_MIB / 2),
        "touching {TOUCHED_MIB} MiB grew resident memory by only {grown:.1} MiB"
    );
}
