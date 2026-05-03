#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    hypercolor_windows_pawnio::run_smbus_service()
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("hypercolor-smbus-service is only supported on Windows");
    std::process::exit(1);
}
