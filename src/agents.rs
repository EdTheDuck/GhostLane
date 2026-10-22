//! Developer, Tester and Git sub agents for AI CLIs that support them, built from the project's columns, plus
//! listing, creating and changing the model of the sub agents in a project.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::ProjectSettings;
use crate::project::{AutoMove, Project};
use crate::ticket::write_atomic;

/// Files carrying this are GhostLane's to rewrite; anything else at the same path is left alone.
const MARKER: &str = "Managed by GhostLane";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Developer,
    Tester,
    Git,
}

impl Role {
    pub fn id(self) -> &'static str {
        match self {
            Role::Developer => "developer",
            Role::Tester => "tester",
            Role::Git => "git",
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
            Role::Git => {
                "Commits the work done on GhostLane tickets with git, and pushes it when the project is set to. Use \
                 when a ticket's changes need committing."
            }
        }
    }

    /// Model the role's agent starts with on AI CLI `cli`; `None` = the session's own. Developers get the strong
    /// default, testing and committing a cheaper one.
    pub fn default_model(self, cli: &str) -> Option<&'static str> {
        let cheap = self != Role::Developer;
        match (cli, cheap) {
            ("claude", false) => Some("claude-opus-5-5"),
            ("claude", true) => Some("claude-sonnet-5-5"),
            ("codex", false) => Some("gpt-5.5"),
            ("codex", true) => Some("gpt-5.4-mini"),
            ("gemini", true) => Some("gemini-3.5-flash"),
            ("copilot", false) => Some("Claude Opus 5.5"),
            ("copilot", true) => Some("Claude Sonnet 5.5"),
            // ponytail: Cursor and opencode models depend on the user's plan or provider, so they keep the
            // session's; add defaults once there's one that works for everyone.
            _ => None,
        }
    }
}

/// Where AI CLI `cli` keeps a project's sub agents and their file ending; `None` when it has none.
fn agent_dir(cli: &str) -> Option<(&'static str, &'static str)> {
    match cli {
        "claude" => Some((".claude/agents", ".md")),
        "gemini" => Some((".gemini/agents", ".md")),
        "copilot" => Some((".github/agents", ".agent.md")),
        "cursor" => Some((".cursor/agents", ".md")),
        "opencode" => Some((".opencode/agents", ".md")),
        "codex" => Some((".codex/agents", ".toml")),
        _ => None,
    }
}

pub fn has_agents(cli: &str) -> bool {
    agent_dir(cli).is_some()
}

fn agent_path(root: &Path, cli: &str, name: &str) -> Option<PathBuf> {
    agent_dir(cli).map(|(dir, ext)| root.join(dir).join(format!("{name}{ext}")))
}

/// Agent names end up in file names and prompts, and `.ghost` files can come from untrusted repos: lowercase
/// letters, digits, `-` and `_` only.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Model ids like `claude-sonnet-5-5`, `anthropic/claude-sonnet-5-5` or `Claude Sonnet 5.5`; nothing that could
/// break out of the YAML or TOML line it is written on.
pub fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.trim() == model
        && model.len() <= 100
        && !model.starts_with('-')
        && !model.contains(": ")
        && model.chars().all(|c| c.is_ascii_alphanumeric() || ".-_/: ".contains(c))
}

/// A sub agent the user adds in the project dialog. The file is theirs: GhostLane never rewrites it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewAgent {
    pub name: String,
    pub description: String,
    /// Empty = the session's model.
    pub model: String,
    pub instructions: String,
}

impl NewAgent {
    /// What's missing before it can be added, if anything.
    pub fn problem(&self) -> Option<&'static str> {
        if !valid_name(&self.name) {
            Some("Name: lowercase letters, digits, - and _")
        } else if !self.model.is_empty() && !valid_model(&self.model) {
            Some("Model: letters, digits, spaces and . - _ / : only")
        } else if self.description.trim().is_empty() {
            Some("Describe when to use it")
        } else if self.instructions.trim().is_empty() {
            Some("Write its instructions")
        } else {
            None
        }
    }
}

