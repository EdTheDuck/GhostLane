use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ticket::{self, Epic, Ticket, write_atomic};

pub const DEFAULT_COLUMNS: [&str; 5] = ["To Do", "In Dev", "Ready For Test", "In Test", "Done"];
/// Index-aligned with `DEFAULT_COLUMNS`.
pub const DEFAULT_AUTO_MOVES: [AutoMove; 5] = [
    AutoMove::Manual,
    AutoMove::AiWorking,
    AutoMove::AiComplete,
    AutoMove::AiWorking,
    AutoMove::AiComplete,
];
/// Play button prompts for the default columns; `None` = the one in Settings.
pub const DEFAULT_PROMPTS: [Option<&str>; 5] = [
    Some(
        "Move the ticket {ticket_path} to In Dev and then develop the ticket. When complete move the ticket to the \
         RFT column",
    ),
    None,
    Some(
        "Move the ticket {ticket_path} to the In Test column. Test the ticket with the Test Criteria. If it passes, \
         move the ticket to Done. If it fails, add the failure reason as a note on the ticket and move the ticket \
         back to the To Do column",
    ),
    None,
    None,
];
/// Sub agents working the default columns that tickets move into on AI Working.
pub const DEFAULT_AGENTS: [Option<&str>; 5] = [None, Some("developer"), None, Some("tester"), None];

/// What moves a ticket into a column from the column before it, without the user dragging it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoMove {
    #[default]
    #[serde(rename = "none")]
    Manual,
    /// The play button was pressed on a ticket in the column before.
    AiWorking,
    /// The AI finished a ticket in the column before.
    AiComplete,
}

impl AutoMove {
    pub const ALL: [AutoMove; 3] = [AutoMove::Manual, AutoMove::AiWorking, AutoMove::AiComplete];

    pub fn label(self) -> &'static str {
        match self {
            AutoMove::Manual => "None",
            AutoMove::AiWorking => "AI Working",
            AutoMove::AiComplete => "AI Complete",
        }
    }

    fn is_manual(&self) -> bool {
        *self == AutoMove::Manual
    }
}

/// Which ticket the columns set to Auto hand to the AI first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoOrder {
    PriorityAsc,
    #[default]
    PriorityDesc,
    IdAsc,
    IdDesc,
}

