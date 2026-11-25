use std::io::Write;
use std::process::{Command, Stdio};

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

fn git_tree_content_hash(dir: &str) -> Option<String> {
    let tree = Command::new("git")
        .args(["ls-tree", "-r", "HEAD", "--", "."])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())?;

    let mut child = Command::new("git")
        .args(["hash-object", "--stdin"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&tree.stdout);
    }
    let out = child.wait_with_output().ok().filter(|o| o.status.success())?;
    let hex = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if hex.len() >= 7 {
        Some(hex[..7].to_string())
    } else {
        None
    }
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

    let count = git_out(&dir, &["rev-list", "--count", "HEAD"])
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1);
    let dirty = git_out(&dir, &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let hash = git_tree_content_hash(&dir)
        .or_else(|| git_out(&dir, &["rev-parse", "--short=7", "HEAD"]))
        .unwrap_or_else(|| "ad6f3a9".to_string());
    let suffix = if dirty { "-dirty" } else { "" };
    let pkg_version = format!("0.1.{count}-{hash}{suffix}");

    println!("cargo:rustc-env=WUNIX_GIT_COMMIT={hash}");
    println!("cargo:rustc-env=WUNIX_PKG_VERSION={pkg_version}");
}
