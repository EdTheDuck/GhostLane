use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::project::AutoOrder;
use crate::ticket::write_atomic;

pub const DEFAULT_AI_PROMPT: &str = "Complete the ticket at {ticket_path}";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Prompt handed to the project's AI CLI by the play button.
    pub ai_prompt: String,
    /// AI CLI ids offered to projects. `None` until detected on first launch.
    pub ai_clis: Option<Vec<String>>,
    /// `None` = auto-detect.
    pub shell: Option<String>,
    pub terminal_font_size: f32,
    /// Look for a newer GitHub release on startup.
    pub check_for_updates: bool,
    /// Which ticket columns set to Auto start next.
    pub auto_order: AutoOrder,
    /// Auto skips tickets sent to the AI this many times. 0 = no limit.
    pub auto_run_limit: u32,
    /// A ticket that fails (comes back from testing, or fails the test command) goes straight back to the AI, up
    /// to this many times in a row. 0 = never.
    pub retry_failed: u32,
    /// Ask for the user's attention when the AI runs out of work.
    pub notify_idle: bool,
    /// Ask for the user's attention when a ticket comes back to an earlier column.
    pub notify_failed: bool,
    /// Show a desktop notification when an AI run ends while GhostLane is in the background.
    pub notify_desktop: bool,
    /// Hide tickets that have been in the last column longer than this many days. 0 = never.
    pub archive_after_days: u32,
    /// Run several tickets at once, each in a git worktree of its own.
    pub parallel_runs: bool,
    /// How many tickets a project runs at once when `parallel_runs` is on.
    pub max_parallel_runs: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ai_prompt: DEFAULT_AI_PROMPT.into(),
            ai_clis: None,
            shell: None,
            terminal_font_size: 13.0,
            check_for_updates: true,
            auto_order: AutoOrder::default(),
            auto_run_limit: 6,
            retry_failed: 0,
            notify_idle: true,
            notify_failed: true,
            notify_desktop: true,
            archive_after_days: 7,
            parallel_runs: false,
            max_parallel_runs: 3,
        }
    }
}

/// Per-project settings that run things on this computer. Kept in the app config rather than the `.ghost`
/// file, so a cloned repo never commits, pushes or runs a command by itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    /// Commit everything when a ticket reaches the last column.
    pub commit: bool,
    /// Push after each of those commits.
    pub push: bool,
    /// Instead of pushing to the current branch, push each ticket to a `ghostlane/<id>` branch and open a pull
    /// request for it.
    pub pull_request: bool,
    /// Runs when the AI finishes a ticket in the last column; the ticket only stays there if it exits 0.
    pub test_command: String,
}

/// When a project's queue works through its tickets by itself. Kept on this computer only, like `ProjectSettings`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    pub enabled: bool,
    /// Minutes after local midnight the window opens and closes. Closing before it opens runs past midnight;
    /// the same time for both means all day.
    pub start: u32,
    pub end: u32,
    /// Days the window opens on, Monday first.
    pub days: [bool; 7],
    /// Tokens one window may use before the queue stops. 0 = no limit.
    pub token_budget: u64,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            start: 22 * 60,
            end: 7 * 60,
            days: [true; 7],
            token_budget: 0,
        }
    }
}

impl Schedule {
    /// Is the window open at `minute` past midnight on `weekday` (0 = Monday)? A window past midnight belongs to
    /// the day it opened on.
    pub fn open_at(&self, weekday: usize, minute: u32) -> bool {
        let day = |d: usize| self.days[d % 7];
        let yesterday = weekday + 6;
        match self.start.cmp(&self.end) {
            std::cmp::Ordering::Equal => day(weekday),
            std::cmp::Ordering::Less => day(weekday) && (self.start..self.end).contains(&minute),
            std::cmp::Ordering::Greater => {
                (day(weekday) && minute >= self.start) || (day(yesterday) && minute < self.end)
            }
        }
    }

    /// Is the window open right now, in local time?
    pub fn open_now(&self) -> bool {
        use chrono::{Datelike, Timelike};
        let now = chrono::Local::now();
        self.open_at(
            now.weekday().num_days_from_monday() as usize,
            now.hour() * 60 + now.minute(),
        )
    }

