use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::spirit::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Extras {
    pub branch: bool,
    pub elapsed: bool,
    pub last_activity: bool,
    pub summary: bool,
    pub pr: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Default card theme. Files written before themes existed may still hold
    /// a `view` key; unknown keys are ignored, so they load fine.
    pub theme: Theme,
    pub extras: Extras,
    pub pr_poll_seconds: u64,
    pub guess_worktree: bool,
    pub done_retention_minutes: u64,
    /// Per-card theme overrides, keyed by the session's cwd (its id when the cwd is empty).
    pub card_themes: BTreeMap<String, Theme>,
}

impl Default for Extras {
    fn default() -> Self {
        Self {
            branch: true,
            elapsed: true,
            last_activity: false,
            summary: true,
            pr: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            extras: Extras::default(),
            pr_poll_seconds: 45,
            guess_worktree: true,
            done_retention_minutes: 10,
            card_themes: BTreeMap::new(),
        }
    }
}

/// Footer note when the config file exists but cannot be read or parsed.
pub const LOAD_FAILED: &str = "설정 파일을 읽지 못해 기본값을 씁니다";
/// Backups are `<name>.bak`, then `<name>.bak.1` … `<name>.bak.MAX_BACKUPS`.
pub const MAX_BACKUPS: usize = 5;

impl Config {
    /// Loads the config; a missing or unparsable file yields the defaults.
    /// The app itself uses [`Config::load_checked`] to report the failure.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn load(path: &Path) -> Config {
        Self::load_checked(path).0
    }

    /// Like [`Config::load`], plus a short reason ([`LOAD_FAILED`]) when the
    /// file exists but cannot be read or parsed. A missing file is not an error.
    pub fn load_checked(path: &Path) -> (Config, Option<String>) {
        match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), None),
            Err(_) => (Config::default(), Some(LOAD_FAILED.into())),
            Ok(text) => match toml::from_str(&text) {
                Ok(c) => (c, None),
                Err(_) => (Config::default(), Some(LOAD_FAILED.into())),
            },
        }
    }

    /// Writes the config atomically (temp file + rename). A symlinked target
    /// is resolved first so the real file is replaced and the link survives.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let text = toml::to_string_pretty(self)?;
        // Write next to the real file, then rename over it so readers never
        // see a half-written file.
        let mut tmp_name = real.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(format!(".tmp{}", std::process::id()));
        let tmp = real.with_file_name(tmp_name);
        let result = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &real));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        Ok(result?)
    }
}

