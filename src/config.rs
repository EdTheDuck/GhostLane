use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ai_prompt: DEFAULT_AI_PROMPT.into(),
            ai_clis: None,
            shell: None,
            terminal_font_size: 13.0,
        }
    }
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
    pub settings: Settings,
}

impl AppConfig {
    fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "GhostLane").map(|d| d.config_dir().join("config.yaml"))
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

    pub fn save(&self) -> io::Result<()> {
        let path = Self::path().ok_or_else(|| io::Error::other("no config directory"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let yaml = serde_yaml_ng::to_string(self).map_err(io::Error::other)?;
        write_atomic(&path, &yaml)
    }

    /// Returns true if anything changed.
    pub fn prune_missing(&mut self) -> bool {
        let before = self.clone();
        self.open_projects.retain(|p| p.is_file());
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
        let mut cfg = AppConfig {
            open_projects: vec![gone.clone(), real.clone()],
            active_project: Some(gone),
            ..Default::default()
        };
        assert!(cfg.prune_missing());
        assert_eq!(cfg.open_projects, std::slice::from_ref(&real));
        assert_eq!(cfg.active_project, Some(real));
        assert!(!cfg.prune_missing());
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
        assert_eq!(cfg.settings.ai_prompt, DEFAULT_AI_PROMPT);
        assert_eq!(cfg.settings.ai_clis, None);
        assert_eq!(cfg.settings.enabled_clis(), ["claude"]);
    }
}
