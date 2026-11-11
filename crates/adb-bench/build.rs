//! Records the version of the compiler that builds the harness, so a report can say which
//! toolchain produced the numbers.
use std::process::Command;

/// Passes `rustc --version` to the crate as `ADB_BENCH_RUSTC` (or `unknown`).
fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let version = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".to_string(), |s| s.trim().to_string());
    println!("cargo:rustc-env=ADB_BENCH_RUSTC={version}");
    println!("cargo:rerun-if-changed=build.rs");
}
