//! Per-app rules from `$XDG_CONFIG_HOME/simple-session/config.toml`:
//!
//! ```toml
//! [apps."firefox.desktop"]
//! launch_mode = "once"        # or "per_window"
//!
//! [apps."org.gnome.Settings.desktop"]
//! ignore = true               # never saved, launched or placed
//!
//! [apps."com.mitchellh.ghostty"]
//! argv = ["ghostty"]          # launch with this command instead
//! ```
//!
//! Apps are keyed like in session files: the desktop file id, or the
//! wm_class for windows without one.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::model::{App, Launch, LaunchMode, Session};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub apps: HashMap<String, AppRule>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppRule {
    pub launch_mode: Option<LaunchMode>,
    #[serde(default)]
    pub ignore: bool,
    pub argv: Option<Vec<String>>,
}

impl Config {
    /// `SIMPLE_SESSION_CONFIG` overrides the location. A missing file is an
    /// empty config.
    pub fn load_default() -> Result<Self> {
        let path = match std::env::var_os("SIMPLE_SESSION_CONFIG") {
            Some(path) => PathBuf::from(path),
            None => match directories::BaseDirs::new() {
                Some(dirs) => dirs.config_dir().join("simple-session/config.toml"),
                None => return Ok(Config::default()),
            },
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("invalid config {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn ignored(&self, app_id: &str) -> bool {
        self.apps.get(app_id).is_some_and(|r| r.ignore)
    }

    /// Launch mode for a newly captured app: the rule, else a built-in guess.
    pub fn launch_mode(&self, app_id: &str) -> LaunchMode {
        self.apps.get(app_id).and_then(|r| r.launch_mode).unwrap_or_else(|| {
            if restores_own_windows(app_id) { LaunchMode::Once } else { LaunchMode::PerWindow }
        })
    }

    /// Applies the rules to a loaded session: drops ignored apps and overrides
    /// launch mode and command. Rules win over what the session file says, so
    /// a rule added later also affects sessions saved before.
    pub fn apply_to(&self, session: &mut Session) {
        session.apps.retain(|app| !self.ignored(&app.id));
        for app in &mut session.apps {
            self.override_app(app);
        }
    }

    fn override_app(&self, app: &mut App) {
        let Some(rule) = self.apps.get(&app.id) else { return };
        if let Some(mode) = rule.launch_mode {
            app.launch_mode = mode;
        }
        if let Some(argv) = rule.argv.as_ref().filter(|a| !a.is_empty()) {
            app.launch = Launch::Argv { argv: argv.clone() };
        }
    }
}

/// Apps that reopen their previous windows themselves when started (browsers,
/// IDEs, editors). Launching them once per saved window would duplicate
/// windows, so they are launched once.
pub fn restores_own_windows(app_id: &str) -> bool {
    let id = app_id.strip_suffix(".desktop").unwrap_or(app_id).to_lowercase();
    const PREFIXES: &[&str] = &[
        "firefox",
        "org.mozilla.firefox",
        "chromium",
        "org.chromium.",
        "google-chrome",
        "com.google.chrome",
        "brave-browser",
        "com.brave.browser",
        "vivaldi",
        "com.vivaldi.",
        "microsoft-edge",
        "com.microsoft.edge",
        "jetbrains-",
        "com.jetbrains.",
        "sublime_text",
        "com.sublimetext.",
        "sublime_merge",
        "com.sublimemerge.",
        "dev.zed.",
    ];
    const EXACT: &[&str] = &[
        "code",
        "code-oss",
        "code-insiders",
        "codium",
        "visual-studio-code",
        "com.visualstudio.code",
        "zed",
    ];
    EXACT.contains(&id.as_str()) || PREFIXES.iter().any(|p| id.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SCHEMA, Workspaces};

    #[test]
    fn known_self_restoring_apps() {
        for id in [
            "firefox.desktop",
            "firefox-developer-edition.desktop",
            "org.mozilla.firefox.desktop",
            "chromium.desktop",
            "google-chrome.desktop",
            "jetbrains-idea.desktop",
            "code-oss.desktop",
            "sublime_merge.desktop",
        ] {
            assert!(restores_own_windows(id), "{id}");
        }
        for id in
            ["org.gnome.Terminal.desktop", "org.gnome.Nautilus.desktop", "codex.desktop", "Alacritty.desktop"]
        {
            assert!(!restores_own_windows(id), "{id}");
        }
    }

    #[test]
    fn rules_override_session_and_guess() {
        let config: Config = toml::from_str(
            r#"
            [apps."firefox.desktop"]
            launch_mode = "per_window"

            [apps."org.gnome.Settings.desktop"]
            ignore = true

            [apps."ghostty"]
            argv = ["ghostty", "--gtk-single-instance=false"]
            "#,
        )
        .unwrap();
        assert_eq!(config.launch_mode("firefox.desktop"), LaunchMode::PerWindow);
        assert_eq!(config.launch_mode("chromium.desktop"), LaunchMode::Once);
        assert_eq!(config.launch_mode("other.desktop"), LaunchMode::PerWindow);

        let app = |id: &str| App {
            id: id.into(),
            launch: Launch::Desktop { desktop_id: format!("{id}.desktop") },
            launch_mode: LaunchMode::Once,
            windows: Vec::new(),
        };
        let mut session = Session {
            schema: SCHEMA,
            name: "t".into(),
            created: String::new(),
            host: String::new(),
            shell_version: String::new(),
            workspaces: Workspaces { count: 1, dynamic: true, only_on_primary: true },
            monitors: Vec::new(),
            apps: vec![app("firefox.desktop"), app("org.gnome.Settings.desktop"), app("ghostty")],
        };
        config.apply_to(&mut session);
        let ids: Vec<_> = session.apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["firefox.desktop", "ghostty"]);
        assert_eq!(session.apps[0].launch_mode, LaunchMode::PerWindow);
        assert_eq!(
            session.apps[1].launch,
            Launch::Argv { argv: vec!["ghostty".into(), "--gtk-single-instance=false".into()] }
        );
    }

    #[test]
    fn rejects_typos() {
        assert!(toml::from_str::<Config>("[apps.\"a\"]\nlaunch_mod = \"once\"").is_err());
    }
}
