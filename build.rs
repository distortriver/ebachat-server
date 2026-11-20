//! The compiler's version for the startup log line where Go prints its runtime version (`go_version`).

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=EBACHAT_RUSTC_VERSION={version}");
    println!("cargo:rerun-if-changed=build.rs");
}
