use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use egui_term::{BackendCommand, BackendSettings, ColorPalette, PtyEvent, TerminalBackend, TerminalTheme};

/// Characters that would let a substituted value break out of the command.
const UNSAFE: &[char] = &['"', '\'', '`', '$', ';', '&', '|', '<', '>', '\n', '\r'];

/// Fills `{ticket_path}`, `{ticket_id}` and `{project_dir}` in the AI command template.
/// Values are refused rather than escaped, since the shell may be PowerShell, cmd or POSIX.
pub fn build_ai_command(template: &str, ticket_path: &Path, ticket_id: &str, project_dir: &Path) -> Result<String, String> {
    let values = [
        ("{ticket_path}", ticket_path.display().to_string()),
        ("{ticket_id}", ticket_id.to_string()),
        ("{project_dir}", project_dir.display().to_string()),
    ];
    let mut cmd = template.trim().to_string();
    if cmd.is_empty() {
        return Err("Set an AI command in Settings first".into());
    }
    for (key, value) in values {
        if !cmd.contains(key) {
            continue;
        }
        if value.contains(UNSAFE) {
            return Err(format!("Not running the AI command: {key} contains characters that aren't safe in a shell command ({value})"));
        }
        cmd = cmd.replace(key, &value);
    }
    Ok(cmd)
}

/// Shell program and arguments. `custom` wins when set.
pub fn detect_shell(custom: Option<&str>) -> (String, Vec<String>) {
    if let Some(s) = custom.map(str::trim).filter(|s| !s.is_empty()) {
        return (s.to_string(), Vec::new());
    }
    if cfg!(windows) {
        for candidate in ["pwsh.exe", "powershell.exe"] {
            if let Ok(path) = which::which(candidate) {
                return (path.display().to_string(), vec!["-NoLogo".into()]);
            }
        }
        (std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()), Vec::new())
    } else {
        let fallback = if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/bash" };
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| fallback.into());
        // Login shell so PATH from the user's profile (where CLIs like `claude` live) is loaded.
        (shell, vec!["-l".into()])
    }
}

pub fn theme() -> TerminalTheme {
    TerminalTheme::new(Box::new(ColorPalette {
        background: "#1E1F22".into(),
        foreground: "#DCDDDE".into(),
        green: "#A8E6A1".into(),
        bright_green: "#C4F0BF".into(),
        ..Default::default()
    }))
}

pub struct Session {
    pub backend: TerminalBackend,
    pub exited: bool,
}

/// One shell per open project, keyed by .ghost path. Started lazily.
pub struct Terminals {
    sessions: HashMap<PathBuf, (u64, Session)>,
    next_id: u64,
    tx: Sender<(u64, PtyEvent)>,
    // egui_term's reader threads panic if this receiver is dropped, so it lives as long as the app.
    rx: Receiver<(u64, PtyEvent)>,
}

impl Terminals {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Terminals { sessions: HashMap::new(), next_id: 0, tx, rx }
    }

    /// Marks sessions whose shell has exited; they stay until `restart`.
    pub fn poll(&mut self) {
        while let Ok((id, event)) = self.rx.try_recv() {
            if matches!(event, PtyEvent::Exit)
                && let Some((_, s)) = self.sessions.values_mut().find(|(sid, _)| *sid == id)
            {
                s.exited = true;
            }
        }
    }

    pub fn get_or_start(
        &mut self,
        key: &Path,
        cwd: &Path,
        shell: Option<&str>,
        ctx: &egui::Context,
    ) -> Result<&mut Session, String> {
        if !self.sessions.contains_key(key) {
            let (shell, args) = detect_shell(shell);
            let settings = BackendSettings { shell: shell.clone(), args, working_directory: Some(cwd.to_path_buf()) };
            let id = self.next_id;
            self.next_id += 1;
            let backend = TerminalBackend::new(id, ctx.clone(), self.tx.clone(), settings)
                .map_err(|e| format!("Couldn't start {shell}: {e}"))?;
            self.sessions.insert(key.to_path_buf(), (id, Session { backend, exited: false }));
        }
        Ok(&mut self.sessions.get_mut(key).expect("just inserted").1)
    }

    pub fn restart(&mut self, key: &Path) {
        self.sessions.remove(key);
    }
}

impl Session {
    pub fn run(&mut self, command: &str) {
        self.backend.process_command(BackendCommand::Write(format!("{command}\r").into_bytes()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_placeholders() {
        let cmd = build_ai_command(
            "claude \"Do {ticket_id} at {ticket_path} in {project_dir}\"",
            Path::new("/p/tickets/to_do/00007.md"),
            "00007",
            Path::new("/p"),
        )
        .unwrap();
        assert_eq!(cmd, "claude \"Do 00007 at /p/tickets/to_do/00007.md in /p\"");
    }

    #[test]
    fn refuses_unsafe_values() {
        let bad = Path::new("/tmp/x\"; rm -rf ~; \"/00001.md");
        assert!(build_ai_command("run {ticket_path}", bad, "00001", Path::new("/tmp")).is_err());
        assert!(build_ai_command("run {ticket_id}", Path::new("/a$b"), "1", Path::new("/")).is_ok()); // unused placeholder
        assert!(build_ai_command("  ", Path::new("/a"), "1", Path::new("/")).is_err());
    }

    #[test]
    fn custom_shell_wins() {
        assert_eq!(detect_shell(Some(" fish ")), ("fish".to_string(), vec![]));
        assert!(!detect_shell(None).0.is_empty());
    }
}
