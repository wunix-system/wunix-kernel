use std::process::Command;

fn git_out(dir: &str, args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/main");
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search={dir}");
    println!("cargo:rustc-link-arg=-Tlinker.ld");

    let commit = git_out(&dir, &["rev-parse", "--short", "HEAD"])
        .unwrap_or_else(|| "ad6f3a9".to_string());
    let count = git_out(&dir, &["rev-list", "--count", "HEAD"])
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1);
    let dirty = git_out(&dir, &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let suffix = if dirty { "-dirty" } else { "" };
    let pkg_version = format!("0.1.{count}-{commit}{suffix}");

    println!("cargo:rustc-env=WUNIX_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=WUNIX_PKG_VERSION={pkg_version}");
}
