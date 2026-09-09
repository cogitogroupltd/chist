use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// What to do when a session is missing locally but present in a backup archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    /// Ask on the terminal before extracting (default).
    Ask,
    /// Restore without asking.
    Auto,
    /// Never look in the archives.
    Never,
}

impl RestoreMode {
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "always" | "yes" | "true" => RestoreMode::Auto,
            "never" | "off" | "no" | "false" => RestoreMode::Never,
            _ => RestoreMode::Ask,
        }
    }
}

pub struct BackupConfig {
    /// Where archives live. Written to, and searched on a local miss.
    pub dir: PathBuf,
    /// Run backups automatically in the background on chist invocations.
    pub auto: bool,
    /// Days between automatic backups.
    pub interval_days: u64,
    /// Filename stem for the tarball: `<prefix>_DD-MM-YYYY.tar.gz`.
    pub archive_prefix: String,
    /// Filename stem for the session index: `<prefix>_DD-MM-YYYY.json`.
    pub index_prefix: String,
    /// How many sessions to write into the index.
    pub index_limit: usize,
    /// Paths to archive. `~` is expanded; missing paths are skipped.
    pub include: Vec<String>,
    /// Behaviour when a session is only found in an archive.
    pub restore: RestoreMode,
}

impl BackupConfig {
    /// Default include list: everything Claude Code owns that is worth keeping.
    fn default_include() -> Vec<String> {
        [
            "~/.claude/projects",
            "~/.claude/history.jsonl",
            "~/.claude/todos",
            "~/.claude/usage-data",
            "~/.claude/CLAUDE.md",
            "~/.claude/settings.json",
            "~/.claude/commands",
            "~/.claude/hooks",
            "~/.claude/mcp.json",
            "~/.claude/plugins/installed_plugins.json",
            "~/.claude/plugins/known_marketplaces.json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn default_dir() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        std::env::var("XDG_DATA_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("chist/backups")
    }

    fn load(data: &Map<String, Value>) -> Self {
        let section = data
            .get("backup")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let dir = std::env::var("CHIST_BACKUP_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| PathBuf::from(expand_home(&s)))
            .or_else(|| {
                section
                    .get("dir")
                    .and_then(|v| v.as_str())
                    .map(|s| PathBuf::from(expand_home(s)))
            })
            .unwrap_or_else(Self::default_dir);

        // Automatic backups are opt-in by existence: if the archive directory is
        // already there, keep it fed. A fresh install stays quiet until the user
        // runs `chist backup --now` or sets `backup.auto: true`.
        let auto = section
            .get("auto")
            .and_then(as_bool)
            .unwrap_or_else(|| dir.is_dir());

        let interval_days = section
            .get("interval_days")
            .and_then(|v| v.as_u64())
            .unwrap_or_else(default_interval_days)
            .max(1);

        let include = section
            .get("include")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .filter(|v: &Vec<String>| !v.is_empty())
            .unwrap_or_else(Self::default_include);

        BackupConfig {
            dir,
            auto,
            interval_days,
            archive_prefix: section
                .get("archive_prefix")
                .and_then(|v| v.as_str())
                .unwrap_or("kube_aws_zsh_hist")
                .to_string(),
            index_prefix: section
                .get("index_prefix")
                .and_then(|v| v.as_str())
                .unwrap_or("claude_sessions")
                .to_string(),
            index_limit: section
                .get("index_limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(500) as usize,
            include,
            restore: section
                .get("restore")
                .and_then(|v| v.as_str())
                .map(RestoreMode::parse)
                .unwrap_or(RestoreMode::Ask),
        }
    }
}

/// Back up often enough that nothing can be born and reaped between two runs.
/// Claude Code prunes `~/.claude/projects` after `cleanupPeriodDays` (default 30),
/// so a quarter of that window — capped at a week — always catches a session.
fn default_interval_days() -> u64 {
    let cleanup = dirs::home_dir()
        .map(|h| h.join(".claude/settings.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        .and_then(|v| v.get("cleanupPeriodDays").and_then(|v| v.as_u64()))
        .unwrap_or(30);

    (cleanup / 4).clamp(1, 7)
}

pub fn expand_home(s: &str) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let home = home.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else if s == "~" {
        home.to_string()
    } else {
        s.to_string()
    }
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub struct Config {
    pub claude_home: PathBuf,
    pub allowed_projects: Option<Vec<String>>,
    pub default_list_limit: usize,
    pub default_format: String,
    pub backup: BackupConfig,
}

impl Config {
    pub fn load(config_file: Option<&Path>) -> Self {
        let config_path = config_file.map(PathBuf::from).unwrap_or_else(|| {
            // XDG-style default: ~/.config/chist/config.yaml
            // Falls back to ~/.chist.yaml for users who prefer that, then to the
            // legacy ~/.claudehist.yaml location for back-compat with the pre-rename tool.
            let home = dirs::home_dir().unwrap_or_default();
            let candidates = [
                home.join(".config/chist/config.yaml"),
                home.join(".chist.yaml"),
                home.join(".claudehist.yaml"),
            ];
            candidates
                .iter()
                .find(|p| p.exists())
                .cloned()
                .unwrap_or_else(|| candidates[0].clone())
        });

        let data = Self::load_yaml(&config_path);

        let claude_home = std::env::var("CLAUDE_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                data.get("claude_home")
                    .and_then(|v| v.as_str())
                    .map(|s| PathBuf::from(expand_home(s)))
            })
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude"));

        let allowed_projects = data.get("allowed_projects").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
        });

        let defaults = data
            .get("defaults")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let default_list_limit = defaults
            .get("list_limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20) as usize;

        let default_format = defaults
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("table")
            .to_string();

        Config {
            claude_home,
            allowed_projects,
            default_list_limit,
            default_format,
            backup: BackupConfig::load(&data),
        }
    }

    /// Load YAML config as JSON value (we parse the subset we need manually
    /// to avoid pulling in a full YAML dep — config files are simple key: value).
    fn load_yaml(path: &Path) -> Map<String, Value> {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Map::new();
        };
        parse_yaml(&content)
    }
}

/// Parse the config subset: top-level scalars and lists, plus one level of
/// nested maps whose own values may themselves be scalars or lists.
///
/// ```yaml
/// claude_home: ~/.claude
/// allowed_projects:
///   - ~/dev/*
/// backup:
///   interval_days: 7
///   include:
///     - ~/.claude/projects
/// ```
fn parse_yaml(content: &str) -> Map<String, Value> {
    let mut root = Map::new();
    // The section we are indented inside, if any.
    let mut section: Option<(String, Map<String, Value>)> = None;
    // The key whose value is still open — a list or a map may follow it.
    let mut open_key: Option<String> = None;
    let mut list: Vec<Value> = Vec::new();

    // Close the open key with whatever list has accumulated under it.
    fn flush_list(
        root: &mut Map<String, Value>,
        section: &mut Option<(String, Map<String, Value>)>,
        open_key: &Option<String>,
        list: &mut Vec<Value>,
    ) {
        if list.is_empty() {
            return;
        }
        if let Some(key) = open_key {
            let value = Value::Array(std::mem::take(list));
            // A list under a top-level key means that key was never a section,
            // even though an empty value made it look like one.
            match section {
                Some((name, map)) if name != key => {
                    map.insert(key.clone(), value);
                }
                _ => {
                    root.insert(key.clone(), value);
                }
            }
        }
        list.clear();
    }

    fn flush_section(
        root: &mut Map<String, Value>,
        section: &mut Option<(String, Map<String, Value>)>,
    ) {
        if let Some((name, map)) = section.take() {
            root.insert(name, Value::Object(map));
        }
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(item) = trimmed.strip_prefix("- ") {
            list.push(scalar(item));
            continue;
        }

        flush_list(&mut root, &mut section, &open_key, &mut list);

        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        let value = value.trim();
        let indented = line.starts_with(' ') || line.starts_with('\t');

        if let Some((_, map)) = section.as_mut()
            && indented
        {
            if value.is_empty() {
                open_key = Some(key);
            } else {
                map.insert(key.clone(), scalar(value));
                open_key = Some(key);
            }
            continue;
        }

        // A top-level key ends any section that was open.
        flush_section(&mut root, &mut section);

        if value.is_empty() {
            // Either a list or a nested map follows; assume a map and let a
            // list flush overwrite it if `- ` items turn up instead.
            section = Some((key.clone(), Map::new()));
            open_key = Some(key);
        } else {
            root.insert(key.clone(), scalar(value));
            open_key = Some(key);
        }
    }

    flush_list(&mut root, &mut section, &open_key, &mut list);
    // An empty section is indistinguishable from a key with no value; drop it.
    if let Some((name, map)) = section.take()
        && !map.is_empty()
    {
        root.insert(name, Value::Object(map));
    }

    root
}

fn scalar(raw: &str) -> Value {
    let s = raw.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(s);

    match s {
        "true" | "yes" => Value::Bool(true),
        "false" | "no" => Value::Bool(false),
        _ => match s.parse::<u64>() {
            Ok(n) => Value::Number(n.into()),
            Err(_) => Value::String(s.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_level_scalars_and_lists() {
        let parsed = parse_yaml(
            "claude_home: ~/.claude\n\
             # a comment\n\
             allowed_projects:\n\
             \x20 - ~/dev/*\n\
             \x20 - ~/tmp\n",
        );
        assert_eq!(parsed["claude_home"], Value::String("~/.claude".into()));
        assert_eq!(parsed["allowed_projects"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn parses_nested_sections_including_lists() {
        let parsed = parse_yaml(
            "defaults:\n\
             \x20 list_limit: 40\n\
             \x20 format: json\n\
             backup:\n\
             \x20 dir: ~/backups\n\
             \x20 auto: true\n\
             \x20 interval_days: 3\n\
             \x20 include:\n\
             \x20   - ~/.claude/projects\n\
             \x20   - ~/.zshrc\n",
        );
        assert_eq!(
            parsed["defaults"]["list_limit"],
            Value::Number(40u64.into())
        );
        assert_eq!(parsed["defaults"]["format"], Value::String("json".into()));
        assert_eq!(parsed["backup"]["dir"], Value::String("~/backups".into()));
        assert_eq!(parsed["backup"]["auto"], Value::Bool(true));
        assert_eq!(parsed["backup"]["include"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn backup_defaults_are_sane_without_config() {
        let backup = BackupConfig::load(&Map::new());
        assert_eq!(backup.archive_prefix, "kube_aws_zsh_hist");
        assert_eq!(backup.index_prefix, "claude_sessions");
        assert!((1..=7).contains(&backup.interval_days));
        assert_eq!(backup.restore, RestoreMode::Ask);
        assert!(!backup.include.is_empty());
    }

    #[test]
    fn restore_mode_parses_synonyms() {
        assert_eq!(RestoreMode::parse("auto"), RestoreMode::Auto);
        assert_eq!(RestoreMode::parse("never"), RestoreMode::Never);
        assert_eq!(RestoreMode::parse("ask"), RestoreMode::Ask);
        assert_eq!(RestoreMode::parse("wat"), RestoreMode::Ask);
    }
}
