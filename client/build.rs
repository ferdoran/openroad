//! Embeds the commit the client was built from, so a running client can say
//! which build it is.
//!
//! Why this exists: evidence collected from a running game — a screenshot, a
//! packet window, a bug report — is unreadable later unless it names the build
//! it came from. The same screenshot means different things on two commits.
//! Before this, the closest answer was the commit of whatever checkout the
//! tooling happened to run in, which is a hint and not proof.
//!
//! Failure is not an error: a source tarball has no git data, so the commit
//! becomes `unknown` and the client reports it as unknown. A build that refuses
//! to compile outside a git checkout would be worse than one that cannot name
//! itself.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let (commit, dirty) = git_state();
    println!("cargo:rustc-env=OPENROAD_BUILD_COMMIT={commit}");
    println!(
        "cargo:rustc-env=OPENROAD_BUILD_DIRTY={}",
        if dirty { "1" } else { "0" }
    );

    // Rebuild when the checked-out commit changes. In this project most work
    // happens in `git worktree` checkouts, where `.git` is a FILE pointing at
    // the real git directory — watching `.git/HEAD` blindly would watch a path
    // that does not exist, and the stamp would then be frozen at whatever the
    // first build saw.
    if let Some(git_dir) = git_dir() {
        for file in ["HEAD", "index"] {
            let path = git_dir.join(file);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
        // A worktree's HEAD lives in the per-worktree directory, which the
        // `gitdir:` line already points at, so the loop above covers both.
    }
}

/// `(short commit, dirty)`. `dirty` counts **tracked** changes only: untracked
/// files are usually assets or scratch output and would mark almost every
/// developer build dirty, which would make the flag meaningless.
fn git_state() -> (String, bool) {
    let commit = run(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = run(&["status", "--porcelain", "--untracked-files=no"])
        .map(|out| !out.is_empty())
        .unwrap_or(false);
    (commit, dirty)
}

fn run(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}

/// The real git directory, resolving the one-line `gitdir:` file that a
/// worktree checkout has in place of a `.git` directory.
fn git_dir() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .to_path_buf();
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let path = text.strip_prefix("gitdir:")?.trim();
    let path = PathBuf::from(path);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(root.join(path))
    }
}
