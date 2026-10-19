//! Commits (and pushes) a project's work when a ticket is done, shows what a ticket changed and reverts it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::terminal::no_window;

/// Everything in the repository except the board itself: ticket files and the `.ghost` file change with every
/// run, and a revert must never bring back or delete tickets.
const OUTSIDE_BOARD: [&str; 3] = [":/", ":(exclude)tickets", ":(exclude)*.ghost"];

/// Diffs bigger than this are cut off; the view only needs to give an idea.
const MAX_DIFF: usize = 512 * 1024;

/// Runs `git args` in `root`. Errors carry git's own message.
fn git(root: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    git_with_input(root, args, None)
}

/// `git`, with `input` written to its stdin.
fn git_with_input(root: &Path, args: &[&str], input: Option<&[u8]>) -> Result<std::process::Output, String> {
    let mut child = no_window(Command::new("git").args(args).current_dir(root))
        // Never wait on a credentials prompt nobody can see.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't run git: {e}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        // Dropped at the end of this block, so git sees the end of its input.
        stdin.write_all(input).map_err(|e| format!("Couldn't run git: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("Couldn't run git: {e}"))?;
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

fn stdout(out: std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// Commit ids come from ticket files, which can come from untrusted repos: only plain hex ever reaches git.
fn is_sha(s: &str) -> bool {
    (4..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn check_sha(s: &str) -> Result<(), String> {
    if is_sha(s) {
        Ok(())
    } else {
        Err(format!("\"{s}\" isn't a commit id"))
    }
}

/// The commit checked out in the repository holding `root`. `None` outside a repository or before the first commit.
pub fn head(root: &Path) -> Option<String> {
    git(root, &["rev-parse", "HEAD"]).ok().map(stdout).filter(|s| is_sha(s))
}

/// Short id of the latest commit `commit` made for ticket `id` (its message starts with `<id>: `) after `since`
/// (seconds since 1970), which keeps the search short in a long history.
pub fn ticket_commit(root: &Path, id: &str, since: u64) -> Option<String> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let since = format!("--since={}", crate::ticket::utc_timestamp(since.saturating_sub(3600)));
    let grep = format!("--grep=^{id}: ");
    let out = git(root, &["log", "-1", "--format=%h", &since, &grep]).ok()?;
    Some(stdout(out)).filter(|s| is_sha(s))
}

/// What a ticket changed outside the board, as a unified diff.
#[derive(Clone, Debug, PartialEq)]
pub struct Changes {
    /// The ticket's commit, when it has one. Otherwise the diff runs from a base commit to the working tree.
    pub commit: Option<String>,
    pub diff: String,
}

/// The changes made in `commit`.
pub fn commit_changes(root: &Path, commit: &str) -> Result<Changes, String> {
    check_sha(commit)?;
    let mut args = vec!["show", "--format=", "--no-ext-diff", "--no-color", commit, "--"];
    args.extend(OUTSIDE_BOARD);
    Ok(Changes {
        commit: Some(commit.to_string()),
        diff: cap(stdout(git(root, &args)?)),
    })
}

/// Everything changed since commit `base`, committed or not, including new files git doesn't track yet.
pub fn changes_since(root: &Path, base: &str) -> Result<Changes, String> {
    check_sha(base)?;
    let mut args = vec!["diff", "--no-ext-diff", "--no-color", base, "--"];
    args.extend(OUTSIDE_BOARD);
    let mut diff = stdout(git(root, &args)?);
    let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
    args.extend(OUTSIDE_BOARD);
    let untracked = git(root, &args)?;
    for name in String::from_utf8_lossy(&untracked.stdout)
        .split('\0')
        .filter(|n| !n.is_empty())
    {
        if diff.len() > MAX_DIFF {
            break;
        }
        diff.push_str(&new_file(&root.join(name), name));
    }
    Ok(Changes {
        commit: None,
        diff: cap(diff),
    })
}

/// An untracked file as a diff that adds it.
fn new_file(path: &Path, name: &str) -> String {
    let mut out = format!("\ndiff --git a/{name} b/{name}\nnew file\n--- /dev/null\n+++ b/{name}\n");
    match std::fs::read(path) {
        Ok(bytes) if bytes.contains(&0) => out.push_str("Binary file\n"),
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            out.push_str(&format!("@@ -0,0 +1,{} @@\n", text.lines().count()));
            for line in text.lines() {
                out.push('+');
                out.push_str(line);
                out.push('\n');
            }
        }
        Err(e) => out.push_str(&format!("Couldn't read it: {e}\n")),
    }
    out
}

fn cap(mut diff: String) -> String {
    if diff.len() > MAX_DIFF {
        let mut end = MAX_DIFF;
        while !diff.is_char_boundary(end) {
            end -= 1;
        }
        diff.truncate(end);
        diff.push_str("\n\n… cut off here; see the rest with git.");
    }
    diff.trim_start_matches('\n').to_string()
}

/// Undoes `commit` with a new commit named `message`, leaving the board's own files (tickets, `.ghost`) alone so
/// the ticket stays on the board. Changes nothing when the files it touches have changed since.
pub fn revert(root: &Path, commit: &str, message: &str) -> Result<(), String> {
    check_sha(commit)?;
    // `apply` run from a subfolder skips files outside it.
    let top = PathBuf::from(stdout(git(root, &["rev-parse", "--show-toplevel"])?));
    if git(&top, &["diff", "--cached", "--quiet"]).is_err() {
        return Err("Commit or unstage the changes already staged in git first".into());
    }
    let parent = format!("{commit}^");
    let mut args = vec!["diff", "--binary", "--no-ext-diff", "--no-color", commit, &parent, "--"];
    args.extend(OUTSIDE_BOARD);
    let patch = git(root, &args)?.stdout;
    if patch.is_empty() {
        return Err("The commit only changed the board, so there's nothing to revert".into());
    }
    git_with_input(&top, &["apply", "--index"], Some(&patch))
        .map_err(|e| format!("{e}. Files it touched have changed since; revert it by hand"))?;
    git(&top, &["commit", "-q", "-m", message])?;
    Ok(())
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

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "Test"],
            &["config", "user.email", "test@example.com"],
            &["config", "core.autocrlf", "false"],
        ] {
            git(d.path(), args).unwrap();
        }
        d
    }

    #[test]
    fn shows_and_reverts_a_tickets_changes_but_not_the_board() {
        let d = repo();
        let root = d.path();
        let write = |name: &str, text: &str| {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap();
        write("a.txt", "a\n");
        write("tickets/to_do/00001.md", "t\n");
        commit(root, "00000: setup", false).unwrap();
        let base = head(root).unwrap();
        assert!(ticket_commit(root, "00001", 0).is_none());

        write("a.txt", "a2\n");
        write("new.txt", "fresh\n");
        write("tickets/to_do/00001.md", "t2\n");
        let open = changes_since(root, &base).unwrap();
        assert_eq!(open.commit, None);
        assert!(open.diff.contains("-a\n+a2"), "{}", open.diff);
        assert!(
            open.diff.contains("+++ b/new.txt\n@@ -0,0 +1,1 @@\n+fresh"),
            "{}",
            open.diff
        );
        assert!(!open.diff.contains("00001.md"), "{}", open.diff);

        std::fs::rename(root.join("tickets/to_do"), root.join("tickets/done")).unwrap();
        commit(root, "00001: Thing", false).unwrap();
        let sha = ticket_commit(root, "00001", crate::ticket::now_secs()).unwrap();
        let done = commit_changes(root, &sha).unwrap();
        assert_eq!(done.commit.as_deref(), Some(sha.as_str()));
        assert!(done.diff.contains("+a2") && done.diff.contains("new.txt") && !done.diff.contains("00001"));

        // A ticket file edited since doesn't get in the way, and stays where the board put it.
        write("tickets/done/00001.md", "t3\n");
        revert(root, &sha, "Revert 00001").unwrap();
        assert_eq!(read("a.txt"), "a\n");
        assert!(!root.join("new.txt").exists());
        assert_eq!(read("tickets/done/00001.md"), "t3\n");
        assert_eq!(
            stdout(git(root, &["log", "-1", "--format=%s"]).unwrap()),
            "Revert 00001"
        );

        // Changed again since: refuses and leaves everything as it is.
        let again = head(root).unwrap();
        write("a.txt", "mine\n");
        assert!(revert(root, &sha, "x").is_err());
        assert_eq!(head(root).unwrap(), again);
        assert_eq!(read("a.txt"), "mine\n");
    }

    #[test]
    fn only_plain_commit_ids_reach_git() {
        let d = repo();
        assert!(changes_since(d.path(), "--output=x").is_err());
        assert!(commit_changes(d.path(), "HEAD; rm").is_err());
        assert!(revert(d.path(), "-n", "m").is_err());
        assert!(ticket_commit(d.path(), "1 --all", 0).is_none());
        assert!(head(d.path()).is_none()); // no commits yet
    }
}
