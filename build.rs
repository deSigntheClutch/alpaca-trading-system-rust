//! Stamps every backtest result with the commit it ran from (`RunConfig.git_sha`), so
//! two results can be attributed to whatever changed between them. Falls back to
//! "unknown" outside a git checkout (a tarball, a container without git).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let sha = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=GIT_SHA={sha}");
    // Re-run when HEAD moves. `--git-path` resolves correctly inside a worktree, where
    // `.git` is a file pointing elsewhere.
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
}
