use std::process::Command;

fn main() {
    // Generate CalVer build version: YYYY.M.D
    // In release mode, use the date as-is. In debug mode, append "-dev".
    let now = current_date();
    let version = format!("{}.{}.{}", now.0, now.1, now.2);

    let profile = std::env::var("PROFILE").unwrap_or_default();
    let build_version = if profile == "release" {
        version
    } else {
        format!("{version}-dev")
    };

    // Try to get git short hash for extra context
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo::rustc-env=BUILD_VERSION={build_version}");
    println!("cargo::rustc-env=BUILD_GIT_HASH={git_hash}");

    // Re-run if git HEAD changes (new commit)
    println!("cargo::rerun-if-changed=.git/HEAD");
}

fn current_date() -> (i32, u32, u32) {
    // Parse date without pulling in chrono
    let output = Command::new("date")
        .args(["+%Y %m %d"])
        .output()
        .expect("failed to run date command");
    let s = String::from_utf8(output.stdout).unwrap();
    let parts: Vec<&str> = s.trim().split(' ').collect();
    let year: i32 = parts[0].parse().unwrap();
    let month: u32 = parts[1].parse().unwrap();
    let day: u32 = parts[2].parse().unwrap();
    (year, month, day)
}
