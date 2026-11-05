use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

use crate::model::{model_family, ModelFamily};
use crate::spirit::theme::Theme;

/// How session cards are drawn (`v` switches; saved as `card_layout`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CardLayout {
    /// A: 29 x 9 cards, the small 7 x 4 spirit left of name, badge, branch/PR
    /// and a summary on up to four lines.
    #[default]
    Standard,
    /// B: 46 x 10 cards (stretched to fill the grid row), the bigger 11 x 4
    /// spirit left of more text (model word, branch and
    /// PR rows, a summary on up to three lines).
    Large,
}

impl CardLayout {
    /// The other layout (there are two).
    pub fn next(self) -> Self {
        match self {
            CardLayout::Standard => CardLayout::Large,
            CardLayout::Large => CardLayout::Standard,
        }
    }
}

/// Reads `card_layout` leniently: `"large"` is the large layout, anything
/// else (an unknown name, another type) falls back to the default instead of
/// failing the whole file.
fn lenient_layout<'de, D: Deserializer<'de>>(d: D) -> Result<CardLayout, D::Error> {
    let v = toml::Value::deserialize(d)?;
    Ok(match v.as_str() {
        Some("large") => CardLayout::Large,
        _ => CardLayout::Standard,
    })
}

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
    /// Theme of sessions whose model is unknown. Files written before themes
    /// existed may still hold a `view` key; unknown keys are ignored, so they
    /// load fine.
    pub theme: Theme,
    pub extras: Extras,
    pub pr_poll_seconds: u64,
    pub guess_worktree: bool,
    pub done_retention_minutes: u64,
    /// Spirit element per model family, keyed by [`ModelFamily::as_key`]. A
    /// family missing from the map uses its default ([`default_model_theme`]).
    /// Older files may hold a `card_themes` table; it is ignored.
    pub model_themes: BTreeMap<String, Theme>,
    /// Context window size used when usage comes from the transcript (no
    /// statusline file). 0 = auto: 200k up to 200k tokens, 1M beyond. The
    /// former default 200000, written by every earlier save, also means auto.
    pub context_limit_tokens: u64,
    /// The statusline context file is trusted for this long after it was written.
    pub ctx_fresh_seconds: u64,
    /// Card layout; an unknown value loads as the default. (Not the removed
    /// `view` key, which older files may still hold and is ignored.)
    #[serde(deserialize_with = "lenient_layout")]
    pub card_layout: CardLayout,
    /// A subagent counts as running while its transcript was written within
    /// this many seconds (and it is not marked finished).
    pub subagent_active_seconds: u64,
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
            model_themes: default_model_themes(),
            context_limit_tokens: 0,
            ctx_fresh_seconds: 300,
            card_layout: CardLayout::default(),
            subagent_active_seconds: 30,
        }
    }
}

/// Built-in element of a model family (user decision: fable = water,
/// opus = fire, sonnet = lightning, haiku = poison).
pub fn default_model_theme(family: ModelFamily) -> Theme {
    match family {
        ModelFamily::Fable => Theme::Water,
        ModelFamily::Opus => Theme::Fire,
        ModelFamily::Sonnet => Theme::Lightning,
        ModelFamily::Haiku => Theme::Poison,
    }
}

/// The full default mapping, as written to a fresh config file.
pub fn default_model_themes() -> BTreeMap<String, Theme> {
    ModelFamily::ALL.into_iter().map(|f| (f.as_key().to_string(), default_model_theme(f))).collect()
}

/// Footer note when the config file exists but cannot be read or parsed.
pub const LOAD_FAILED: &str = "설정 파일을 읽지 못해 기본값을 씁니다";
/// Backups are `<name>.bak`, then `<name>.bak.1` … `<name>.bak.MAX_BACKUPS`.
pub const MAX_BACKUPS: usize = 5;

impl Config {
    /// Element of a model family: the configured one, else its default.
    pub fn model_theme(&self, family: ModelFamily) -> Theme {
        self.model_themes.get(family.as_key()).copied().unwrap_or_else(|| default_model_theme(family))
    }

