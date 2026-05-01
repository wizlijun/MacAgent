//! Launcher configuration + AppleScript Terminal.app integration.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherConfig {
    pub launchers: Vec<Launcher>,
    #[serde(default)]
    pub gui: GuiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Launcher {
    pub id: String,
    pub label: String,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
}

/// M7 GUI app launch whitelist (bundle ids allowed via launcher_m7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiConfig {
    #[serde(default = "default_allowed_bundles")]
    pub allowed_bundles: Vec<String>,
}

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            allowed_bundles: default_allowed_bundles(),
        }
    }
}

fn default_allowed_bundles() -> Vec<String> {
    vec![
        // AI
        "com.openai.chat".into(),
        "com.anthropic.claude".into(),
        "com.openai.codex".into(),
        // Browsers
        "com.google.Chrome".into(),
        "com.apple.Safari".into(),
        // Editors
        "com.microsoft.VSCode".into(),
        "com.todesktop.230313mzl4w4u92".into(), // Cursor
        // Terminals
        "dev.warp.Warp-Stable".into(),
        "com.googlecode.iterm2".into(),
        "com.apple.Terminal".into(),
        // Productivity / Design
        "com.figma.Desktop".into(),
        "notion.id".into(),
        "com.linear".into(),
        // Chat
        "com.tinyspeck.slackmacgap".into(),
        "com.hnc.Discord".into(),
        "ru.keepcoder.Telegram".into(),
        "com.tencent.xinWeChat".into(),
        // Media / Office
        "com.spotify.client".into(),
        "com.microsoft.Word".into(),
    ]
}

pub fn config_path() -> PathBuf {
    let mut p = dirs::home_dir().expect("home_dir");
    p.push("Library/Application Support/macagent/launchers.json5");
    p
}

pub async fn load_or_init() -> Result<LauncherConfig> {
    let path = config_path();
    if !path.exists() {
        let default = LauncherConfig::default_config();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, default_json5_string())?;
        return Ok(default);
    }
    let content = std::fs::read_to_string(&path)?;
    let cfg: LauncherConfig = json5::from_str(&content)?;
    Ok(cfg)
}

/// Sync read of `launchers.json5` (for non-async eframe contexts); falls back to defaults.
pub fn load_or_init_blocking() -> std::io::Result<LauncherConfig> {
    let path = config_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(cfg) = json5::from_str::<LauncherConfig>(&text) {
            return Ok(cfg);
        }
    }
    Ok(LauncherConfig::default_config())
}

/// Persist `cfg` to `launchers.json5` as pretty JSON (json5 reads JSON natively).
pub fn save_config(cfg: &LauncherConfig) -> std::io::Result<()> {
    let path = config_path();
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, json)
}

impl LauncherConfig {
    pub fn default_config() -> Self {
        Self {
            launchers: vec![
                Launcher {
                    id: "zsh".into(),
                    label: "Zsh shell".into(),
                    argv: vec!["zsh".into(), "-l".into()],
                    cwd: None,
                },
                Launcher {
                    id: "claude-code".into(),
                    label: "Claude Code".into(),
                    argv: vec!["claude".into(), "code".into()],
                    cwd: None,
                },
                Launcher {
                    id: "codex".into(),
                    label: "Codex".into(),
                    argv: vec!["codex".into()],
                    cwd: None,
                },
                Launcher {
                    id: "npm-test".into(),
                    label: "npm test".into(),
                    argv: vec!["npm".into(), "test".into()],
                    cwd: None,
                },
                Launcher {
                    id: "git-status".into(),
                    label: "git status".into(),
                    argv: vec!["git".into(), "status".into()],
                    cwd: None,
                },
            ],
            gui: GuiConfig::default(),
        }
    }
}

