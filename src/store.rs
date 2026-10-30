//! Session files in `$XDG_DATA_HOME/simple-session/sessions/<name>.json`.
//!
//! Writes are atomic (temp file + rename). Overwriting or deleting a session
//! first copies it to `backups/`, keeping the newest [`KEEP_BACKUPS`] per name.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::model::{SCHEMA, Session};

const KEEP_BACKUPS: usize = 5;

pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// `SIMPLE_SESSION_DIR` overrides the location (tests, nested sessions).
    pub fn open_default() -> Result<Self> {
        let dir = match std::env::var_os("SIMPLE_SESSION_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => directories::BaseDirs::new()
                .context("cannot determine the home directory")?
                .data_dir()
                .join("simple-session/sessions"),
        };
        Ok(Store { dir })
    }

    #[cfg(test)]
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Store { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.dir.join(format!("{name}.json")))
    }

    pub fn exists(&self, name: &str) -> Result<bool> {
        Ok(self.path(name)?.exists())
    }

    pub fn save(&self, session: &Session, overwrite: bool) -> Result<PathBuf> {
        let path = self.path(&session.name)?;
        if path.exists() {
            if !overwrite {
                bail!("session '{}' already exists (use --force to overwrite)", session.name);
            }
            self.backup(&session.name, &path)?;
        }
        fs::create_dir_all(&self.dir).with_context(|| format!("cannot create {}", self.dir.display()))?;

        let tmp = self.dir.join(format!(".{}.json.tmp", session.name));
        fs::write(&tmp, serde_json::to_string_pretty(session)? + "\n")
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))?;
        Ok(path)
    }

    pub fn load(&self, name: &str) -> Result<Session> {
        let path = self.path(name)?;
        let text = fs::read_to_string(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => anyhow::anyhow!("no session named '{name}'"),
            _ => anyhow::Error::new(e).context(format!("cannot read {}", path.display())),
        })?;
        let session: Session = serde_json::from_str(&text)
            .with_context(|| format!("invalid session file {}", path.display()))?;
        if session.schema != SCHEMA {
            bail!("session '{name}' has schema {}, this version reads {SCHEMA}", session.schema);
        }
        Ok(session)
    }

    /// All readable sessions, sorted by name. Unreadable files are reported
    /// as errors next to their name instead of failing the whole listing.
    pub fn list(&self) -> Result<Vec<(String, Result<Session>)>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", self.dir.display())),
        };
        let mut sessions: Vec<_> = entries
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                let name = name.strip_suffix(".json")?.to_owned();
                validate_name(&name).ok()?;
                let session = self.load(&name);
                Some((name, session))
            })
            .collect();
        sessions.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(sessions)
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let path = self.path(name)?;
        if !path.exists() {
            bail!("no session named '{name}'");
        }
        self.backup(name, &path)?;
        fs::remove_file(&path).with_context(|| format!("cannot delete {}", path.display()))
    }

    fn backup(&self, name: &str, path: &Path) -> Result<()> {
        let dir = self.dir.join("backups");
        fs::create_dir_all(&dir)?;
        let stamp = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%S%.3fZ").to_string();
        fs::copy(path, dir.join(format!("{name}.{stamp}.json")))
            .with_context(|| format!("cannot back up {}", path.display()))?;

        // Exact suffix shape, so backups of "work.2" never count as "work".
        let prefix = format!("{name}.");
        let suffix_len = stamp.len() + ".json".len();
        let mut backups: Vec<_> = fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|f| {
                f.strip_prefix(&prefix).is_some_and(|rest| {
                    rest.len() == suffix_len && rest.starts_with(|c: char| c.is_ascii_digit())
                })
            })
            .collect();
        backups.sort();
        let excess = backups.len().saturating_sub(KEEP_BACKUPS);
        for old in &backups[..excess] {
            fs::remove_file(dir.join(old))?;
        }
        Ok(())
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !valid {
        bail!("invalid session name '{name}': use letters, digits, '-', '_' or '.'");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Workspaces;

    fn session(name: &str) -> Session {
        Session {
            schema: SCHEMA,
            name: name.into(),
            created: "2026-10-30T17:00:00Z".into(),
            host: "host".into(),
            shell_version: "51.1".into(),
            workspaces: Workspaces { count: 2, dynamic: true, only_on_primary: true },
            monitors: Vec::new(),
            apps: Vec::new(),
        }
    }

    #[test]
    fn save_load_list_delete_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path().join("sessions"));
        assert!(store.list().unwrap().is_empty());

        store.save(&session("work"), false).unwrap();
        store.save(&session("home"), false).unwrap();
        assert_eq!(store.load("work").unwrap().name, "work");
        let names: Vec<_> = store.list().unwrap().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["home", "work"]);

        store.delete("work").unwrap();
        assert!(!store.exists("work").unwrap());
        assert!(store.load("work").unwrap_err().to_string().contains("no session named"));
        assert_eq!(fs::read_dir(store.dir().join("backups")).unwrap().count(), 1);
    }

    #[test]
    fn overwrite_requires_force_and_backups_are_rotated() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path());
        store.save(&session("work"), false).unwrap();
        assert!(store.save(&session("work"), false).is_err());

        for _ in 0..KEEP_BACKUPS + 2 {
            store.save(&session("work"), true).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Backups of similarly named sessions are not counted or rotated.
        for other in ["work-old", "work.2"] {
            store.save(&session(other), false).unwrap();
            store.save(&session(other), true).unwrap();
        }

        let backups: Vec<_> = fs::read_dir(tmp.path().join("backups"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        let count = |prefix: &str| backups.iter().filter(|f| f.starts_with(prefix)).count();
        assert_eq!(count("work.2."), 1);
        assert_eq!(count("work-old."), 1);
        assert_eq!(count("work.") - count("work.2."), KEEP_BACKUPS);
    }

    #[test]
    fn rejects_unsafe_names() {
        for name in ["", "../x", "a/b", ".hidden", "with space"] {
            assert!(validate_name(name).is_err(), "{name:?} should be rejected");
        }
        validate_name("work-2026.10_a").unwrap();
    }
}