    /// Element of a session running `model`: its family's element, or
    /// [`Config::theme`] when the model is missing or not a known family.
    pub fn theme_for_model(&self, model: Option<&str>) -> Theme {
        model.and_then(model_family).map_or(self.theme, |f| self.model_theme(f))
    }

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

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// `<home>/.config/cmux-magigent/config.toml`: where the config is written.
pub fn config_path_in(home: &Path) -> PathBuf {
    home.join(".config").join("cmux-magigent").join("config.toml")
}

/// The config location of the app's former name (read-only fallback).
pub fn legacy_config_path_in(home: &Path) -> PathBuf {
    home.join(".config").join("cmux-cc-monitor").join("config.toml")
}

/// Where the config is saved.
pub fn default_path() -> PathBuf {
    config_path_in(&home())
}

/// The file to load: `new` when anything exists there (even a broken link, so
/// a problem is reported rather than skipped), else `legacy` when it exists,
/// else `new`. Never moves or deletes anything.
pub fn resolve_load_path(new: &Path, legacy: &Path) -> PathBuf {
    if std::fs::symlink_metadata(new).is_err() && std::fs::symlink_metadata(legacy).is_ok() {
        legacy.to_path_buf()
    } else {
        new.to_path_buf()
    }
}

/// The file to load at startup (see [`resolve_load_path`]).
pub fn load_path() -> PathBuf {
    let h = home();
    resolve_load_path(&config_path_in(&h), &legacy_config_path_in(&h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_paths_use_cmux_magigent_with_legacy_beside() {
        let home = Path::new("/h");
        assert_eq!(config_path_in(home), Path::new("/h/.config/cmux-magigent/config.toml"));
        assert_eq!(legacy_config_path_in(home), Path::new("/h/.config/cmux-cc-monitor/config.toml"));
    }

    #[test]
    fn legacy_config_is_read_when_new_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let new = config_path_in(dir.path());
        let old = legacy_config_path_in(dir.path());
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "pr_poll_seconds = 77\n").unwrap();
        let load = resolve_load_path(&new, &old);
        assert_eq!(load, old);
        assert_eq!(Config::load(&load).pr_poll_seconds, 77);
        // saving goes to the new location; the legacy file is untouched
        let mut c = Config::load(&load);
        c.pr_poll_seconds = 88;
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        c.save(&new).unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "pr_poll_seconds = 77\n");
        assert_eq!(resolve_load_path(&new, &old), new, "new wins once it exists");
        assert_eq!(Config::load(&new).pr_poll_seconds, 88);
    }

