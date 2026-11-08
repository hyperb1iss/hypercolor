//! Link the Visual C++ runtime into these programs.
//!
//! The launcher and the bootstrap slots live outside the units, where no
//! app-local copy of the runtime sits beside them, and a clean Windows
//! install ships none. Linking it statically leaves them importing only the
//! Universal CRT, which is part of Windows. The arguments are the ones
//! `tauri-build` gives the app shell, for the same reason.

use std::path::Path;
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    replace_msvcrt_import();
    for library in [
        "libvcruntimed.lib",
        "vcruntime.lib",
        "vcruntimed.lib",
        "libcmtd.lib",
        "msvcrt.lib",
        "msvcrtd.lib",
        "libucrt.lib",
        "libucrtd.lib",
    ] {
        println!("cargo:rustc-link-arg=/NODEFAULTLIB:{library}");
    }
    for library in ["libcmt.lib", "libvcruntime.lib", "ucrt.lib"] {
        println!("cargo:rustc-link-arg=/DEFAULTLIB:{library}");
    }
}

/// `rustc` names `msvcrt.lib` itself, which `/NODEFAULTLIB` does not reach,
/// so a nearly empty object file takes its place on this package's own
/// library path. A link-search path would also reach every crate that
/// depends on this one, and those link with no C runtime at all, because the
/// static runtime arguments above stay with this package.
fn replace_msvcrt_import() {
    let machine: [u8; 2] = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => [0x64, 0x86],
        Ok("aarch64") => [0x64, 0xAA],
        _ => return,
    };
    let body: [u8; 80] = [
        1, 0, 94, 3, 96, 98, 60, 0, 0, 0, 1, 0, 0, 0, 0, 0, 132, 1, 46, 100, 114, 101, 99, 116,
        118, 101, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 60, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 10, 16, 0, 46, 100, 114, 101, 99, 116, 118, 101, 0, 0, 0, 0, 1, 0, 0, 0, 3, 0, 4, 0,
        0, 0,
    ];
    let out = env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let library = [machine.as_slice(), body.as_slice()].concat();
    fs::write(Path::new(&out).join("msvcrt.lib"), library).expect("write msvcrt.lib");
    println!("cargo:rustc-link-arg=/LIBPATH:{out}");
}