impl AutoOrder {
    pub const ALL: [AutoOrder; 4] = [
        AutoOrder::PriorityAsc,
        AutoOrder::PriorityDesc,
        AutoOrder::IdAsc,
        AutoOrder::IdDesc,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AutoOrder::PriorityAsc => "Priority Ascending",
            AutoOrder::PriorityDesc => "Priority Descending",
            AutoOrder::IdAsc => "Number Ascending",
            AutoOrder::IdDesc => "Number Descending",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    #[default]
    Id,
    Priority,
    Title,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sort {
    pub by: SortBy,
    pub dir: SortDir,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub folder: String,
    #[serde(default)]
    pub sort: Sort,
    #[serde(default, skip_serializing_if = "AutoMove::is_manual")]
    pub auto_move: AutoMove,
    /// Play button prompt for tickets in this column; `None` = the one in Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_prompt: Option<String>,
    /// Most tickets the column should hold; its count turns red past it and Auto won't fill it further.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wip_limit: Option<u32>,
    /// Sub agent that works tickets moved in on AI Working; `None` = the session itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// Markers around the ticket guide GhostLane keeps in an AI CLI's instructions file.
const GUIDE_START: &str = "<!-- ghostlane:start -->";
const GUIDE_END: &str = "<!-- ghostlane:end -->";

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GhostFile {
    #[serde(default = "one")]
    pub version: u32,
    pub name: String,
    #[serde(default = "one")]
    pub next_ticket_id: u32,
    #[serde(default)]
    pub columns: Vec<Column>,
    /// AI CLI id the play button uses for this project (see `terminal::AI_CLIS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_cli: Option<String>,
}

/// A column as edited in the project modal. `orig_folder` links it to an existing column.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColumnEdit {
    pub name: String,
    pub orig_folder: Option<String>,
    pub auto_move: AutoMove,
    pub ai_prompt: Option<String>,
    pub wip_limit: Option<u32>,
    pub agent: Option<String>,
}

pub struct Project {
    pub ghost_path: PathBuf,
    pub root: PathBuf,
    pub meta: GhostFile,
    /// Index-aligned with `meta.columns`.
    pub tickets: Vec<Vec<Ticket>>,
    /// By number.
    pub epics: Vec<Epic>,
}

fn err(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

/// Column name -> portable folder name. `None` when nothing usable remains.
pub fn folder_name(name: &str) -> Option<String> {
    let lower = name.trim().to_lowercase();
    let joined = lower.split_whitespace().collect::<Vec<_>>().join("_");
    let mut kept = String::new();
    for c in joined.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-') {
        if !(c == '_' && kept.ends_with('_')) {
            kept.push(c);
        }
    }
    let mut s = kept.trim_matches(|c| c == '_' || c == '-').to_string();
    if s.is_empty() {
        return None;
    }
    let reserved = matches!(s.as_str(), "con" | "prn" | "aux" | "nul")
        || ((s.starts_with("com") || s.starts_with("lpt")) && s.len() == 4 && matches!(s.as_bytes()[3], b'1'..=b'9'));
    if reserved {
        s.push('_');
    }
    Some(s)
}

/// Validates column names; returns their folder names in order.
pub fn validate_columns(names: &[String]) -> Result<Vec<String>, String> {
    if names.is_empty() {
        return Err("A project needs at least one column".into());
    }
    let mut seen = HashSet::new();
    let mut folders = Vec::new();
    for name in names {
        let folder = folder_name(name).ok_or_else(|| format!("\"{name}\" is not a valid column name"))?;
        if !seen.insert(folder.clone()) {
            return Err(format!("Two columns would both use the folder \"{folder}\""));
        }
        folders.push(folder);
    }
    Ok(folders)
}

impl Project {
    pub fn load(ghost_path: &Path) -> io::Result<Project> {
        // Absolute, so the root (also the terminal's working folder) never comes out empty.
        let ghost_path = &std::path::absolute(ghost_path)?;
        let text = fs::read_to_string(ghost_path)?;
        let mut meta: GhostFile = serde_yaml_ng::from_str(&text)
            .map_err(|e| err(format!("{} is not a valid .ghost file: {e}", ghost_path.display())))?;
        // Folders end up in paths and shell commands, and .ghost files can come from untrusted repos:
        // only accept what `folder_name` would have produced.
        let mut seen = HashSet::new();
        for c in &meta.columns {
            if folder_name(&c.folder).as_deref() != Some(c.folder.as_str()) || !seen.insert(&c.folder) {
                return Err(err(format!(
                    "{} has an invalid column folder \"{}\"",
                    ghost_path.display(),
                    c.folder.escape_default()
                )));
            }
        }
        // Agent names go into prompts; drop any that aren't plain names.
        for c in &mut meta.columns {
            if c.agent.as_deref().is_some_and(|a| !crate::agents::valid_name(a)) {
                c.agent = None;
            }
        }
        let root = ghost_path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut p = Project {
            ghost_path: ghost_path.to_path_buf(),
            root,
            meta,
            tickets: Vec::new(),
            epics: Vec::new(),
        };
        p.rescan()?;
        Ok(p)
    }

    /// Creates `.ghost` + `tickets/<columns>` directly inside `dir`.
    pub fn create(dir: &Path, name: &str, column_names: &[String]) -> io::Result<Project> {
        let name = name.trim();
        if name.is_empty() {
            return Err(err("Project name is required"));
        }
        if let Some(existing) = find_ghost_file(dir) {
            return Err(err(format!(
                "{} already contains a project ({})",
                dir.display(),
                existing.display()
            )));
        }
        let folders = validate_columns(column_names).map_err(err)?;
        let slug = folder_name(name).unwrap_or_else(|| "project".into());
        let columns = column_names
            .iter()
            .zip(folders)
            .map(|(n, f)| Column {
                name: n.trim().to_string(),
                folder: f,
                sort: Sort::default(),
                auto_move: AutoMove::Manual,
                ai_prompt: None,
                wip_limit: None,
                agent: None,
            })
            .collect();
        let meta = GhostFile {
            version: 1,
            name: name.to_string(),
            next_ticket_id: 1,
            columns,
            ai_cli: None,
        };
        let ghost_path = dir.join(format!("{slug}.ghost"));
        let p = Project {
            ghost_path: ghost_path.clone(),
            root: dir.to_path_buf(),
            meta,
            tickets: Vec::new(),
            epics: Vec::new(),
        };
        for i in 0..p.meta.columns.len() {
            fs::create_dir_all(p.column_dir(i))?;
        }
        p.save_meta()?;
        Project::load(&ghost_path)
    }

    pub fn tickets_dir(&self) -> PathBuf {
        self.root.join("tickets")
    }

    pub fn column_dir(&self, col: usize) -> PathBuf {
        self.tickets_dir().join(&self.meta.columns[col].folder)
    }

    /// Where epics live, beside `tickets/` so it can never clash with a column folder. Only made once one is.
    pub fn epics_dir(&self) -> PathBuf {
        self.root.join("epics")
    }

    pub fn save_meta(&self) -> io::Result<()> {
        let yaml = serde_yaml_ng::to_string(&self.meta).map_err(|e| err(e.to_string()))?;
        write_atomic(&self.ghost_path, &yaml)
    }

    /// Re-reads every column folder (recreating missing ones) and bumps `next_ticket_id`
    /// past the highest ID on disk.
    // ponytail: full rescan on every change; fine up to thousands of tickets.
    pub fn rescan(&mut self) -> io::Result<()> {
        let mut all = Vec::with_capacity(self.meta.columns.len());
        let mut max_id = 0;
        for col in 0..self.meta.columns.len() {
            let dir = self.column_dir(col);
            fs::create_dir_all(&dir)?;
            let mut tickets = Vec::new();
            for entry in fs::read_dir(&dir)? {
                let path = entry?.path();
                if !path.is_file() {
                    continue;
                }
                match Ticket::load(&path) {
                    Ok(Some(t)) => {
                        max_id = max_id.max(t.id);
                        tickets.push(t);
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("skipping {}: {e}", path.display()),
                }
            }
            let sort = self.meta.columns[col].sort;
            ticket::sort(&mut tickets, sort.by, sort.dir);
            all.push(tickets);
        }
        self.tickets = all;
        // Epics count from the same numbers, so a number always means one thing.
        let mut epics = Vec::new();
        if let Ok(entries) = fs::read_dir(self.epics_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                match Epic::load(&path) {
                    Ok(Some(e)) if path.is_file() => {
                        max_id = max_id.max(e.id);
                        epics.push(e);
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("skipping {}: {e}", path.display()),
                }
            }
        }
        epics.sort_by_key(|e| e.id);
        self.epics = epics;
        self.meta.next_ticket_id = self.meta.next_ticket_id.max(max_id + 1);
        Ok(())
    }

    pub fn find_epic(&self, id: u32) -> Option<&Epic> {
        self.epics.iter().find(|e| e.id == id)
    }

    /// (column, ticket) of epic `id`'s tickets, in board order.
    pub fn epic_tickets(&self, id: u32) -> Vec<(usize, &Ticket)> {
        self.tickets
            .iter()
            .enumerate()
            .flat_map(|(c, ts)| ts.iter().map(move |t| (c, t)))
            .filter(|(_, t)| t.epic == Some(id))
            .collect()
    }

    /// (done, total) tickets of epic `id`; done = in the last column.
    pub fn epic_progress(&self, id: u32) -> (usize, usize) {
        let tickets = self.epic_tickets(id);
        let done = tickets.iter().filter(|(c, _)| *c == self.last_col()).count();
        (done, tickets.len())
    }

    /// The first number no ticket or epic file uses yet, from `next_ticket_id` on.
    fn free_id(&self) -> u32 {
        let name = |id| format!("{}.md", ticket::format_id(id));
        let mut id = self.meta.next_ticket_id;
        while (0..self.meta.columns.len()).any(|c| self.column_dir(c).join(name(id)).exists())
            || self.epics_dir().join(name(id)).exists()
        {
            id += 1;
        }
        id
    }

    /// Writes a new epic with the next free number. Returns the saved epic.
    pub fn add_epic(&mut self, mut e: Epic) -> io::Result<Epic> {
        let id = self.free_id();
        e.id = id;
        e.path = self.epics_dir().join(format!("{}.md", ticket::format_id(id)));
        e.malformed = false;
        e.save()?;
        self.meta.next_ticket_id = id + 1;
        self.save_meta()?;
        self.rescan()?;
        Ok(e)
    }

    pub fn set_sort(&mut self, col: usize, sort: Sort) -> io::Result<()> {
        self.meta.columns[col].sort = sort;
        ticket::sort(&mut self.tickets[col], sort.by, sort.dir);
        self.save_meta()
    }

    /// (column, id) of the ticket Auto works on next, from the columns whose folder is in `auto` and whose
    /// tickets move on "AI Working": first by `order`, then the right-most column, then the lowest id.
    /// Tickets at `run_limit` are skipped, and so are columns whose next column is at its WIP limit.
    pub fn next_auto(&self, auto: &HashSet<String>, order: AutoOrder, run_limit: u32) -> Option<(usize, u32)> {
        self.meta
            .columns
            .iter()
            .enumerate()
            .filter(|(c, col)| {
                auto.contains(&col.folder)
                    && self
                        .auto_target(*c, AutoMove::AiWorking)
                        .is_some_and(|to| !self.is_full(to))
            })
            .flat_map(|(c, _)| self.tickets.get(c).into_iter().flatten().map(move |t| (c, t)))
            .filter(|(_, t)| !t.at_run_limit(run_limit) && self.blockers(t).is_empty())
            .min_by(|(ca, a), (cb, b)| {
                let ord = match order {
                    AutoOrder::PriorityAsc => a.priority.cmp(&b.priority),
                    AutoOrder::PriorityDesc => b.priority.cmp(&a.priority),
                    AutoOrder::IdAsc => a.id.cmp(&b.id),
                    AutoOrder::IdDesc => b.id.cmp(&a.id),
                };
                ord.then(cb.cmp(ca)).then(a.id.cmp(&b.id))
            })
            .map(|(c, t)| (c, t.id))
    }

    /// Does column `col` hold as many tickets as its WIP limit allows, or more?
    pub fn is_full(&self, col: usize) -> bool {
        let count = self.tickets.get(col).map_or(0, Vec::len);
        self.meta.columns[col]
            .wip_limit
            .is_some_and(|n| n > 0 && count >= n as usize)
    }

    /// The last column, where finished tickets go.
    pub fn last_col(&self) -> usize {
        self.meta.columns.len().saturating_sub(1)
    }

    /// The tickets `t` waits for: those in its `blocked_by` that exist and aren't in the last column yet.
    pub fn blockers(&self, t: &Ticket) -> Vec<u32> {
        t.blocked_by
            .iter()
            .copied()
            .filter(|&b| self.find_ticket(b).is_some_and(|(c, _)| c != self.last_col()))
            .collect()
    }

    /// Every label on the board, sorted; spellings that differ only in case count once.
    pub fn labels(&self) -> Vec<String> {
        let mut all: Vec<String> = Vec::new();
        for l in self.tickets.iter().flatten().flat_map(|t| &t.labels) {
            if !all.iter().any(|a| a.to_lowercase() == l.to_lowercase()) {
                all.push(l.clone());
            }
        }
        all.sort_by_key(|l| l.to_lowercase());
        all
    }

    /// Tickets that list `id` in their `blocked_by`, in board order.
    pub fn dependents(&self, id: u32) -> Vec<u32> {
        self.tickets
            .iter()
            .flatten()
            .filter(|t| t.blocked_by.contains(&id))
            .map(|t| t.id)
            .collect()
    }

    /// Is ticket `id` waiting for other tickets?
    pub fn is_blocked(&self, id: u32) -> bool {
        self.find_ticket(id).is_some_and(|(_, t)| !self.blockers(t).is_empty())
    }

    /// (column, ticket) of ticket `id`.
    pub fn find_ticket(&self, id: u32) -> Option<(usize, &Ticket)> {
        self.tickets
            .iter()
            .enumerate()
            .find_map(|(c, ts)| ts.iter().find(|t| t.id == id).map(|t| (c, t)))
    }

    /// The column after `col`, when tickets move into it on `trigger`.
    pub fn auto_target(&self, col: usize, trigger: AutoMove) -> Option<usize> {
        (self.meta.columns.get(col + 1)?.auto_move == trigger).then_some(col + 1)
    }

    pub fn move_ticket(&mut self, path: &Path, to_col: usize) -> io::Result<()> {
        let file = path.file_name().ok_or_else(|| err("bad ticket path"))?;
        let target = self.column_dir(to_col).join(file);
        if target == path {
            return Ok(());
        }
        if target.exists() {
            return Err(err(format!("{} already exists", target.display())));
        }
        fs::rename(path, &target)?;
        self.rescan()
    }

    /// Writes a new ticket into `col` with the next free ID. Returns the saved ticket.
    pub fn add_ticket(&mut self, col: usize, mut t: Ticket) -> io::Result<Ticket> {
        let id = self.free_id();
        t.id = id;
        t.path = self.column_dir(col).join(format!("{}.md", ticket::format_id(id)));
        t.malformed = false;
        t.save()?;
        self.meta.next_ticket_id = id + 1;
        self.save_meta()?;
        self.rescan()?;
        Ok(t)
    }

    /// Applies an edited column list: renames, adds, removes (moving tickets of removed
    /// columns into `removed_targets[old_folder]`, an index into `edits`), and reorders.
    /// Emptied folders are deleted; anything left behind goes to the OS trash.
    /// On error, steps already done stay done.
    pub fn apply_columns(
        &mut self,
        name: &str,
        edits: &[ColumnEdit],
        removed_targets: &HashMap<String, usize>,
    ) -> io::Result<()> {
        let names: Vec<String> = edits.iter().map(|e| e.name.clone()).collect();
        let folders = validate_columns(&names).map_err(err)?;
        let dir = self.tickets_dir();
        let tmp = |f: &str| dir.join(format!(".{f}.ghostlane_tmp"));

        let kept: HashMap<&str, usize> = edits
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.orig_folder.as_deref().map(|f| (f, i)))
            .collect();
        let old_cols = self.meta.columns.clone();

        // 1. Park every folder that is renamed or removed under a temp name.
        for c in &old_cols {
            let unchanged = kept.get(c.folder.as_str()).is_some_and(|&i| folders[i] == c.folder);
            if !unchanged && dir.join(&c.folder).exists() {
                fs::rename(dir.join(&c.folder), tmp(&c.folder))?;
            }
        }
        // 2. Bring renamed folders back under their new names; create new ones.
        for (e, folder) in edits.iter().zip(&folders) {
            let target = dir.join(folder);
            match &e.orig_folder {
                Some(orig) if orig != folder && tmp(orig).exists() => fs::rename(tmp(orig), &target)?,
                _ => fs::create_dir_all(&target)?,
            }
        }
        // 3. Empty removed folders into their target column, then trash them.
        for c in &old_cols {
            if kept.contains_key(c.folder.as_str()) || !tmp(&c.folder).exists() {
                continue;
            }
            let parked = tmp(&c.folder);
            if let Some(&target) = removed_targets.get(&c.folder) {
                let target_dir = dir.join(&folders[target]);
                for entry in fs::read_dir(&parked)? {
                    let from = entry?.path();
                    let to = target_dir.join(from.file_name().unwrap_or_default());
                    if to.exists() {
                        return Err(err(format!("{} already exists", to.display())));
                    }
                    fs::rename(&from, &to)?;
                }
            }
            if fs::remove_dir(&parked).is_err() {
                trash::delete(&parked).map_err(|e| err(format!("Couldn't remove {}: {e}", parked.display())))?;
            }
        }
        // 4. Save the new column list, keeping each kept column's sort.
        self.meta.name = name.trim().to_string();
        self.meta.columns = edits
            .iter()
            .zip(folders)
            .map(|(e, folder)| {
                let sort = e
                    .orig_folder
                    .as_ref()
                    .and_then(|o| old_cols.iter().find(|c| &c.folder == o))
                    .map(|c| c.sort)
                    .unwrap_or_default();
                Column {
                    name: e.name.trim().to_string(),
                    folder,
                    sort,
                    auto_move: e.auto_move,
                    ai_prompt: e.ai_prompt.clone(),
                    wip_limit: e.wip_limit.filter(|&n| n > 0),
                    agent: e.agent.clone(),
                }
            })
            .collect();
        self.save_meta()?;
        self.rescan()
    }
}

impl Project {
    /// "**Name** (`tickets/folder/`)" for column `col`, on one line.
    pub fn column_label(&self, col: usize) -> String {
        let c = &self.meta.columns[col];
        format!("**{}** (`tickets/{}/`)", c.name.replace(['\n', '\r'], " "), c.folder)
    }

    /// Markdown telling an AI CLI how this project's tickets and columns work.
    pub fn ai_guide(&self) -> String {
        let mut columns = String::new();
        for (i, c) in self.meta.columns.iter().enumerate() {
            let arrives = match (i.checked_sub(1).map(|p| self.column_label(p)), c.auto_move) {
                (Some(prev), AutoMove::AiWorking) => {
                    format!(" (GhostLane moves tickets here from {prev} when the AI starts on them)")
                }
                (Some(prev), AutoMove::AiComplete) => {
                    format!(" (GhostLane moves tickets here from {prev} when the AI finishes them)")
                }
                _ => String::new(),
            };
            columns.push_str(&format!("{}. {}{arrives}\n", i + 1, self.column_label(i)));
        }
        let ghost = self.ghost_path.file_name().unwrap_or_default().to_string_lossy();
        format!(
            r#"## GhostLane tickets

This project's tickets are managed with GhostLane, a board that keeps every ticket as a Markdown file. The folder a ticket sits in is its column, and GhostLane picks up changes on disk straight away.

### Columns

In board order, with each column's folder:

{columns}
To move a ticket to another column, move its file into that column's folder and keep the file name. A ticket lives in exactly one folder, so never copy it. Columns can be referred to by name or by initials (for example "RFT" for "Ready For Test").

### Ticket files

- The file name is the ticket's ID, zero-padded to five digits: `00042.md`. Never rename a ticket file.
- A new ticket takes the next unused ID: one higher than the highest ID in any column folder. IDs are never reused.
- Every ticket has this layout:

```markdown
---
id: '00042'
title: Short title
priority: medium
run_count: 0
---

## Description

What needs doing and why.

## Test Scope

How to check it is done.

## Notes

Anything else worth keeping, such as why a test failed.
```

- `priority` is one of `low`, `medium`, `high` or `highest`.
- Optional `labels` lists short tags for grouping and filtering, for example `labels: [ui, backend]`.
- Optional `blocked_by` lists the IDs of tickets that must reach the last column first, for example `blocked_by: ['00012', '00015']`. GhostLane won't start a blocked ticket from its queue or an Auto column.
- Optional `epic` names the epic the ticket belongs to, for example `epic: '00040'`. A ticket belongs to at most one epic.
- `## Test Scope` and `## Notes` are optional; leave a section out when it is empty.
- Leave `id`, `run_count`, `done_at` and `runs` as they are; GhostLane maintains them.
- To add a note, append it under `## Notes` (create the section if it is missing) rather than changing the description.
- Don't edit `{ghost}`; it holds the board's columns and settings.

### Epics

An epic groups tickets working towards one bigger goal. Each is a file in `epics/` named with its ID, which comes from the same numbers as tickets (so never reuse a ticket's ID): front matter with `id` and `title`, then a `## Description` of the goal. Read a ticket's epic for the bigger picture before working on it. Progress is worked out from where its tickets are, so don't record it in the epic.
"#
        )
    }

    /// Puts `ai_guide` into `file` (relative to the root) between GhostLane's markers, replacing the
    /// previous guide and keeping the rest of the file. Creates the file if needed.
    pub fn write_ai_guide(&self, file: &str) -> io::Result<()> {
        let path = self.root.join(file);
        let existing = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let updated = replace_guide(&existing, &self.ai_guide());
        if updated == existing {
            return Ok(());
        }
        write_atomic(&path, &updated)
    }
}

/// Comment GhostLane puts above the lines it adds to `.gitignore`.
const IGNORE_COMMENT: &str = "# GhostLane board";

impl Project {
    /// `.gitignore` lines that keep the board (this .ghost file and the tickets folder) out of git.
    fn ignore_lines(&self) -> [String; 2] {
        let ghost = self.ghost_path.file_name().unwrap_or_default().to_string_lossy();
        [format!("/{ghost}"), "/tickets/".to_string()]
    }

    /// Does the project's `.gitignore` leave out both the .ghost file and the tickets folder?
    pub fn git_excluded(&self) -> bool {
        let text = fs::read_to_string(self.root.join(".gitignore")).unwrap_or_default();
        self.ignore_lines()
            .iter()
            .all(|entry| text.lines().any(|l| same_ignore(l, entry)))
    }

    /// Adds the .ghost file and tickets folder to the project's `.gitignore`, or takes them out again. Touches no
    /// other line; a `.gitignore` left empty is removed.
    pub fn set_git_excluded(&self, excluded: bool) -> io::Result<()> {
        let path = self.root.join(".gitignore");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let updated = toggle_ignore(&text, &self.ignore_lines(), excluded);
        if updated == text {
            Ok(())
        } else if updated.trim().is_empty() {
            fs::remove_file(&path)
        } else {
            write_atomic(&path, &updated)
        }
    }
}

/// Is `.gitignore` line `line` the same pattern as `entry`, give or take its leading and trailing slashes?
fn same_ignore(line: &str, entry: &str) -> bool {
    let bare = |s: &str| s.trim().trim_start_matches('/').trim_end_matches('/').to_string();
    !line.trim_start().starts_with('#') && bare(line) == bare(entry)
}

/// `.gitignore` text with `entries` added (those missing, under GhostLane's comment) or removed (with the comment).
fn toggle_ignore(text: &str, entries: &[String], excluded: bool) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    if excluded {
        let missing: Vec<&String> = entries
            .iter()
            .filter(|e| !text.lines().any(|l| same_ignore(l, e)))
            .collect();
        if missing.is_empty() {
            return text.to_string();
        }
        let mut out = text.trim_end().to_string();
        if !out.is_empty() {
            out.push_str(newline);
            out.push_str(newline);
        }
        out.push_str(IGNORE_COMMENT);
        for e in missing {
            out.push_str(newline);
            out.push_str(e);
        }
        out.push_str(newline);
        return out;
    }
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| l.trim() != IGNORE_COMMENT && !entries.iter().any(|e| same_ignore(l, e)))
        .collect();
    if kept.len() == text.lines().count() {
        return text.to_string();
    }
    let mut out = kept.join(newline).trim_end().to_string();
    if !out.is_empty() {
        out.push_str(newline);
    }
    out
}

/// `text` with the block between GhostLane's markers set to `guide`, appended when there is none yet.
fn replace_guide(text: &str, guide: &str) -> String {
    let block = format!("{GUIDE_START}\n{}\n{GUIDE_END}", guide.trim());
    if let (Some(start), Some(end)) = (text.find(GUIDE_START), text.find(GUIDE_END))
        && start < end
    {
        return format!("{}{block}{}", &text[..start], &text[end + GUIDE_END.len()..]);
    }
    match text.trim_end() {
        "" => format!("{block}\n"),
        rest => format!("{rest}\n\n{block}\n"),
    }
}

/// Days the stats chart covers.
pub const STATS_DAYS: usize = 14;

/// How a project's tickets have been going, from their run histories.
#[derive(Debug, Default, PartialEq)]
pub struct Stats {
    /// Tickets in the last column by the UTC day they got there, oldest first; the last entry is today.
    pub done_per_day: [u32; STATS_DAYS],
    pub runs: usize,
    pub avg_run_secs: Option<u64>,
    /// (done without ever coming back or failing the test command, done tickets the AI worked on).
    pub first_try: (usize, usize),
    /// (id, title, run count) of the tickets sent to the AI most often, more than once, most first.
    pub most_retried: Vec<(u32, String, u32)>,
    /// Tokens used by the runs that reported them.
    pub tokens: ticket::Tokens,
    pub runs_with_tokens: usize,
    /// (id, title, done, total, tokens) of each epic.
    pub epics: Vec<(u32, String, usize, usize, u64)>,
}

impl Project {
    pub fn stats(&self, now: u64) -> Stats {
        let mut s = Stats::default();
        let today = now / 86_400;
        let all = || self.tickets.iter().flatten();
        let runs: Vec<&ticket::RunRecord> = all().flat_map(|t| &t.runs).collect();
        s.runs = runs.len();
        s.avg_run_secs = (!runs.is_empty()).then(|| runs.iter().map(|r| r.seconds).sum::<u64>() / runs.len() as u64);
        for tokens in runs.iter().filter_map(|r| r.tokens) {
            s.tokens.add(tokens);
            s.runs_with_tokens += 1;
        }
        for t in self.tickets.get(self.last_col()).into_iter().flatten() {
            if let Some(ago) = t.done_at.and_then(|d| today.checked_sub(d / 86_400))
                && (ago as usize) < STATS_DAYS
            {
                s.done_per_day[STATS_DAYS - 1 - ago as usize] += 1;
            }
            if !t.runs.is_empty() {
                s.first_try.1 += 1;
                let bounced = |r: &ticket::RunRecord| {
                    matches!(r.result, ticket::RunResult::SentBack | ticket::RunResult::TestsFailed)
                };
                s.first_try.0 += usize::from(!t.runs.iter().any(bounced));
            }
        }
        let mut retried: Vec<&Ticket> = all().filter(|t| t.run_count > 1).collect();
        retried.sort_by(|a, b| b.run_count.cmp(&a.run_count).then(a.id.cmp(&b.id)));
        s.most_retried = retried
            .iter()
            .take(5)
            .map(|t| (t.id, t.title.clone(), t.run_count))
            .collect();
        s.epics = self
            .epics
            .iter()
            .map(|e| {
                let (done, total) = self.epic_progress(e.id);
                let tokens = self
                    .epic_tickets(e.id)
                    .iter()
                    .flat_map(|(_, t)| &t.runs)
                    .filter_map(|r| r.tokens)
                    .map(|t| t.total())
                    .sum();
                (e.id, e.title.clone(), done, total, tokens)
            })
            .collect();
        s
    }
}

impl Project {
    /// Every ticket as a CSV row, in board order, with a header row.
    pub fn export_csv(&self) -> String {
        let mut out = String::from("id,title,column,priority,labels,run_count,blocked_by,epic,done_at\r\n");
        for (col, tickets) in self.tickets.iter().enumerate() {
            for t in tickets {
                let blocked: Vec<String> = t.blocked_by.iter().map(|&b| ticket::format_id(b)).collect();
                let row = [
                    t.id_str(),
                    t.title.clone(),
                    self.meta.columns[col].name.clone(),
                    t.priority.label().to_string(),
                    t.labels.join("; "),
                    t.run_count.to_string(),
                    blocked.join("; "),
                    t.epic.map(ticket::format_id).unwrap_or_default(),
                    t.done_at.map(ticket::utc_timestamp).unwrap_or_default(),
                ];
                let row: Vec<String> = row.iter().map(|f| csv_field(f)).collect();
                out.push_str(&row.join(","));
                out.push_str("\r\n");
            }
        }
        out
    }

