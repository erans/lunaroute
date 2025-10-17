use std::process::Command;

fn main() {
    // Get version from Cargo.toml
    let version = env!("CARGO_PKG_VERSION");
    println!("cargo:rustc-env=VERSION={}", version);

    // Get git SHA, fallback to "-dev" if not available
    let git_sha = std::env::var("GIT_SHA")
        .or_else(|_| {
            // Try to get from git command
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .and_then(|output| {
                    if output.status.success() {
                        String::from_utf8(output.stdout).ok()
                    } else {
                        None
                    }
                })
                .map(|s| s.trim().to_string())
                .ok_or(std::env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| "dev".to_string());

    println!("cargo:rustc-env=SHA={}", git_sha);

    // Re-run if git HEAD changes
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-env-changed=GIT_SHA");
}
