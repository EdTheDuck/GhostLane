//! Commits (and pushes) a project's work when a ticket is done, shows what a ticket changed and reverts it.
//! With parallel runs, each ticket works in a worktree of its own on a `ghostlane/<id>` branch, merged back (or
//! opened as a pull request) when it is done.

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
    checked("git", args, run("git", root, args, input)?)
}

/// Runs `program args` in `root` whatever its exit code; errors only when it can't be started.
fn run(program: &str, root: &Path, args: &[&str], input: Option<&[u8]>) -> Result<std::process::Output, String> {
    let mut child = no_window(Command::new(program).args(args).current_dir(root))
        // Never wait on a credentials prompt nobody can see.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't run {program}: {e}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        // Dropped at the end of this block, so git sees the end of its input.
        stdin
            .write_all(input)
            .map_err(|e| format!("Couldn't run {program}: {e}"))?;
    }
    child
        .wait_with_output()
        .map_err(|e| format!("Couldn't run {program}: {e}"))
}

/// `out`, or its last line of error output when it failed.
fn checked(program: &str, args: &[&str], out: std::process::Output) -> Result<std::process::Output, String> {
    if out.status.success() {
        return Ok(out);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let msg = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed");
    Err(format!("{program} {}: {}", args[0], msg.trim()))
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

/// The branch ticket `id` works on in its worktree.
pub fn ticket_branch(id: &str) -> String {
    format!("ghostlane/{id}")
}

/// Where the project folder `root` sits in its repository: empty at the top, otherwise like `app/`.
pub fn prefix(root: &Path) -> Result<String, String> {
    Ok(stdout(git(root, &["rev-parse", "--show-prefix"])?))
}

/// Top folder of the worktree that has `branch` checked out, if one does.
pub fn worktree_of(root: &Path, branch: &str) -> Option<PathBuf> {
    let list = stdout(git(root, &["worktree", "list", "--porcelain"]).ok()?);
    let want = format!("branch refs/heads/{branch}");
    let mut path = None;
    for line in list.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if line == want {
            return path;
        }
    }
    None
}

/// Makes sure `branch` has a worktree (a new one goes in `dir`, branched off the main checkout's HEAD) and brings
/// the main checkout's latest commit into it. Merge conflicts stay in the worktree for the AI to resolve.
/// Returns the worktree's top folder.
pub fn prepare_worktree(root: &Path, dir: &Path, branch: &str) -> Result<PathBuf, String> {
    let head = head(root).ok_or("Parallel runs need a git repository with at least one commit")?;
    if let Some(existing) = worktree_of(root, branch).filter(|p| p.is_dir()) {
        // Already up to date, or conflicts left for the AI: neither stops the run.
        let _ = git(&existing, &["merge", "-q", "--no-edit", &head]);
        return Ok(existing);
    }
    // Forget worktrees whose folders were deleted, so their branches can be checked out again.
    git(root, &["worktree", "prune"])?;
    if dir.exists() {
        // Not a worktree git knows about (checked above): left over, and only ever in GhostLane's own folder.
        std::fs::remove_dir_all(dir).map_err(|e| format!("Couldn't clear {}: {e}", dir.display()))?;
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't create {}: {e}", parent.display()))?;
    }
    let path = dir.to_string_lossy();
    let exists = git(root, &["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).is_ok();
    if exists {
        git(root, &["worktree", "add", "-q", &path, branch])?;
        let _ = git(dir, &["merge", "-q", "--no-edit", &head]);
    } else {
        git(root, &["worktree", "add", "-q", "-b", branch, &path, &head])?;
    }
    Ok(dir.to_path_buf())
}

/// Commits what changed in the worktree's project folder `dir` to its branch, then drops what is left: edits to
/// the worktree's copies of the board, which belongs to the main checkout.
pub fn save_worktree(dir: &Path, message: &str) -> Result<(), String> {
    let mut args = vec!["add", "-A", "--"];
    args.extend(OUTSIDE_BOARD);
    git(dir, &args)?;
    if git(dir, &["diff", "--cached", "--quiet"]).is_err() {
        git(dir, &["commit", "-q", "-m", message])?;
    }
    // Only reached once everything else is committed, so this can only throw away board edits.
    git(dir, &["reset", "-q", "--hard"])?;
    Ok(())
}

/// Why a ticket's branch couldn't be merged.
#[derive(Debug, PartialEq)]
pub enum MergeError {
    /// It changes these files in ways that clash with the main checkout.
    Conflicts(Vec<String>),
    Failed(String),
}

/// Stages everything `branch` changed onto the main checkout at `root` without committing (a squash merge).
/// Changes nothing when it would conflict.
pub fn squash_merge(root: &Path, branch: &str) -> Result<(), MergeError> {
    let args = [
        "merge-tree",
        "--write-tree",
        "--name-only",
        "--no-messages",
        "HEAD",
        branch,
    ];
    let out = run("git", root, &args, None).map_err(MergeError::Failed)?;
    if out.status.code() == Some(1) {
        // The new tree's id, then the files that clash.
        let files = String::from_utf8_lossy(&out.stdout)
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        return Err(MergeError::Conflicts(files));
    }
    checked("git", &args, out).map_err(MergeError::Failed)?;
    git(root, &["merge", "-q", "--squash", branch]).map_err(MergeError::Failed)?;
    Ok(())
}

/// The commit where `dir`'s HEAD and commit `other` last met.
pub fn merge_base(dir: &Path, other: &str) -> Result<String, String> {
    check_sha(other)?;
    let base = stdout(git(dir, &["merge-base", "HEAD", other])?);
    check_sha(&base)?;
    Ok(base)
}

/// Deletes the worktree at `dir` and its `branch`.
pub fn remove_worktree(root: &Path, dir: &Path, branch: &str) -> Result<(), String> {
    git(root, &["worktree", "remove", "--force", &dir.to_string_lossy()])?;
    git(root, &["branch", "-q", "-D", branch])?;
    Ok(())
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
    fn worktrees_carry_a_tickets_work_until_it_merges() {
        let d = repo();
        let root = d.path();
        let write = |dir: &Path, name: &str, text: &str| {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(root, "a.txt", "a\n");
        write(root, "b.txt", "b\n");
        write(root, "tickets/to_do/00001.md", "t\n");
        commit(root, "setup", false).unwrap();
        let branch = ticket_branch("00001");
        let dirs = tempfile::tempdir().unwrap();
        let dir = dirs.path().join("wt");

        let wt = prepare_worktree(root, &dir, &branch).unwrap();
        assert_eq!(
            worktree_of(root, &branch).map(|p| p.canonicalize().unwrap()),
            Some(wt.canonicalize().unwrap())
        );
        write(&wt, "a.txt", "a from the ticket\n");
        write(&wt, "new.txt", "new\n");
        write(&wt, "tickets/to_do/00001.md", "the copy, not the board\n");
        save_worktree(&wt, "Work on 00001").unwrap();
        assert_eq!(
            std::fs::read_to_string(wt.join("tickets/to_do/00001.md")).unwrap(),
            "t\n"
        );
        assert!(stdout(git(&wt, &["status", "--porcelain"]).unwrap()).is_empty());

        // The main checkout moved on meanwhile, without clashing; the next run takes that in.
        write(root, "b.txt", "b on main\n");
        commit(root, "main moves", false).unwrap();
        let again = prepare_worktree(root, &dir, &branch).unwrap();
        assert_eq!(again.canonicalize().unwrap(), wt.canonicalize().unwrap());
        assert_eq!(std::fs::read_to_string(wt.join("b.txt")).unwrap(), "b on main\n");

        squash_merge(root, &branch).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "a from the ticket\n"
        );
        assert!(root.join("new.txt").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("tickets/to_do/00001.md")).unwrap(),
            "t\n"
        );
        commit(root, "00001: Thing", false).unwrap();
        remove_worktree(root, &wt, &branch).unwrap();
        assert!(!wt.exists());
        assert_eq!(worktree_of(root, &branch), None);
    }

    #[test]
    fn clashing_branches_merge_nothing() {
        let d = repo();
        let root = d.path();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        commit(root, "setup", false).unwrap();
        let dirs = tempfile::tempdir().unwrap();
        let branch = ticket_branch("00002");
        let wt = prepare_worktree(root, &dirs.path().join("wt"), &branch).unwrap();
        std::fs::write(wt.join("a.txt"), "ticket\n").unwrap();
        save_worktree(&wt, "Work on 00002").unwrap();
        std::fs::write(root.join("a.txt"), "main\n").unwrap();
        commit(root, "main", false).unwrap();
        let before = head(root).unwrap();
        assert_eq!(
            squash_merge(root, &branch),
            Err(MergeError::Conflicts(vec!["a.txt".into()]))
        );
        assert_eq!(head(root).unwrap(), before);
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "main\n");
        assert!(stdout(git(root, &["status", "--porcelain"]).unwrap()).is_empty());
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
