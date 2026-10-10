use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ticket::{self, Ticket, write_atomic};

pub const DEFAULT_COLUMNS: [&str; 5] = ["To Do", "In Dev", "Ready For Test", "In Test", "Done"];

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
}

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
}

/// A column as edited in the project modal. `orig_folder` links it to an existing column.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnEdit {
    pub name: String,
    pub orig_folder: Option<String>,
}

pub struct Project {
    pub ghost_path: PathBuf,
    pub root: PathBuf,
    pub meta: GhostFile,
    /// Index-aligned with `meta.columns`.
    pub tickets: Vec<Vec<Ticket>>,
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
        || ((s.starts_with("com") || s.starts_with("lpt"))
            && s.len() == 4
            && matches!(s.as_bytes()[3], b'1'..=b'9'));
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
        let text = fs::read_to_string(ghost_path)?;
        let meta: GhostFile = serde_yaml_ng::from_str(&text)
            .map_err(|e| err(format!("{} is not a valid .ghost file: {e}", ghost_path.display())))?;
        let root = ghost_path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut p = Project { ghost_path: ghost_path.to_path_buf(), root, meta, tickets: Vec::new() };
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
            return Err(err(format!("{} already contains a project ({})", dir.display(), existing.display())));
        }
        let folders = validate_columns(column_names).map_err(err)?;
        let slug = folder_name(name).unwrap_or_else(|| "project".into());
        let columns = column_names
            .iter()
            .zip(folders)
            .map(|(n, f)| Column { name: n.trim().to_string(), folder: f, sort: Sort::default() })
            .collect();
        let meta = GhostFile { version: 1, name: name.to_string(), next_ticket_id: 1, columns };
        let ghost_path = dir.join(format!("{slug}.ghost"));
        let p = Project { ghost_path: ghost_path.clone(), root: dir.to_path_buf(), meta, tickets: Vec::new() };
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
        self.meta.next_ticket_id = self.meta.next_ticket_id.max(max_id + 1);
        Ok(())
    }

    pub fn set_sort(&mut self, col: usize, sort: Sort) -> io::Result<()> {
        self.meta.columns[col].sort = sort;
        ticket::sort(&mut self.tickets[col], sort.by, sort.dir);
        self.save_meta()
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
        let mut id = self.meta.next_ticket_id;
        let name = |id| format!("{}.md", ticket::format_id(id));
        while (0..self.meta.columns.len()).any(|c| self.column_dir(c).join(name(id)).exists()) {
            id += 1;
        }
        t.id = id;
        t.path = self.column_dir(col).join(name(id));
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
                Column { name: e.name.trim().to_string(), folder, sort }
            })
            .collect();
        self.save_meta()?;
        self.rescan()
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
        p.add_ticket(col, Ticket { title: title.into(), ..Default::default() }).unwrap()
    }

    #[test]
    fn create_and_reload() {
        let d = tempfile::tempdir().unwrap();
        let p = new_project(d.path());
        assert!(d.path().join("my_project.ghost").is_file());
        assert!(d.path().join("tickets/ready_for_test").is_dir());
        assert!(Project::create(d.path(), "Other", &s(&["A"])).is_err());
        let back = Project::load(&p.ghost_path).unwrap();
        assert_eq!(back.meta, p.meta);
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
        let edit = |name: &str, orig: Option<&str>| ColumnEdit { name: name.into(), orig_folder: orig.map(Into::into) };
        // Swap A and B by name, drop C into the renamed "A" (old b), add D.
        let edits = [edit("B", Some("a")), edit("A", Some("b")), edit("D", None)];
        let targets = HashMap::from([("c".to_string(), 1)]);
        p.apply_columns("P2", &edits, &targets).unwrap();

        assert_eq!(p.meta.name, "P2");
        let folders: Vec<_> = p.meta.columns.iter().map(|c| c.folder.as_str()).collect();
        assert_eq!(folders, ["b", "a", "d"]);
        let titles = |c: usize| p.tickets[c].iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(0), ["in a"]);
        assert_eq!(titles(1), ["in b", "in c"]);
        assert!(titles(2).is_empty());
        assert!(!p.tickets_dir().join("c").exists());
        assert!(!a.path.exists()); // a's file now lives under folder b
        assert!(p.tickets_dir().join("b").join(a.path.file_name().unwrap()).exists());
    }
}
