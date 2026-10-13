//! Developer and Tester sub agents for AI CLIs that support them, built from the project's columns.

use std::fs;
use std::io;
use std::path::PathBuf;

use crate::project::{AutoMove, Project};
use crate::ticket::write_atomic;

/// Files carrying this are GhostLane's to rewrite; anything else at the same path is left alone.
const MARKER: &str = "Managed by GhostLane";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Developer,
    Tester,
}

impl Role {
    fn id(self) -> &'static str {
        match self {
            Role::Developer => "developer",
            Role::Tester => "tester",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Role::Developer => {
                "Develops GhostLane tickets. Use for any development task on a ticket, from picking it up and \
                 moving it through the development columns to implementing it."
            }
            Role::Tester => {
                "Tests GhostLane tickets. Use for any testing task on a ticket, from checking it against its Test \
                 Scope to moving it on when it passes or back for rework when it fails."
            }
        }
    }
}

/// A step the AI works through: tickets wait in `from`, move to `working` when the AI starts (an
/// "AI Working" transition) and on to `done` when it finishes.
#[derive(Debug, PartialEq)]
struct Stage {
    from: usize,
    working: usize,
    done: Option<usize>,
}

fn stages(p: &Project) -> Vec<Stage> {
    let n = p.meta.columns.len();
    (1..n)
        .filter(|&i| p.meta.columns[i].auto_move == AutoMove::AiWorking)
        .map(|i| Stage {
            from: i - 1,
            working: i,
            done: (i + 1 < n).then_some(i + 1),
        })
        .collect()
}

// ponytail: a stage is testing when its working column's name says so; a per-column role setting if boards outgrow it.
fn is_testing(p: &Project, s: &Stage) -> bool {
    let name = p.meta.columns[s.working].name.to_lowercase();
    name.contains("test") || name.contains("qa")
}

/// System prompt for `role`: what it does, which columns are its own, and the board guide.
fn instructions(p: &Project, role: Role) -> String {
    let all = stages(p);
    let mine: Vec<&Stage> = all
        .iter()
        .filter(|s| is_testing(p, s) == (role == Role::Tester))
        .collect();
    // Failed tests go back to where development starts.
    let rework = all.iter().find(|s| !is_testing(p, s)).map_or(0, |s| s.from);
    let label = |c: usize| p.column_label(c);

    let mut out = match role {
        Role::Developer => String::from(
            "You are the Developer for this project's GhostLane board. You pick up tickets that need building, \
             implement them and hand them on for testing.\n",
        ),
        Role::Tester => String::from(
            "You are the Tester for this project's GhostLane board. You check finished work against each \
             ticket's Test Scope and either pass it on or send it back. You don't fix the code yourself.\n",
        ),
    };
    out.push_str("\n## Your columns\n\n");
    if mine.is_empty() {
        out.push_str(
            "No column on this board is set up for you, so work on the ticket you are given and move it only \
             when asked.\n",
        );
    }
    for s in &mine {
        let finish = match (s.done, role) {
            (Some(done), Role::Developer) => format!("When the work is done, move it to {}.", label(done)),
            (Some(done), Role::Tester) => format!(
                "If it passes, move it to {}. If it fails, add the reason under `## Notes` and move it back to {}.",
                label(done),
                label(rework)
            ),
            (None, _) => "When you are done, leave it there.".to_string(),
        };
        out.push_str(&format!(
            "- Pick up tickets from {}. Before you start, move the ticket to {}. {finish}\n",
            label(s.from),
            label(s.working)
        ));
    }
    out.push_str("\n## Working on a ticket\n\n");
    out.push_str(
        "1. Read the whole ticket: the Description says what is wanted, the Test Scope how it is checked, and \
         the Notes hold earlier findings such as why a test failed.\n\
         2. Move the ticket to your working column before you start. GhostLane may already have moved it when \
         the play button was pressed; if so, leave it where it is.\n",
    );
    out.push_str(match role {
        Role::Developer => {
            "3. Build what the Description asks for, so that everything in the Test Scope will pass. Address \
             any failures recorded in the Notes.\n\
             4. Move the ticket on as described above. If you can't finish, add a note under `## Notes` saying \
             why and leave the ticket in your working column.\n"
        }
        Role::Tester => {
            "3. Check every point in the Test Scope: run the project's tests and try the behaviour described. \
             Don't change the code.\n\
             4. Move the ticket on as described above. When it fails, say exactly what failed and how to \
             reproduce it in the note.\n"
        }
    });
    out.push('\n');
    out.push_str(&p.ai_guide().replace("## GhostLane tickets", "## The board"));
    out
}