/// Copies `path` to the first free `<name>.bak`, `<name>.bak.1`, … next to
/// it and returns the backup path. Never overwrites an existing file; fails
/// when every slot up to [`MAX_BACKUPS`] is taken.
pub fn backup(path: &Path) -> anyhow::Result<PathBuf> {
    use std::io::Write;
    let bytes = std::fs::read(path)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    for i in 0..=MAX_BACKUPS {
        let candidate = match i {
            0 => path.with_file_name(format!("{name}.bak")),
            n => path.with_file_name(format!("{name}.bak.{n}")),
        };
        // `create_new` fails when anything (even a dangling link) is there.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(mut f) => {
                f.write_all(&bytes)?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("no free backup name")
}

pub fn default_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("cmux-cc-monitor")
        .join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_match_spec() {
        let c = Config::default();
        assert_eq!(c.theme, Theme::Fire);
        assert!(c.card_themes.is_empty());
        assert!(!c.extras.pr);
        assert!(!c.extras.last_activity);
        assert!(c.extras.summary);
        assert!(c.extras.branch);
        assert!(c.extras.elapsed);
        assert_eq!(c.pr_poll_seconds, 45);
        assert_eq!(c.done_retention_minutes, 10);
        assert!(c.guess_worktree);
    }

    #[test]
    fn config_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let mut c = Config::default();
        c.theme = Theme::Water;
        c.extras.pr = true;
        c.pr_poll_seconds = 90;
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path), c);
    }

    #[test]
    fn save_is_atomic_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config::default().save(&path).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("config.toml")]);
    }

    #[test]
    fn save_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "pr_poll_seconds = 1\n# stale\n").unwrap();
        let mut c = Config::default();
        c.pr_poll_seconds = 77;
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path), c);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("stale"));
    }

    #[test]
    fn config_load_missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Config::load(&dir.path().join("nope.toml")), Config::default());
    }

    #[test]
    fn config_load_garbage_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "{{{").unwrap();
        assert_eq!(Config::load(&path), Config::default());
    }

    #[test]
    fn config_partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"ice\"\n[extras]\npr = true\n").unwrap();
        let c = Config::load(&path);
        assert_eq!(c.theme, Theme::Ice);
        assert!(c.extras.pr);
        assert!(c.extras.summary);
        assert_eq!(c.pr_poll_seconds, 45);
    }

    #[test]
    fn load_checked_reports_parse_failure_of_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"Fire\"\n").unwrap(); // enum values are lowercase
        let (c, err) = Config::load_checked(&path);
        assert_eq!(c, Config::default());
        assert_eq!(err.as_deref(), Some(LOAD_FAILED));
        assert_eq!(Config::load(&path), Config::default(), "load stays a thin wrapper");
    }

    #[test]
    fn load_checked_missing_file_is_silent_and_valid_file_ok() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Config::load_checked(&dir.path().join("nope.toml")), (Config::default(), None));
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"lightning\"\n").unwrap();
        let (c, err) = Config::load_checked(&path);
        assert_eq!((c.theme, err), (Theme::Lightning, None));
    }

    #[test]
    fn backup_is_created_next_to_the_file_and_never_clobbers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "first = broken").unwrap();
        let bak = backup(&path).unwrap();
        assert_eq!(bak, dir.path().join("config.toml.bak"));
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "first = broken");
        std::fs::write(&path, "second = broken").unwrap();
        let bak1 = backup(&path).unwrap();
        assert_eq!(bak1, dir.path().join("config.toml.bak.1"));
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "first = broken", "older backup untouched");
        assert_eq!(std::fs::read_to_string(&bak1).unwrap(), "second = broken");
        // all slots taken: error, nothing overwritten
        for i in 2..=MAX_BACKUPS {
            std::fs::write(dir.path().join(format!("config.toml.bak.{i}")), "x").unwrap();
        }
        assert!(backup(&path).is_err());
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "first = broken");
    }

    #[test]
    fn save_through_symlink_keeps_the_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("dotfiles");
        std::fs::create_dir(&real_dir).unwrap();
        let real = real_dir.join("config.toml");
        std::fs::write(&real, "theme = \"ice\"\n").unwrap();
        let link_dir = dir.path().join("cfg");
        std::fs::create_dir(&link_dir).unwrap();
        let link = link_dir.join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut c = Config::default();
        c.pr_poll_seconds = 99;
        c.save(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "still a symlink");
        assert_eq!(Config::load(&real), c, "content written to the real file");
        let names: Vec<_> = std::fs::read_dir(&link_dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("config.toml")], "no temp file left next to the link");
    }

    #[test]
    fn old_view_key_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "view = \"a\"\npr_poll_seconds = 30\n[extras]\npr = true\n").unwrap();
        let (c, err) = Config::load_checked(&path);
        assert_eq!(err, None, "the removed view key is ignored");
        assert_eq!(c.theme, Theme::default());
        assert!(c.card_themes.is_empty());
        assert_eq!(c.pr_poll_seconds, 30);
        assert!(c.extras.pr);
    }

    #[test]
    fn unknown_theme_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "theme = \"bogus\"\npr_poll_seconds = 30\n";
        std::fs::write(&path, text).unwrap();
        let (c, err) = Config::load_checked(&path);
        assert_eq!(c, Config::default());
        assert_eq!(err.as_deref(), Some(LOAD_FAILED));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "loading never rewrites the file");
        // a bad per-card theme is the same kind of failure
        std::fs::write(&path, "[card_themes]\n\"/w/proj\" = \"plasma\"\n").unwrap();
        assert_eq!(Config::load_checked(&path), (Config::default(), Some(LOAD_FAILED.into())));
    }

    #[test]
    fn themes_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = Config::default();
        c.theme = Theme::Ice;
        c.card_themes.insert("/w/proj-a".into(), Theme::Lightning);
        c.card_themes.insert("sess-1".into(), Theme::Water);
        c.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("theme = \"ice\""), "{text}");
        assert_eq!(Config::load(&path), c);
    }

    #[test]
    fn default_path_ends_with_expected_suffix() {
        assert!(default_path().ends_with("cmux-cc-monitor/config.toml"));
    }
}
