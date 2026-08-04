use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn command_os_in(working_directory: &Path, program: &OsStr, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .current_dir(working_directory)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn command_in(working_directory: &Path, program: &str, args: &[&str]) -> Option<String> {
    command_os_in(working_directory, OsStr::new(program), args)
}

fn verified_repository_root(manifest_dir: &Path) -> Option<PathBuf> {
    let discovered = command_in(manifest_dir, "git", &["rev-parse", "--show-toplevel"])?;
    let discovered = fs::canonicalize(discovered).ok()?;
    let manifest_dir = fs::canonicalize(manifest_dir).ok()?;
    (discovered == manifest_dir).then_some(discovered)
}

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR"),
    );

    // Source and dependency changes must refresh identity. Git references are
    // watched only after proving that Git resolved this package repository,
    // rather than an unrelated parent consumer checkout.
    for path in ["src", "crates", "Cargo.toml", "Cargo.lock", "build.rs"] {
        println!("cargo:rerun-if-changed={path}");
    }

    let repository_root = verified_repository_root(&manifest_dir);
    if let Some(root) = repository_root.as_deref() {
        if let Some(head) = command_in(root, "git", &["rev-parse", "--git-path", "HEAD"]) {
            println!("cargo:rerun-if-changed={head}");
        }
        if let Some(symbolic_ref) = command_in(root, "git", &["symbolic-ref", "-q", "HEAD"]) {
            if let Some(branch_ref) =
                command_in(root, "git", &["rev-parse", "--git-path", &symbolic_ref])
            {
                println!("cargo:rerun-if-changed={branch_ref}");
            }
        }
        for git_path in ["index", "packed-refs"] {
            if let Some(path) = command_in(root, "git", &["rev-parse", "--git-path", git_path]) {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }

    let git_commit = repository_root
        .as_deref()
        .and_then(|root| command_in(root, "git", &["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unverified-no-package-repository-commit".to_owned());
    let git_dirty = repository_root.as_deref().is_none_or(|root| {
        Command::new("git")
            .current_dir(root)
            .args([
                "status",
                "--porcelain=v1",
                "--untracked-files=normal",
                "--",
                ".",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_none_or(|output| !output.stdout.is_empty())
    });
    let cargo_lock = fs::read(manifest_dir.join("Cargo.lock"))
        .expect("package-repository Cargo.lock is required for build identity");
    let package_lock_sha256 = format!("{:x}", Sha256::digest(cargo_lock));
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let rustc_command = rustc
        .to_str()
        .unwrap_or("unverified-non-utf8-rustc-command");
    let rustc_verbose = command_os_in(&manifest_dir, &rustc, &["-vV"])
        .unwrap_or_else(|| "unverified-rustc-version".to_owned());
    let rustc_version = rustc_verbose
        .lines()
        .next()
        .unwrap_or("unverified-rustc-version");
    let rustc_verbose_sha256 = format!("{:x}", Sha256::digest(rustc_verbose.as_bytes()));
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unverified-target".to_owned());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unverified-profile".to_owned());
    let mut enabled_features = std::env::vars_os()
        .filter_map(|(name, value)| {
            let name = name.to_str()?;
            (name.starts_with("CARGO_FEATURE_") && value == "1").then_some(name.to_owned())
        })
        .collect::<Vec<_>>();
    enabled_features.sort_unstable();
    let enabled_features = enabled_features.join(",");

    println!("cargo:rustc-env=HFT_RECON_GIT_COMMIT={git_commit}");
    println!("cargo:rustc-env=HFT_RECON_GIT_DIRTY={git_dirty}");
    println!("cargo:rustc-env=HFT_RECON_PACKAGE_LOCK_SHA256={package_lock_sha256}");
    println!("cargo:rustc-env=HFT_RECON_RUSTC_COMMAND={rustc_command}");
    println!("cargo:rustc-env=HFT_RECON_RUSTC_VERSION={rustc_version}");
    println!("cargo:rustc-env=HFT_RECON_RUSTC_VERBOSE_SHA256={rustc_verbose_sha256}");
    println!("cargo:rustc-env=HFT_RECON_TARGET={target}");
    println!("cargo:rustc-env=HFT_RECON_PROFILE={profile}");
    println!("cargo:rustc-env=HFT_RECON_ENABLED_FEATURES={enabled_features}");
}
