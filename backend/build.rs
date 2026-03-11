use std::process::Command;

fn main() {
    // Re-run if git state or env var changes
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/");
    println!("cargo:rerun-if-env-changed=PHOS_VERSION");

    let version = std::env::var("PHOS_VERSION")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| git_tag())
        .or_else(|| git_branch())
        .or_else(|| git_sha())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=PHOS_VERSION={}", version);
}

fn git_tag() -> Option<String> {
    Command::new("git")
        .args(["describe", "--tags", "--exact-match"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn git_branch() -> Option<String> {
    Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn git_sha() -> Option<String> {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| format!("sha-{}", s.trim()))
        .filter(|s| s != "sha-")
}
