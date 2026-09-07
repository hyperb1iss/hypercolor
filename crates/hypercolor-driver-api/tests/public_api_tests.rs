#[test]
fn driver_contracts_have_one_public_import_path() {
    let crate_root = include_str!("../src/lib.rs");

    assert!(!crate_root.lines().any(|line| line.starts_with("pub mod ")));
}
