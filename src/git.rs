//! Commits (and pushes) a project's work when a ticket is done.

use std::path::Path;
use std::process::Command;

use crate::terminal::no_window;

/// Runs `git args` in `root`. Errors carry git's own message.
fn git(root: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let out = no_window(Command::new("git").args(args).current_dir(root))
        // Never wait on a credentials prompt nobody can see.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Couldn't run git: {e}"))?;
    if out.status.success() {
        return Ok(out);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let msg = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed");
    Err(format!("git {}: {}", args[0], msg.trim()))
}

/// Stages everything in the repository holding `root` and commits it as `message`, then pushes if `push`.
/// Returns false when there was nothing to commit (and so nothing was pushed).
pub fn commit(root: &Path, message: &str, push: bool) -> Result<bool, String> {
    git(root, &["add", "-A"])?;
    // `diff --quiet` exits 1 when something is staged.
    if git(root, &["diff", "--cached", "--quiet"]).is_ok() {
        return Ok(false);
    }
    git(root, &["commit", "-q", "-m", message])?;
    if push {
        git(root, &["push", "-q"])?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_changes_and_skips_when_clean() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        assert!(commit(root, "x", false).unwrap_err().contains("git add"));
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "Test"],
            &["config", "user.email", "test@example.com"],
        ] {
            git(root, args).unwrap();
        }
        std::fs::write(root.join("a.txt"), "a").unwrap();
        assert!(commit(root, "00001: Add \"a\"; rm -rf ~", false).unwrap());
        let log = git(root, &["log", "--format=%s"]).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&log.stdout).trim(),
            "00001: Add \"a\"; rm -rf ~"
        );
        assert!(!commit(root, "nothing", false).unwrap());
        std::fs::write(root.join("b.txt"), "b").unwrap();
        // No remote to push to.
        assert!(commit(root, "00002: b", true).unwrap_err().contains("git push"));
    }
}