/// Sub agent changes made in the project dialog, applied when it's saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentEdits {
    pub new: Vec<NewAgent>,
    /// New models by agent name; empty = the session's model.
    pub models: Vec<(String, String)>,
}

/// A sub agent found in the project.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentInfo {
    pub name: String,
    /// `None` = the session's model.
    pub model: Option<String>,
    /// GhostLane made it and rewrites it when the project is saved.
    pub managed: bool,
}

/// The sub agents AI CLI `cli` would find in the project at `root`, by name.
pub fn list(root: &Path, cli: &str) -> Vec<AgentInfo> {
    let Some((dir, ext)) = agent_dir(cli) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(root.join(dir)) else {
        return Vec::new();
    };
    let mut found: Vec<AgentInfo> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().strip_suffix(ext)?.to_string();
            let text = fs::read_to_string(e.path()).ok()?;
            Some(AgentInfo {
                name,
                model: model_in(&text).map(str::to_string),
                managed: text.contains(MARKER),
            })
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// The header lines of an agent file: Markdown front matter, or TOML before the instructions.
fn header(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .enumerate()
        .take_while(|(i, l)| *i == 0 || (l.trim() != "---" && !l.starts_with("developer_instructions")))
        .map(|(_, l)| l)
}

/// `claude-sonnet-5-5` from `model: claude-sonnet-5-5` or `model = "gpt-5.4-mini"`.
fn model_value(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("model")?.trim_start();
    let value = rest.strip_prefix(':').or_else(|| rest.strip_prefix('='))?;
    Some(value.trim().trim_matches(['"', '\'']))
}

fn model_in(text: &str) -> Option<&str> {
    header(text)
        .find_map(model_value)
        .filter(|m| !m.is_empty() && *m != "inherit")
}

/// `text` with its model line set to `model` (`None` = no line, so the session's model), placed last in the
/// header. Text that isn't an agent file comes back unchanged.
fn with_model(text: &str, model: Option<&str>, toml: bool) -> String {
    if !toml && !text.starts_with("---") {
        return text.to_string();
    }
    let mut line = model.map(|m| {
        if toml {
            format!("model = \"{m}\"")
        } else {
            format!("model: {m}")
        }
    });
    let mut out = Vec::new();
    let mut in_header = true;
    for (i, l) in text.lines().enumerate() {
        if in_header && i > 0 && (l.trim() == "---" || l.starts_with("developer_instructions")) {
            out.extend(line.take());
            in_header = false;
        }
        if in_header && model_value(l).is_some() {
            continue;
        }
        out.push(l.to_string());
    }
    if in_header {
        return text.to_string();
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// Sets the model of sub agent `name` of AI CLI `cli`; empty = the session's own. A missing agent is skipped.
pub fn set_model(root: &Path, cli: &str, name: &str, model: &str) -> io::Result<()> {
    if !valid_name(name) || !(model.is_empty() || valid_model(model)) {
        return Err(io::Error::other(format!("\"{model}\" isn't a model name")));
    }
    let Some(path) = agent_path(root, cli, name) else {
        return Ok(());
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let toml = path.extension().is_some_and(|e| e == "toml");
    let new = with_model(&text, (!model.is_empty()).then_some(model), toml);
    if new == text { Ok(()) } else { write_atomic(&path, &new) }
}

/// Writes the user's new sub agent `a` for AI CLI `cli`. Refuses to replace a file that's already there.
pub fn create(root: &Path, cli: &str, a: &NewAgent) -> io::Result<()> {
    if let Some(problem) = a.problem() {
        return Err(io::Error::other(problem));
    }
    let Some(path) = agent_path(root, cli, &a.name) else {
        return Ok(());
    };
    if path.exists() {
        return Err(io::Error::other(format!("{} already exists", path.display())));
    }
    let model = (!a.model.is_empty()).then_some(a.model.as_str());
    let text = render(cli, &a.name, &a.description, model, a.instructions.trim(), false);
    write_atomic(&path, &(text + "\n"))
}

/// Agent file text for AI CLI `cli`. `managed` marks it as GhostLane's and asks for a high reasoning effort.
fn render(cli: &str, name: &str, description: &str, model: Option<&str>, body: &str, managed: bool) -> String {
    let desc = description.replace(['\n', '\r'], " ").replace('"', "'");
    if cli == "codex" {
        let mut out = String::new();
        if managed {
            out.push_str(&format!(
                "# {MARKER}: edits here are replaced when the project is saved.\n"
            ));
        }
        out.push_str(&format!("name = \"{name}\"\ndescription = \"{desc}\"\n"));
        if managed {
            out.push_str("model_reasoning_effort = \"high\"\n");
        }
        if let Some(m) = model {
            out.push_str(&format!("model = \"{m}\"\n"));
        }
        // A literal string can hold anything but its own closing quotes.
        out.push_str(&format!(
            "developer_instructions = '''\n{}'''\n",
            body.replace("'''", "''")
        ));
        return out;
    }
    let mut front = match cli {
        "opencode" => "mode: subagent\n".to_string(),
        "gemini" => format!("name: {name}\nkind: local\n"),
        _ => format!("name: {name}\n"),
    };
    front.push_str(&format!("description: \"{desc}\"\n"));
    if managed && cli == "claude" {
        front.push_str("effort: high\n");
    }
    if let Some(m) = model {
        front.push_str(&format!("model: {m}\n"));
    }
    let marker = if managed {
        format!("<!-- {MARKER}: edits here are replaced when the project is saved. -->\n\n")
    } else {
        String::new()
    };
    format!("---\n{front}---\n\n{marker}{body}")
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

/// The sub agent that works a stage: the one picked for its column, else the Tester when the column's name says
/// testing, else the Developer.
fn stage_agent<'a>(p: &'a Project, s: &Stage) -> &'a str {
    let c = &p.meta.columns[s.working];
    let name = c.name.to_lowercase();
    match &c.agent {
        Some(a) => a,
        None if name.contains("test") || name.contains("qa") => Role::Tester.id(),
        None => Role::Developer.id(),
    }
}

/// System prompt for `role`: what it does, which columns are its own, and the board guide. `push`: the Git agent
/// pushes its commits.
fn instructions(p: &Project, role: Role, push: bool) -> String {
    let all = stages(p);
    let mine: Vec<&Stage> = all.iter().filter(|s| stage_agent(p, s) == role.id()).collect();
    // Failed tests go back to where development starts.
    let rework = all
        .iter()
        .find(|s| stage_agent(p, s) == Role::Developer.id())
        .map_or(0, |s| s.from);
    let label = |c: usize| p.column_label(c);
    let (and_push, and_pushed, or_pushing) = if push {
        (" and push", " and pushed", " or pushing")
    } else {
        ("", "", "")
    };

    let mut out = match role {
        Role::Developer => String::from(
            "You are the Developer for this project's GhostLane board. You pick up tickets that need building, \
             implement them and hand them on for testing.\n",
        ),
        Role::Tester => String::from(
            "You are the Tester for this project's GhostLane board. You check finished work against each \
             ticket's Test Scope and either pass it on or send it back. You don't fix the code yourself.\n",
        ),
        Role::Git => format!(
            "You are the Git agent for this project's GhostLane board. You commit{and_push} the work done on \
             tickets. You don't change the code yourself.\n"
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
            (Some(done), Role::Git) => format!("Once the work is committed{and_pushed}, move it to {}.", label(done)),
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
    match role {
        Role::Developer => out.push_str(
            "3. Build what the Description asks for, so that everything in the Test Scope will pass. Address \
             any failures recorded in the Notes.\n\
             4. Move the ticket on as described above. If you can't finish, add a note under `## Notes` saying \
             why and leave the ticket in your working column.\n",
        ),
        Role::Tester => out.push_str(
            "3. Check every point in the Test Scope: run the project's tests and try the behaviour described. \
             Don't change the code.\n\
             4. Move the ticket on as described above. When it fails, say exactly what failed and how to \
             reproduce it in the note.\n",
        ),
        Role::Git => out.push_str(&format!(
            "3. Stage everything with `git add -A` and commit it with the ticket's number and title as the \
             message, like `00012: Add the login page`. {} Don't change, revert or leave out any files.\n\
             4. Move the ticket on as described above. If committing{or_pushing} fails, add the error under \
             `## Notes` and leave the ticket in your working column.\n",
            if push {
                "Then push it with `git push`."
            } else {
                "Don't push."
            }
        )),
    }
    out.push('\n');
    out.push_str(&p.ai_guide().replace("## GhostLane tickets", "## The board"));
    out
}

/// Writes the Developer and Tester agents for AI CLI `cli` into the project, and the Git agent when the project
/// commits. A model already set in a file stays; without one they get the role's default. Returns the files left alone because someone else made them.
pub fn write_agents(p: &Project, cli: &str, settings: &ProjectSettings) -> io::Result<Vec<PathBuf>> {
    let mut skipped = Vec::new();
    let push = settings.push && !settings.pull_request;
    let roles = [Role::Developer, Role::Tester, Role::Git];
    for role in roles.into_iter().filter(|&r| r != Role::Git || settings.commit) {
        let Some(path) = agent_path(&p.root, cli, role.id()) else {
            continue;
        };
        let old = match fs::read_to_string(&path) {
            Ok(old) => Some(old),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if old.as_ref().is_some_and(|o| !o.contains(MARKER)) {
            skipped.push(path);
            continue;
        }
        // Files from before models were set get the default too.
        let model = old.as_deref().and_then(model_in).or(role.default_model(cli));
        let body = instructions(p, role, push);
        let text = render(cli, role.id(), role.description(), model, &body, true);
        if old.as_deref() != Some(text.as_str()) {
            write_atomic(&path, &text)?;
        }
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
        let dev = instructions(&p, Role::Developer, false);
        assert!(dev.contains(
            "Pick up tickets from **To Do** (`tickets/to_do/`). Before you start, move the ticket to **In Dev** \
             (`tickets/in_dev/`). When the work is done, move it to **Ready For Test** (`tickets/ready_for_test/`)."
        ));
        assert!(!dev.contains("Pick up tickets from **Ready For Test**"));
        let test = instructions(&p, Role::Tester, false);
        assert!(test.contains(
            "Pick up tickets from **Ready For Test** (`tickets/ready_for_test/`). Before you start, move the ticket \
             to **In Test** (`tickets/in_test/`). If it passes, move it to **Done** (`tickets/done/`). If it fails, \
             add the reason under `## Notes` and move it back to **To Do** (`tickets/to_do/`)."
        ));
        assert!(test.contains("## The board") && test.contains("## Notes"));
    }

    #[test]
    fn a_column_picked_for_an_agent_is_its_own() {
        let d = tempfile::tempdir().unwrap();
        let mut p = default_project(d.path());
        p.meta.columns[3].agent = Some("git".into());
        let git = instructions(&p, Role::Git, true);
        assert!(git.contains(
            "Pick up tickets from **Ready For Test** (`tickets/ready_for_test/`). Before you start, move the ticket \
             to **In Test** (`tickets/in_test/`). Once the work is committed and pushed, move it to **Done**"
        ));
        assert!(git.contains("Then push it with `git push`."));
        assert!(instructions(&p, Role::Git, false).contains("Don't push."));
        assert!(!instructions(&p, Role::Tester, false).contains("Pick up tickets from"));
    }

    #[test]
    fn writes_each_cli_format_and_keeps_files_it_did_not_make() {
        let d = tempfile::tempdir().unwrap();
        let mut p = default_project(d.path());
        let s = ProjectSettings::default();
        assert!(write_agents(&p, "claude", &s).unwrap().is_empty());
        let dev = fs::read_to_string(d.path().join(".claude/agents/developer.md")).unwrap();
        assert!(dev.starts_with("---\nname: developer\ndescription: \"Develops"));
        let front = dev.split("---\n").nth(1).unwrap();
        assert!(serde_yaml_ng::from_str::<serde_yaml_ng::Mapping>(front).is_ok());
        assert!(front.contains("effort: high\nmodel: claude-opus-5-5\n"));
        let tester = fs::read_to_string(d.path().join(".claude/agents/tester.md")).unwrap();
        assert!(tester.contains("model: claude-sonnet-5-5\n"));
        assert!(!d.path().join(".claude/agents/git.md").exists(), "only when committing");

        // Renaming a column rewrites the agents.
        p.meta.columns[1].name = "Building".into();
        write_agents(&p, "claude", &s).unwrap();
        let dev = fs::read_to_string(d.path().join(".claude/agents/developer.md")).unwrap();
        assert!(dev.contains("**Building**"));

        // A tester someone wrote by hand stays.
        fs::write(d.path().join(".claude/agents/tester.md"), "mine").unwrap();
        let skipped = write_agents(&p, "claude", &s).unwrap();
        assert_eq!(skipped, [d.path().join(".claude/agents/tester.md")]);
        assert_eq!(fs::read_to_string(&skipped[0]).unwrap(), "mine");

        write_agents(&p, "codex", &s).unwrap();
        let toml = fs::read_to_string(d.path().join(".codex/agents/tester.toml")).unwrap();
        assert!(
            toml.contains("name = \"tester\"") && toml.contains("developer_instructions = '''\nYou are the Tester")
        );
        assert!(toml.contains("model_reasoning_effort = \"high\"\nmodel = \"gpt-5.4-mini\"\n"));
        write_agents(&p, "opencode", &s).unwrap();
        let open = fs::read_to_string(d.path().join(".opencode/agents/developer.md")).unwrap();
        assert!(!open.contains("model:"), "opencode keeps the session's model");
        assert!(agent_path(d.path(), "aider", "developer").is_none());
    }

    #[test]
    fn git_agent_comes_with_committing_and_uses_the_tester_model() {
        let d = tempfile::tempdir().unwrap();
        let p = default_project(d.path());
        let s = ProjectSettings {
            commit: true,
            ..Default::default()
        };
        write_agents(&p, "claude", &s).unwrap();
        let git = fs::read_to_string(d.path().join(".claude/agents/git.md")).unwrap();
        assert!(git.contains("model: claude-sonnet-5-5\n") && git.contains("Don't push."));
        assert_eq!(Role::Git.default_model("codex"), Role::Tester.default_model("codex"));
    }

    #[test]
    fn models_are_listed_changed_and_kept() {
        let d = tempfile::tempdir().unwrap();
        let p = default_project(d.path());
        let s = ProjectSettings::default();
        for cli in ["claude", "codex"] {
            write_agents(&p, cli, &s).unwrap();
            set_model(d.path(), cli, "developer", "my-model").unwrap();
            write_agents(&p, cli, &s).unwrap();
            let listed = list(d.path(), cli);
            assert_eq!(listed[0].name, "developer");
            assert_eq!(
                listed[0].model.as_deref(),
                Some("my-model"),
                "{cli}: survives a rewrite"
            );
            assert!(listed[0].managed);
            set_model(d.path(), cli, "developer", "").unwrap();
            write_agents(&p, cli, &s).unwrap();
            let back = list(d.path(), cli)[0].model.clone();
            assert_eq!(
                back.as_deref(),
                Role::Developer.default_model(cli),
                "{cli}: back to the default"
            );
        }
        assert!(set_model(d.path(), "claude", "tester", "x\"\nevil: 1").is_err());

        let new = NewAgent {
            name: "docs".into(),
            description: "Writes \"docs\"".into(),
            model: "claude-haiku-5-5".into(),
            instructions: "Write the docs.".into(),
        };
        create(d.path(), "claude", &new).unwrap();
        assert!(create(d.path(), "claude", &new).is_err(), "never replaces a file");
        let docs = list(d.path(), "claude").into_iter().find(|a| a.name == "docs").unwrap();
        assert_eq!(docs.model.as_deref(), Some("claude-haiku-5-5"));
        assert!(!docs.managed);
        let text = fs::read_to_string(d.path().join(".claude/agents/docs.md")).unwrap();
        let front = text.split("---\n").nth(1).unwrap();
        assert!(serde_yaml_ng::from_str::<serde_yaml_ng::Mapping>(front).is_ok());
        assert!(text.ends_with("Write the docs.\n"));
        let bad = NewAgent {
            name: "../x".into(),
            ..new
        };
        assert!(bad.problem().is_some() && create(d.path(), "claude", &bad).is_err());
    }
}