/// Path and contents of `role`'s agent file for AI CLI `cli`, or `None` when that CLI has no sub agents.
fn agent_file(p: &Project, cli: &str, role: Role) -> Option<(String, String)> {
    let (id, desc) = (role.id(), role.description());
    let body = instructions(p, role);
    let md = |path: String, extra: &str| {
        let text = format!(
            "---\n{extra}description: \"{desc}\"\n---\n\n<!-- {MARKER}: edits here are replaced when the project is saved. -->\n\n{body}"
        );
        Some((path, text))
    };
    let named = format!("name: {id}\n");
    match cli {
        "claude" => md(format!(".claude/agents/{id}.md"), &named),
        "gemini" => md(format!(".gemini/agents/{id}.md"), &format!("{named}kind: local\n")),
        "copilot" => md(format!(".github/agents/{id}.agent.md"), &named),
        "cursor" => md(format!(".cursor/agents/{id}.md"), &named),
        "opencode" => md(format!(".opencode/agents/{id}.md"), "mode: subagent\n"),
        "codex" => Some((
            format!(".codex/agents/{id}.toml"),
            format!(
                "# {MARKER}: edits here are replaced when the project is saved.\nname = \"{id}\"\ndescription = \"{desc}\"\n\
                 developer_instructions = '''\n{}'''\n",
                // A literal string can hold anything but its own closing quotes.
                body.replace("'''", "''")
            ),
        )),
        _ => None,
    }
}

/// Writes the Developer and Tester agents for AI CLI `cli` into the project. Returns the files left alone
/// because someone else made them.
pub fn write_agents(p: &Project, cli: &str) -> io::Result<Vec<PathBuf>> {
    let mut skipped = Vec::new();
    for role in [Role::Developer, Role::Tester] {
        let Some((rel, text)) = agent_file(p, cli, role) else {
            continue;
        };
        let path = p.root.join(rel);
        match fs::read_to_string(&path) {
            Ok(old) if old == text => continue,
            Ok(old) if !old.contains(MARKER) => {
                skipped.push(path);
                continue;
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        write_atomic(&path, &text)?;
    }
    Ok(skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{DEFAULT_AUTO_MOVES, DEFAULT_COLUMNS};

    fn default_project(dir: &std::path::Path) -> Project {
        let names: Vec<String> = DEFAULT_COLUMNS.iter().map(|s| s.to_string()).collect();
        let mut p = Project::create(dir, "P", &names).unwrap();
        for (c, m) in p.meta.columns.iter_mut().zip(DEFAULT_AUTO_MOVES) {
            c.auto_move = m;
        }
        p
    }

    #[test]
    fn default_board_splits_development_and_testing() {
        let d = tempfile::tempdir().unwrap();
        let p = default_project(d.path());
        let dev = instructions(&p, Role::Developer);
        assert!(dev.contains(
            "Pick up tickets from **To Do** (`tickets/to_do/`). Before you start, move the ticket to **In Dev** \
             (`tickets/in_dev/`). When the work is done, move it to **Ready For Test** (`tickets/ready_for_test/`)."
        ));
        assert!(!dev.contains("Pick up tickets from **Ready For Test**"));
        let test = instructions(&p, Role::Tester);
        assert!(test.contains(
            "Pick up tickets from **Ready For Test** (`tickets/ready_for_test/`). Before you start, move the ticket \
             to **In Test** (`tickets/in_test/`). If it passes, move it to **Done** (`tickets/done/`). If it fails, \
             add the reason under `## Notes` and move it back to **To Do** (`tickets/to_do/`)."
        ));
        assert!(test.contains("## The board") && test.contains("## Notes"));
    }

    #[test]
    fn writes_each_cli_format_and_keeps_files_it_did_not_make() {
        let d = tempfile::tempdir().unwrap();
        let mut p = default_project(d.path());
        assert!(write_agents(&p, "claude").unwrap().is_empty());
        let dev = fs::read_to_string(d.path().join(".claude/agents/developer.md")).unwrap();
        assert!(dev.starts_with("---\nname: developer\ndescription: \"Develops"));
        let front = dev.split("---\n").nth(1).unwrap();
        assert!(serde_yaml_ng::from_str::<serde_yaml_ng::Mapping>(front).is_ok());

        // Renaming a column rewrites the agents.
        p.meta.columns[1].name = "Building".into();
        write_agents(&p, "claude").unwrap();
        let dev = fs::read_to_string(d.path().join(".claude/agents/developer.md")).unwrap();
        assert!(dev.contains("**Building**"));

        // A tester someone wrote by hand stays.
        fs::write(d.path().join(".claude/agents/tester.md"), "mine").unwrap();
        let skipped = write_agents(&p, "claude").unwrap();
        assert_eq!(skipped, [d.path().join(".claude/agents/tester.md")]);
        assert_eq!(fs::read_to_string(&skipped[0]).unwrap(), "mine");

        write_agents(&p, "codex").unwrap();
        let toml = fs::read_to_string(d.path().join(".codex/agents/tester.toml")).unwrap();
        assert!(
            toml.contains("name = \"tester\"") && toml.contains("developer_instructions = '''\nYou are the Tester")
        );
        write_agents(&p, "opencode").unwrap();
        assert!(d.path().join(".opencode/agents/developer.md").is_file());
        assert!(agent_file(&p, "aider", Role::Developer).is_none());
    }
}