    #[test]
    fn no_config_anywhere_loads_new_path_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let new = config_path_in(dir.path());
        let old = legacy_config_path_in(dir.path());
        assert_eq!(resolve_load_path(&new, &old), new);
    }

    #[test]
    fn config_defaults_match_spec() {
        let c = Config::default();
        assert_eq!(c.theme, Theme::Fire);
        assert_eq!(c.model_themes, default_model_themes());
        assert!(!c.extras.pr);
        assert!(!c.extras.last_activity);
        assert!(c.extras.summary);
        assert!(c.extras.branch);
        assert!(c.extras.elapsed);
        assert_eq!(c.pr_poll_seconds, 45);
        assert_eq!(c.done_retention_minutes, 10);
        assert!(c.guess_worktree);
        assert_eq!(c.context_limit_tokens, 0, "0 = auto (window inferred)");
        assert_eq!(c.ctx_fresh_seconds, 300);
    }

    #[test]
    fn old_config_without_context_keys_loads_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"water\"\npr_poll_seconds = 30\nsome_future_key = 1\n").unwrap();
        let (c, err) = Config::load_checked(&path);
        assert_eq!(err, None);
        assert_eq!((c.context_limit_tokens, c.ctx_fresh_seconds), (0, 300));
        std::fs::write(&path, "context_limit_tokens = 1000000\nctx_fresh_seconds = 60\n").unwrap();
        let c = Config::load(&path);
        assert_eq!((c.context_limit_tokens, c.ctx_fresh_seconds), (1_000_000, 60));
        // files saved by earlier versions hold the old default: loads unchanged
        // (the context code reads that value as auto)
        std::fs::write(&path, "context_limit_tokens = 200000\n").unwrap();
        let (c, err) = Config::load_checked(&path);
        assert_eq!((c.context_limit_tokens, err), (crate::source::context::LEGACY_DEFAULT_LIMIT, None));
        // auto round-trips
        Config::default().save(&path).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("context_limit_tokens = 0"));
        assert_eq!(Config::load(&path), Config::default());
    }

    #[test]
    fn subagent_window_defaults_to_30s_and_old_configs_load() {
        assert_eq!(Config::default().subagent_active_seconds, 30);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"water\"\npr_poll_seconds = 30\n").unwrap();
        let (c, err) = Config::load_checked(&path);
        assert_eq!((c.subagent_active_seconds, c.pr_poll_seconds, err), (30, 30, None));
        std::fs::write(&path, "subagent_active_seconds = 90\n").unwrap();
        assert_eq!(Config::load(&path).subagent_active_seconds, 90);
        Config::default().save(&path).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("subagent_active_seconds = 30"));
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
        std::fs::write(&path, "theme = \"poison\"\n[extras]\npr = true\n").unwrap();
        let c = Config::load(&path);
        assert_eq!(c.theme, Theme::Poison);
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
        std::fs::write(&real, "theme = \"poison\"\n").unwrap();
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
        assert_eq!(c.model_themes, default_model_themes());
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
        // a bad per-model theme is the same kind of failure
        std::fs::write(&path, "[model_themes]\nopus = \"plasma\"\n").unwrap();
        assert_eq!(Config::load_checked(&path), (Config::default(), Some(LOAD_FAILED.into())));
    }

    #[test]
    fn removed_ice_theme_is_an_unknown_theme() {
        // ice was replaced by poison: an old file naming it takes the unknown-theme path
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "theme = \"ice\"\npr_poll_seconds = 30\n";
        std::fs::write(&path, text).unwrap();
        assert_eq!(Config::load_checked(&path), (Config::default(), Some(LOAD_FAILED.into())));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "loading never rewrites the file");
        std::fs::write(&path, "[model_themes]\nhaiku = \"ice\"\n").unwrap();
        assert_eq!(Config::load_checked(&path), (Config::default(), Some(LOAD_FAILED.into())));
    }

    #[test]
    fn removed_melee_theme_is_an_unknown_theme() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for text in ["theme = \"melee\"\n", "[model_themes]\nopus = \"melee\"\n"] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(Config::load_checked(&path), (Config::default(), Some(LOAD_FAILED.into())), "{text}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "loading never rewrites the file");
        }
    }

    #[test]
    fn default_model_mapping() {
        let c = Config::default();
        let want = [
            (ModelFamily::Fable, Theme::Water),
            (ModelFamily::Opus, Theme::Fire),
            (ModelFamily::Sonnet, Theme::Lightning),
            (ModelFamily::Haiku, Theme::Poison),
        ];
        for (f, t) in want {
            assert_eq!(c.model_themes.get(f.as_key()), Some(&t), "{f:?}");
            assert_eq!(c.model_theme(f), t, "{f:?}");
        }
        assert_eq!(c.model_themes.len(), 4);
    }

    #[test]
    fn theme_for_model_uses_mapping_then_fallback() {
        let mut c = Config::default();
        assert_eq!(c.theme_for_model(Some("claude-opus-4-5")), Theme::Fire);
        assert_eq!(c.theme_for_model(Some("claude-fable-5-1")), Theme::Water);
        assert_eq!(c.theme_for_model(Some("claude-sonnet-5-5")), Theme::Lightning);
        assert_eq!(c.theme_for_model(Some("claude-haiku-5-5")), Theme::Poison);
        // unknown or missing model: the configured default theme
        c.theme = Theme::Water;
        for m in [None, Some(""), Some("<synthetic>"), Some("gpt-5")] {
            assert_eq!(c.theme_for_model(m), Theme::Water, "{m:?}");
        }
        // a custom mapping is honored
        c.model_themes.insert("opus".into(), Theme::Lightning);
        assert_eq!(c.theme_for_model(Some("us.anthropic.claude-opus-4-5")), Theme::Lightning);
        // a missing family entry falls back to that family's default, not cfg.theme
        c.model_themes.remove("haiku");
        assert_eq!(c.theme_for_model(Some("claude-haiku-5-5")), Theme::Poison);
    }

    #[test]
    fn partial_model_map_falls_back_per_family() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"water\"\n[model_themes]\nopus = \"poison\"\nfuture-model = \"fire\"\n").unwrap();
        let (c, err) = Config::load_checked(&path);
        assert_eq!(err, None);
        assert_eq!(c.model_theme(ModelFamily::Opus), Theme::Poison);
        assert_eq!(c.model_theme(ModelFamily::Fable), Theme::Water);
        assert_eq!(c.model_theme(ModelFamily::Sonnet), Theme::Lightning);
        assert_eq!(c.model_theme(ModelFamily::Haiku), Theme::Poison);
        assert_eq!(c.theme_for_model(Some("claude-sonnet-5-5")), Theme::Lightning);
    }

    #[test]
    fn old_config_with_card_themes_loads() {
        // card_themes (per-card overrides) was removed: the key is ignored
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"lightning\"\n[card_themes]\n\"/w/proj\" = \"water\"\nsess-1 = \"fire\"\n").unwrap();
        let (c, err) = Config::load_checked(&path);
        assert_eq!(err, None);
        assert_eq!(c.theme, Theme::Lightning);
        assert_eq!(c.model_themes, default_model_themes());
    }

    #[test]
    fn themes_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = Config::default();
        c.theme = Theme::Poison;
        c.model_themes.insert("opus".into(), Theme::Lightning);
        c.model_themes.insert("fable".into(), Theme::Fire);
        c.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("theme = \"poison\""), "{text}");
        assert!(text.contains("[model_themes]") && text.contains("opus = \"lightning\""), "{text}");
        assert_eq!(Config::load(&path), c);
    }

    #[test]
    fn card_layout_defaults_to_standard_and_round_trips() {
        assert_eq!(Config::default().card_layout, CardLayout::Standard);
        assert_eq!(CardLayout::Standard.next(), CardLayout::Large);
        assert_eq!(CardLayout::Large.next(), CardLayout::Standard);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = Config::default();
        c.card_layout = CardLayout::Large;
        c.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("card_layout = \"large\""), "{text}");
        assert_eq!(Config::load_checked(&path), (c, None));
        Config::default().save(&path).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("card_layout = \"standard\""));
    }

    #[test]
    fn card_layout_unknown_or_old_values_load_as_standard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for text in [
            "card_layout = \"huge\"\npr_poll_seconds = 30\n",
            "card_layout = \"Large\"\npr_poll_seconds = 30\n",
            "card_layout = 3\npr_poll_seconds = 30\n",
            "card_layout = [\"large\"]\npr_poll_seconds = 30\n",
            "pr_poll_seconds = 30\n",
            // the removed `view` key is not the layout
            "view = \"b\"\npr_poll_seconds = 30\n",
        ] {
            std::fs::write(&path, text).unwrap();
            let (c, err) = Config::load_checked(&path);
            assert_eq!(err, None, "{text}: the rest of the file still loads");
            assert_eq!((c.card_layout, c.pr_poll_seconds), (CardLayout::Standard, 30), "{text}");
        }
        std::fs::write(&path, "card_layout = \"large\"\n").unwrap();
        assert_eq!(Config::load(&path).card_layout, CardLayout::Large);
    }

    #[test]
    fn default_path_ends_with_expected_suffix() {
        assert!(default_path().ends_with("cmux-magigent/config.toml"));
    }
}