fn default_json5_string() -> &'static str {
    r#"{
  "launchers": [
    { "id": "zsh",         "label": "Zsh shell",     "argv": ["zsh", "-l"],         "cwd": null },
    { "id": "claude-code", "label": "Claude Code",   "argv": ["claude", "code"],    "cwd": null },
    { "id": "codex",       "label": "Codex",         "argv": ["codex"],             "cwd": null },
    { "id": "npm-test",    "label": "npm test",      "argv": ["npm", "test"],       "cwd": null },
    { "id": "git-status",  "label": "git status",    "argv": ["git", "status"],     "cwd": null }
  ],
  "gui": {
    "allowed_bundles": [
      "com.openai.chat",
      "com.anthropic.claude",
      "com.openai.codex",
      "com.google.Chrome",
      "com.apple.Safari",
      "com.microsoft.VSCode",
      "com.todesktop.230313mzl4w4u92",
      "dev.warp.Warp-Stable",
      "com.googlecode.iterm2",
      "com.apple.Terminal",
      "com.figma.Desktop",
      "notion.id",
      "com.linear",
      "com.tinyspeck.slackmacgap",
      "com.hnc.Discord",
      "ru.keepcoder.Telegram",
      "com.tencent.xinWeChat",
      "com.spotify.client",
      "com.microsoft.Word"
    ]
  }
}
"#
}

/// Open Terminal.app and run `macagent run --launcher-id <id> -- <argv>`.
pub async fn launch_in_terminal(launcher: &Launcher, cwd_override: Option<&str>) -> Result<()> {
    let macagent_path = std::env::current_exe()?;
    let macagent_path_str = macagent_path.display().to_string();

    // Build shell command string: [cd <cwd> &&] <macagent> run --launcher-id <id> -- <args...>
    let mut cmd = String::new();
    if let Some(cwd) = cwd_override.or(launcher.cwd.as_deref()) {
        cmd.push_str(&format!("cd {} && ", shell_escape::escape(cwd.into())));
    }
    cmd.push_str(&format!(
        "{} run --launcher-id {} -- ",
        shell_escape::escape(macagent_path_str.into()),
        shell_escape::escape(launcher.id.clone().into()),
    ));
    for arg in &launcher.argv {
        cmd.push_str(&shell_escape::escape(arg.clone().into()));
        cmd.push(' ');
    }

    // Escape cmd for embedding inside AppleScript double-quoted string.
    // AppleScript strings use \" for literal quote and \\ for literal backslash.
    let escaped_cmd = cmd.replace('\\', "\\\\").replace('"', "\\\"");

    let applescript = format!(r#"tell application "Terminal" to do script "{escaped_cmd}""#,);

    let status = Command::new("osascript")
        .arg("-e")
        .arg(&applescript)
        .status()
        .await?;

    if !status.success() {
        anyhow::bail!("osascript failed with status {:?}", status.code());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_config_has_5_entries() {
        let cfg = LauncherConfig::default_config();
        assert_eq!(cfg.launchers.len(), 5);

        let ids: Vec<&str> = cfg.launchers.iter().map(|l| l.id.as_str()).collect();
        assert!(ids.contains(&"zsh"));
        assert!(ids.contains(&"claude-code"));
        assert!(ids.contains(&"codex"));
        assert!(ids.contains(&"npm-test"));
        assert!(ids.contains(&"git-status"));
    }

    #[tokio::test]
    async fn parse_user_config() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("launchers.json5");

        let content = r#"{
  // Custom launcher config
  "launchers": [
    { "id": "my-shell", "label": "My Shell", "argv": ["bash", "--login"], "cwd": "/tmp" },
    { "id": "editor",   "label": "Editor",   "argv": ["nvim"],            "cwd": null  }
  ]
}"#;
        std::fs::write(&config_path, content).unwrap();

        let text = std::fs::read_to_string(&config_path).unwrap();
        let cfg: LauncherConfig = json5::from_str(&text).unwrap();

        assert_eq!(cfg.launchers.len(), 2);
        assert_eq!(cfg.launchers[0].id, "my-shell");
        assert_eq!(cfg.launchers[0].argv, vec!["bash", "--login"]);
        assert_eq!(cfg.launchers[0].cwd.as_deref(), Some("/tmp"));
        assert_eq!(cfg.launchers[1].id, "editor");
        assert!(cfg.launchers[1].cwd.is_none());
    }

    #[tokio::test]
    async fn load_or_init_creates_default_when_missing() {
        // We can't easily override config_path() without refactoring, so
        // we test the default_json5_string round-trips correctly.
        let cfg: LauncherConfig = json5::from_str(default_json5_string()).unwrap();
        assert_eq!(cfg.launchers.len(), 5);
        assert_eq!(cfg.launchers[0].id, "zsh");
        assert_eq!(cfg.launchers[1].id, "claude-code");
    }
}