    /// A Markdown summary of the board, column by column, as of `now`.
    pub fn export_markdown(&self, now: u64) -> String {
        let one_line = |s: &str| s.replace(['\n', '\r'], " ");
        let mut out = format!(
            "# {}\n\nExported {}.\n",
            one_line(&self.meta.name),
            ticket::readable_utc(&ticket::utc_timestamp(now))
        );
        for (col, tickets) in self.tickets.iter().enumerate() {
            let name = one_line(&self.meta.columns[col].name);
            out.push_str(&format!("\n## {name} ({})\n\n", tickets.len()));
            if tickets.is_empty() {
                out.push_str("Nothing here.\n");
            }
            for t in tickets {
                let mut line = format!("- **{}** {} · {}", t.id_str(), one_line(&t.title), t.priority.label());
                for l in &t.labels {
                    line.push_str(&format!(" `{}`", l.replace('`', "")));
                }
                if let Some(e) = t.epic.and_then(|e| self.find_epic(e)) {
                    line.push_str(&format!(" · epic {} {}", e.id_str(), one_line(&e.title)));
                }
                let blockers: Vec<String> = self.blockers(t).iter().map(|&b| ticket::format_id(b)).collect();
                if !blockers.is_empty() {
                    line.push_str(&format!(" · blocked by {}", blockers.join(", ")));
                }
                if let Some(done) = t.done_at.filter(|_| col == self.last_col()) {
                    line.push_str(&format!(" · done {}", &ticket::utc_timestamp(done)[..10]));
                }
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }
}

/// One CSV field, quoted when needed. Text a spreadsheet would run as a formula gets a leading `'`, since ticket
/// files can come from untrusted repos.
fn csv_field(field: &str) -> String {
    let field = if field.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{field}")
    } else {
        field.to_string()
    };
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field
    }
}

pub fn find_ghost_file(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.is_file() && p.extension().is_some_and(|x| x == "ghost"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epics_share_ticket_numbers_and_count_progress() {
        let d = tempfile::tempdir().unwrap();
        let mut p = Project::create(d.path(), "P", &s(&["To Do", "Done"])).unwrap();
        assert!(!p.epics_dir().exists(), "made only once an epic is");
        let e = p
            .add_epic(Epic {
                title: "Big goal".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(e.id, 1);
        let mut t = Ticket {
            title: "Part".into(),
            epic: Some(e.id),
            ..Default::default()
        };
        let a = p.add_ticket(0, t.clone()).unwrap();
        assert_eq!(a.id, 2, "numbers are shared");
        t.title = "Other part".into();
        let b = p.add_ticket(0, t).unwrap();
        p.move_ticket(&b.path, 1).unwrap();
        assert_eq!(p.epic_progress(e.id), (1, 2));
        assert_eq!(p.epic_tickets(e.id).len(), 2);

        // An epic file written by hand is picked up, and its number is never handed out again.
        std::fs::write(p.epics_dir().join("00009.md"), "---\ntitle: By hand\n---\n").unwrap();
        p.rescan().unwrap();
        assert_eq!(p.find_epic(9).map(|e| e.title.as_str()), Some("By hand"));
        assert_eq!(p.meta.next_ticket_id, 10);
        assert!(p.ai_guide().contains("### Epics"));
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name("To Do").as_deref(), Some("to_do"));
        assert_eq!(folder_name("Ready For Test").as_deref(), Some("ready_for_test"));
        assert_eq!(folder_name("QA / Review?").as_deref(), Some("qa_review"));
        assert_eq!(folder_name("  Über  Done ").as_deref(), Some("über_done"));
        assert_eq!(folder_name("CON").as_deref(), Some("con_"));
        assert_eq!(folder_name("com1").as_deref(), Some("com1_"));
        assert_eq!(folder_name("com10").as_deref(), Some("com10"));
        assert_eq!(folder_name("!!!"), None);
        assert_eq!(folder_name(""), None);
    }

    #[test]
    fn column_validation() {
        assert!(validate_columns(&[]).is_err());
        assert!(validate_columns(&s(&["To Do", "to-do?", "to do"])).is_err());
        assert!(validate_columns(&s(&["***"])).is_err());
        assert_eq!(validate_columns(&s(&["A", "B"])).unwrap(), ["a", "b"]);
    }

    fn new_project(dir: &Path) -> Project {
        Project::create(dir, "My Project", &s(&DEFAULT_COLUMNS)).unwrap()
    }

    fn add(p: &mut Project, col: usize, title: &str) -> Ticket {
        p.add_ticket(
            col,
            Ticket {
                title: title.into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn create_and_reload() {
        let d = tempfile::tempdir().unwrap();
        let p = new_project(d.path());
        assert!(d.path().join("my_project.ghost").is_file());
        assert!(d.path().join("tickets/ready_for_test").is_dir());
        assert!(Project::create(d.path(), "Other", &s(&["A"])).is_err());
        let mut p = p;
        p.meta.ai_cli = Some("codex".into());
        p.save_meta().unwrap();
        let back = Project::load(&p.ghost_path).unwrap();
        assert_eq!(back.meta, p.meta);
        assert_eq!(back.meta.ai_cli.as_deref(), Some("codex"));
        assert!(back.root.is_absolute());
        assert_eq!(back.root, d.path());
    }

    #[test]
    fn load_refuses_unsafe_or_duplicate_folders() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("p.ghost");
        for folder in ["../../outside", "x’; calc; ’", "To Do", "", "a\nb"] {
            let yaml = format!("name: P\ncolumns:\n- name: A\n  folder: {folder:?}\n");
            fs::write(&path, yaml).unwrap();
            assert!(Project::load(&path).is_err(), "{folder:?}");
        }
        fs::write(
            &path,
            "name: P\ncolumns:\n- {name: A, folder: a}\n- {name: B, folder: a}\n",
        )
        .unwrap();
        assert!(Project::load(&path).is_err());
        fs::write(
            &path,
            "name: P\ncolumns:\n- {name: A, folder: con_}\n- {name: B, folder: über_done}\n",
        )
        .unwrap();
        assert!(Project::load(&path).is_ok());
    }

    #[test]
    fn column_agents_round_trip_and_odd_names_are_dropped() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("p.ghost");
        fs::write(
            &path,
            "name: P\ncolumns:\n- {name: A, folder: a, agent: tester}\n- {name: B, folder: b, agent: \"x; rm -rf /\"}\n",
        )
        .unwrap();
        let p = Project::load(&path).unwrap();
        assert_eq!(p.meta.columns[0].agent.as_deref(), Some("tester"));
        assert_eq!(p.meta.columns[1].agent, None);
        p.save_meta().unwrap();
        let p = Project::load(&path).unwrap();
        assert_eq!(p.meta.columns[0].agent.as_deref(), Some("tester"));
    }

    #[test]
    fn ids_increment_and_bump_past_disk() {
        let d = tempfile::tempdir().unwrap();
        let mut p = new_project(d.path());
        assert_eq!(add(&mut p, 0, "a").id, 1);
        assert_eq!(add(&mut p, 1, "b").id, 2);
        fs::write(p.column_dir(2).join("00041.md"), "---\ntitle: copied\n---\n").unwrap();
        p.rescan().unwrap();
        assert_eq!(p.meta.next_ticket_id, 42);
        let t = add(&mut p, 0, "c");
        assert_eq!(t.id, 42);
        assert!(t.path.ends_with("tickets/to_do/00042.md"));
    }

    #[test]
    fn auto_picks_by_order_then_right_most_column() {
        let d = tempfile::tempdir().unwrap();
        let mut p = new_project(d.path());
        for (c, m) in p.meta.columns.iter_mut().zip(DEFAULT_AUTO_MOVES) {
            c.auto_move = m;
        }
        let mut put = |col, priority| {
            let t = Ticket {
                priority,
                ..Default::default()
            };
            p.add_ticket(col, t).unwrap().id
        };
        let (a, b, c) = (
            put(0, ticket::Priority::High),
            put(2, ticket::Priority::High),
            put(0, ticket::Priority::Low),
        );
        let _not_auto = put(4, ticket::Priority::Highest);
        let auto: HashSet<String> = ["to_do", "ready_for_test", "done"].map(String::from).into();
        assert_eq!(p.next_auto(&auto, AutoOrder::PriorityDesc, 0), Some((2, b)));
        assert_eq!(p.next_auto(&auto, AutoOrder::PriorityAsc, 0), Some((0, c)));
        assert_eq!(p.next_auto(&auto, AutoOrder::IdAsc, 0), Some((0, a)));
        assert_eq!(p.next_auto(&auto, AutoOrder::IdDesc, 0), Some((0, c)));
        assert_eq!(p.next_auto(&HashSet::new(), AutoOrder::IdAsc, 0), None);
        // A ticket the AI has had enough goes at is skipped.
        let mut t = p.find_ticket(b).unwrap().1.clone();
        t.run_count = 3;
        t.save().unwrap();
        p.rescan().unwrap();
        assert_eq!(p.next_auto(&auto, AutoOrder::PriorityDesc, 3), Some((0, a)));
        assert_eq!(p.next_auto(&auto, AutoOrder::PriorityDesc, 4), Some((2, b)));
        // A full In Test column stops Auto taking from Ready For Test.
        p.meta.columns[3].wip_limit = Some(1);
        assert!(!p.is_full(3));
        p.add_ticket(3, Ticket::default()).unwrap();
        assert!(p.is_full(3));
        assert_eq!(p.next_auto(&auto, AutoOrder::PriorityDesc, 4), Some((0, a)));
    }

    #[test]
    fn blocked_tickets_wait_until_their_blockers_are_done() {
        let d = tempfile::tempdir().unwrap();
        let mut p = new_project(d.path());
        for (c, m) in p.meta.columns.iter_mut().zip(DEFAULT_AUTO_MOVES) {
            c.auto_move = m;
        }
        let first = add(&mut p, 0, "first");
        let mut second = add(&mut p, 0, "second");
        second.blocked_by = vec![first.id, 99]; // 99 doesn't exist, so it doesn't block
        second.priority = ticket::Priority::Highest;
        second.save().unwrap();
        p.rescan().unwrap();
        let auto: HashSet<String> = ["to_do".to_string()].into();
        assert!(p.is_blocked(second.id));
        assert_eq!(p.blockers(p.find_ticket(second.id).unwrap().1), [first.id]);
        assert_eq!(p.dependents(first.id), [second.id]);
        assert!(p.dependents(second.id).is_empty());
        assert_eq!(p.next_auto(&auto, AutoOrder::PriorityDesc, 0), Some((0, first.id)));
        p.move_ticket(&first.path, p.last_col()).unwrap();
        assert!(!p.is_blocked(second.id));
        assert_eq!(p.next_auto(&auto, AutoOrder::PriorityDesc, 0), Some((0, second.id)));
    }

    #[test]
    fn auto_targets_follow_the_next_column() {
        let d = tempfile::tempdir().unwrap();
        let mut p = new_project(d.path());
        for (c, m) in p.meta.columns.iter_mut().zip(DEFAULT_AUTO_MOVES) {
            c.auto_move = m;
        }
        assert_eq!(p.auto_target(0, AutoMove::AiWorking), Some(1));
        assert_eq!(p.auto_target(0, AutoMove::AiComplete), None);
        assert_eq!(p.auto_target(1, AutoMove::AiComplete), Some(2));
        assert_eq!(p.auto_target(3, AutoMove::AiComplete), Some(4));
        assert_eq!(p.auto_target(4, AutoMove::AiComplete), None);
        let yaml = serde_yaml_ng::to_string(&p.meta).unwrap();
        assert!(yaml.contains("auto_move: ai_working") && !yaml.contains("none"));
    }

    #[test]
    fn guide_is_added_once_and_keeps_the_rest_of_the_file() {
        assert_eq!(replace_guide("", "G"), format!("{GUIDE_START}\nG\n{GUIDE_END}\n"));
        let mine = "# Mine\n\nKeep this.\n";
        let once = replace_guide(mine, "Old");
        assert!(once.starts_with("# Mine\n\nKeep this.\n\n<!--"));
        let twice = replace_guide(&format!("{once}\nAfter\n"), "New");
        assert_eq!(twice.matches(GUIDE_START).count(), 1);
        assert!(twice.contains("\nNew\n") && !twice.contains("Old"));
        assert!(twice.starts_with("# Mine") && twice.ends_with("\nAfter\n"));
    }

    #[test]
    fn guide_lists_columns_and_lands_in_the_cli_file() {
        let d = tempfile::tempdir().unwrap();
        let p = new_project(d.path());
        p.write_ai_guide("CLAUDE.md").unwrap();
        let text = fs::read_to_string(d.path().join("CLAUDE.md")).unwrap();
        assert!(text.contains("3. **Ready For Test** (`tickets/ready_for_test/`)\n"));
        assert!(text.contains("`my_project.ghost`"));
        p.write_ai_guide("CLAUDE.md").unwrap();
        assert_eq!(fs::read_to_string(d.path().join("CLAUDE.md")).unwrap(), text);
        p.write_ai_guide(".github/copilot-instructions.md").unwrap();
        assert!(d.path().join(".github/copilot-instructions.md").is_file());

        let mut p = p;
        for (c, m) in p.meta.columns.iter_mut().zip(DEFAULT_AUTO_MOVES) {
            c.auto_move = m;
        }
        assert!(p.ai_guide().contains(
            "2. **In Dev** (`tickets/in_dev/`) (GhostLane moves tickets here from **To Do** (`tickets/to_do/`) \
             when the AI starts on them)\n"
        ));
    }

    #[test]
    fn move_between_columns() {
        let d = tempfile::tempdir().unwrap();
        let mut p = new_project(d.path());
        let t = add(&mut p, 0, "a");
        p.move_ticket(&t.path, 4).unwrap();
        assert_eq!(p.tickets[0].len(), 0);
        assert_eq!(p.tickets[4][0].title, "a");
        assert!(!t.path.exists());
    }

    #[test]
    fn column_edits() {
        let d = tempfile::tempdir().unwrap();
        let mut p = Project::create(d.path(), "P", &s(&["A", "B", "C"])).unwrap();
        let a = add(&mut p, 0, "in a");
        add(&mut p, 1, "in b");
        add(&mut p, 2, "in c");
        let edit = |name: &str, orig: Option<&str>| ColumnEdit {
            name: name.into(),
            orig_folder: orig.map(Into::into),
            ..Default::default()
        };
        // Swap A and B by name, drop C into the renamed "A" (old b), add D.
        let mut edits = [edit("B", Some("a")), edit("A", Some("b")), edit("D", None)];
        edits[1].auto_move = AutoMove::AiWorking;
        edits[2].ai_prompt = Some("Test {ticket_path}".into());
        edits[0].wip_limit = Some(3);
        edits[1].wip_limit = Some(0);
        let targets = HashMap::from([("c".to_string(), 1)]);
        p.apply_columns("P2", &edits, &targets).unwrap();

        assert_eq!(p.meta.name, "P2");
        let folders: Vec<_> = p.meta.columns.iter().map(|c| c.folder.as_str()).collect();
        assert_eq!(folders, ["b", "a", "d"]);
        let titles = |c: usize| p.tickets[c].iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(0), ["in a"]);
        assert_eq!(titles(1), ["in b", "in c"]);
        assert!(titles(2).is_empty());
        assert_eq!(p.meta.columns[1].auto_move, AutoMove::AiWorking);
        assert_eq!(p.meta.columns[2].ai_prompt.as_deref(), Some("Test {ticket_path}"));
        assert_eq!(p.meta.columns[0].wip_limit, Some(3));
        assert_eq!(p.meta.columns[1].wip_limit, None);
        let back = Project::load(&p.ghost_path).unwrap();
        assert_eq!(back.meta, p.meta);
        assert!(!p.tickets_dir().join("c").exists());
        assert!(!a.path.exists()); // a's file now lives under folder b
        assert!(p.tickets_dir().join("b").join(a.path.file_name().unwrap()).exists());
    }

    #[test]
    fn gitignore_entries_come_and_go_leaving_the_rest() {
        let entries = ["/p.ghost".to_string(), "/tickets/".to_string()];
        let added = toggle_ignore("target/\n", &entries, true);
        assert_eq!(added, "target/\n\n# GhostLane board\n/p.ghost\n/tickets/\n");
        assert_eq!(toggle_ignore(&added, &entries, true), added, "added once");
        assert_eq!(toggle_ignore(&added, &entries, false), "target/\n");
        // The user's own spelling counts, and only the missing one is added.
        assert_eq!(
            toggle_ignore("tickets\r\n", &entries, true),
            "tickets\r\n\r\n# GhostLane board\r\n/p.ghost\r\n"
        );
        assert_eq!(
            toggle_ignore("", &entries, true),
            "# GhostLane board\n/p.ghost\n/tickets/\n"
        );
        assert_eq!(
            toggle_ignore("a\n# tickets\n", &entries, false),
            "a\n# tickets\n",
            "comments stay"
        );

        let d = tempfile::tempdir().unwrap();
        let p = new_project(d.path());
        assert!(!p.git_excluded());
        p.set_git_excluded(true).unwrap();
        assert!(p.git_excluded());
        let text = fs::read_to_string(d.path().join(".gitignore")).unwrap();
        assert!(text.contains("/my_project.ghost\n/tickets/\n"), "{text}");
        p.set_git_excluded(false).unwrap();
        assert!(!p.git_excluded());
        assert!(!d.path().join(".gitignore").exists(), "nothing else in it, so it goes");
        fs::write(d.path().join(".gitignore"), "node_modules/\n").unwrap();
        p.set_git_excluded(true).unwrap();
        p.set_git_excluded(false).unwrap();
        assert_eq!(
            fs::read_to_string(d.path().join(".gitignore")).unwrap(),
            "node_modules/\n"
        );
    }

    #[test]
    fn exports_csv_and_markdown() {
        let d = tempfile::tempdir().unwrap();
        let mut p = Project::create(d.path(), "Shop", &s(&["To Do", "Done"])).unwrap();
        let first = Ticket {
            title: "Cart, \"quick\" view".into(),
            labels: vec!["ui".into(), "web".into()],
            ..Default::default()
        };
        let first = p.add_ticket(0, first).unwrap();
        let formula = Ticket {
            title: "=HYPERLINK(\"x\")".into(),
            blocked_by: vec![first.id],
            ..Default::default()
        };
        p.add_ticket(0, formula).unwrap();
        let done = Ticket {
            title: "Login".into(),
            done_at: Some(1_709_210_096),
            ..Default::default()
        };
        p.add_ticket(1, done).unwrap();

        let csv = p.export_csv();
        let lines: Vec<&str> = csv.split("\r\n").collect();
        assert_eq!(
            lines[0],
            "id,title,column,priority,labels,run_count,blocked_by,epic,done_at"
        );
        assert_eq!(lines[1], "00001,\"Cart, \"\"quick\"\" view\",To Do,Medium,ui; web,0,,,");
        assert_eq!(lines[2], "00002,\"'=HYPERLINK(\"\"x\"\")\",To Do,Medium,,0,00001,,");
        assert_eq!(lines[3], "00003,Login,Done,Medium,,0,,,2024-02-29T12:34:56Z");

        let md = p.export_markdown(1_709_210_096);
        assert!(md.starts_with("# Shop\n\nExported 2024-02-29 12:34 UTC.\n"), "{md}");
        assert!(md.contains("\n## To Do (2)\n\n- **00001** Cart, \"quick\" view · Medium `ui` `web`\n"));
        assert!(md.contains("- **00002** =HYPERLINK(\"x\") · Medium · blocked by 00001\n"));
        assert!(md.contains("\n## Done (1)\n\n- **00003** Login · Medium · done 2024-02-29\n"));
    }

    #[test]
    fn stats_from_run_histories() {
        use crate::ticket::{RunRecord, RunResult};
        let d = tempfile::tempdir().unwrap();
        let mut p = Project::create(d.path(), "P", &s(&["To Do", "Done"])).unwrap();
        let run = |seconds, result| RunRecord {
            started: String::new(),
            seconds,
            from: "To Do".into(),
            to: "Done".into(),
            result,
            base: None,
            tokens: (seconds > 50).then_some(ticket::Tokens {
                input: seconds,
                cached: 0,
                output: 1,
            }),
        };
        let day = 86_400;
        let now = 100 * day + 3600;
        let mk = |title: &str, run_count, done_at, runs: Vec<RunRecord>| Ticket {
            title: title.into(),
            run_count,
            done_at,
            runs,
            ..Default::default()
        };
        for (col, t) in [
            (1, mk("Clean", 1, Some(100 * day), vec![run(60, RunResult::MovedOn)])),
            (
                1,
                mk(
                    "Bounced",
                    3,
                    Some(99 * day + 5),
                    vec![run(100, RunResult::SentBack), run(20, RunResult::MovedOn)],
                ),
            ),
            (1, mk("Old", 0, Some(50 * day), vec![])),
            (0, mk("Stuck", 4, None, vec![run(0, RunResult::Unmoved)])),
        ] {
            p.add_ticket(col, t).unwrap();
        }
        let st = p.stats(now);
        assert_eq!(st.done_per_day[STATS_DAYS - 1], 1);
        assert_eq!(st.done_per_day[STATS_DAYS - 2], 1);
        assert_eq!(st.done_per_day.iter().sum::<u32>(), 2); // "Old" is outside the window
        assert_eq!((st.runs, st.avg_run_secs), (4, Some(45)));
        assert_eq!((st.tokens.total(), st.runs_with_tokens), (162, 2));
        assert_eq!(st.first_try, (1, 2));
        assert_eq!(
            st.most_retried,
            [(4, "Stuck".to_string(), 4), (2, "Bounced".to_string(), 3)]
        );
        let empty = tempfile::tempdir().unwrap();
        let st = Project::create(empty.path(), "E", &s(&["A"])).unwrap().stats(now);
        assert_eq!(st, Stats::default());
    }
}