    /// The days it opens on, in words: "every day", "weekdays", or like "Mon, Wed".
    pub fn days_text(&self) -> String {
        const NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        match self.days {
            [true, true, true, true, true, true, true] => "every day".into(),
            [true, true, true, true, true, false, false] => "weekdays".into(),
            [false, false, false, false, false, true, true] => "weekends".into(),
            [false, false, false, false, false, false, false] => "no days picked".into(),
            days => NAMES
                .iter()
                .zip(days)
                .filter(|(_, on)| *on)
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

/// `22:00` for 1320 minutes past midnight.
pub fn clock_time(minute: u32) -> String {
    format!("{:02}:{:02}", minute / 60 % 24, minute % 60)
}

/// The local time now, like `06:41`.
pub fn local_time() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

impl Settings {
    /// Ticked AI CLIs, never empty.
    pub fn enabled_clis(&self) -> Vec<String> {
        match &self.ai_clis {
            Some(ids) if !ids.is_empty() => ids.clone(),
            _ => vec![crate::terminal::DEFAULT_CLI.to_string()],
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub open_projects: Vec<PathBuf>,
    pub active_project: Option<PathBuf>,
    pub terminal_collapsed: bool,
    /// The queue beside the terminal is hidden; it shows by default.
    pub queue_closed: bool,
    pub settings: Settings,
    /// Keyed by .ghost path.
    pub projects: HashMap<PathBuf, ProjectSettings>,
    /// When each project's queue may run, keyed by .ghost path.
    pub schedules: HashMap<PathBuf, Schedule>,
}

/// `<config_dir>/GhostLane`.
pub fn config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "GhostLane").map(|d| d.config_dir().to_path_buf())
}

impl AppConfig {
    fn path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("config.yaml"))
    }

    /// Loads config (defaults if missing/corrupt) and drops projects whose .ghost is gone.
    pub fn load() -> Self {
        let mut cfg: AppConfig = Self::path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| serde_yaml_ng::from_str(&t).ok())
            .unwrap_or_default();
        if cfg.prune_missing() {
            let _ = cfg.save();
        }
        cfg
    }

    /// Settings of the project with .ghost `ghost`; defaults when it has none.
    pub fn project(&self, ghost: &Path) -> ProjectSettings {
        self.projects.get(ghost).cloned().unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::path().ok_or_else(|| io::Error::other("no config directory"))?;
        let yaml = serde_yaml_ng::to_string(self).map_err(io::Error::other)?;
        write_atomic(&path, &yaml)
    }

    /// Returns true if anything changed.
    pub fn prune_missing(&mut self) -> bool {
        let before = self.clone();
        self.open_projects.retain(|p| p.is_file());
        self.projects.retain(|p, _| p.is_file());
        self.schedules.retain(|p, _| p.is_file());
        if self
            .active_project
            .as_ref()
            .is_none_or(|a| !self.open_projects.contains(a))
        {
            self.active_project = self.open_projects.first().cloned();
        }
        *self != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_drops_missing_and_fixes_active() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("a.ghost");
        fs::write(&real, "name: A\n").unwrap();
        let gone = d.path().join("gone.ghost");
        let commit = ProjectSettings {
            commit: true,
            ..Default::default()
        };
        let mut cfg = AppConfig {
            open_projects: vec![gone.clone(), real.clone()],
            active_project: Some(gone.clone()),
            projects: HashMap::from([(gone.clone(), commit.clone()), (real.clone(), commit.clone())]),
            ..Default::default()
        };
        assert!(cfg.prune_missing());
        assert_eq!(cfg.project(&real), commit);
        assert_eq!(cfg.project(&gone), ProjectSettings::default());
        assert_eq!(cfg.projects.len(), 1);
        assert_eq!(cfg.open_projects, std::slice::from_ref(&real));
        assert_eq!(cfg.active_project, Some(real));
        assert!(!cfg.prune_missing());
    }

    #[test]
    fn schedules_open_on_their_days_and_run_past_midnight() {
        let weekdays = Schedule {
            enabled: true,
            start: 9 * 60,
            end: 17 * 60,
            days: [true, true, true, true, true, false, false],
            token_budget: 0,
        };
        assert!(weekdays.open_at(0, 9 * 60));
        assert!(!weekdays.open_at(0, 17 * 60));
        assert!(!weekdays.open_at(5, 12 * 60), "Saturday");
        assert_eq!(weekdays.days_text(), "weekdays");

        // Opens Friday night only, so it is still open early Saturday but not early Friday.
        let mut night = Schedule {
            days: [false, false, false, false, true, false, false],
            ..Default::default()
        };
        assert!(night.open_at(4, 23 * 60));
        assert!(night.open_at(5, 6 * 60 + 59));
        assert!(!night.open_at(5, 7 * 60));
        assert!(!night.open_at(4, 6 * 60));
        assert_eq!(night.days_text(), "Fri");
        // Sunday night runs into Monday.
        night.days = [false, false, false, false, false, false, true];
        assert!(night.open_at(0, 60));

        let all_day = Schedule {
            start: 0,
            end: 0,
            ..Default::default()
        };
        assert!(all_day.open_at(3, 0) && all_day.open_at(3, 23 * 60 + 59));
        assert_eq!(clock_time(22 * 60 + 30), "22:30");
    }

    #[test]
    fn partial_yaml_fills_defaults_and_ignores_old_keys() {
        let yaml = "terminal_collapsed: true
settings:
  ui_scale: 1.5
  ai_command: claude x
";
        let cfg: AppConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(cfg.terminal_collapsed);
        assert!(!cfg.queue_closed, "the queue shows unless closed");
        assert_eq!(cfg.settings.ai_prompt, DEFAULT_AI_PROMPT);
        assert_eq!(cfg.settings.ai_clis, None);
        assert_eq!(cfg.settings.enabled_clis(), ["claude"]);
    }
}
