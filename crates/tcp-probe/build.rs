#![forbid(unsafe_code)]

use std::{env, path::PathBuf, process::Command};

fn is_lowercase_hex_40(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn main() {
    println!("cargo:rerun-if-env-changed=APSP_NATIVE_TCP_SOURCE_COMMIT");
    if env::var("APSP_NATIVE_TCP_SOURCE_COMMIT")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return;
    }

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by Cargo"),
    );
    let repo_root = manifest_dir.join("../..");
    let git_head = repo_root.join(".git/HEAD");
    if git_head.exists() {
        println!("cargo:rerun-if-changed={}", git_head.display());
    }

    if let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        && output.status.success()
        && let Ok(stdout) = String::from_utf8(output.stdout)
    {
        let commit = stdout.trim();
        if is_lowercase_hex_40(commit) {
            println!("cargo:rustc-env=APSP_NATIVE_TCP_SOURCE_COMMIT={commit}");
        }
    }
}
