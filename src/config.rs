use std::path::{Path, PathBuf};

pub struct Config {
    pub claude_home: PathBuf,
    pub allowed_projects: Option<Vec<String>>,
    pub default_list_limit: usize,
    pub default_format: String,
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
                data.get("claude_home").and_then(|v| v.as_str()).map(|s| {
                    let expanded =
                        s.replace('~', &dirs::home_dir().unwrap_or_default().to_string_lossy());
                    PathBuf::from(expanded)
                })
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
        }
    }

    /// Load YAML config as JSON value (we parse the subset we need manually
    /// to avoid pulling in a full YAML dep — config files are simple key: value).
    fn load_yaml(path: &Path) -> serde_json::Map<String, serde_json::Value> {
        let Ok(content) = std::fs::read_to_string(path) else {
            return serde_json::Map::new();
        };

        // Simple YAML parser for our config format
        let mut map = serde_json::Map::new();
        let mut current_key: Option<String> = None;
        let mut list_items: Vec<serde_json::Value> = Vec::new();
        let mut in_list = false;
        let mut defaults_map = serde_json::Map::new();
        let mut in_defaults = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // Check for list item
            if let Some(stripped) = trimmed.strip_prefix("- ") {
                let val = stripped.trim();
                list_items.push(serde_json::Value::String(val.to_string()));
                in_list = true;
                continue;
            }

            // Flush any pending list
            if in_list {
                if let Some(ref key) = current_key {
                    map.insert(key.clone(), serde_json::Value::Array(list_items.clone()));
                }
                list_items.clear();
                in_list = false;
            }

            // Check for indented key (defaults sub-keys)
            if line.starts_with("  ") && in_defaults {
                if let Some((k, v)) = trimmed.split_once(':') {
                    let val = v.trim();
                    // Try parsing as number
                    if let Ok(n) = val.parse::<u64>() {
                        defaults_map
                            .insert(k.trim().to_string(), serde_json::Value::Number(n.into()));
                    } else {
                        defaults_map.insert(
                            k.trim().to_string(),
                            serde_json::Value::String(val.to_string()),
                        );
                    }
                }
                continue;
            }

            if in_defaults {
                map.insert(
                    "defaults".to_string(),
                    serde_json::Value::Object(defaults_map.clone()),
                );
                in_defaults = false;
            }

            // Top-level key: value
            if let Some((k, v)) = trimmed.split_once(':') {
                let key = k.trim().to_string();
                let val = v.trim();

                if key == "defaults" {
                    in_defaults = true;
                    defaults_map.clear();
                    continue;
                }

                if val.is_empty() {
                    // Next lines might be a list
                    current_key = Some(key);
                } else {
                    map.insert(key, serde_json::Value::String(val.to_string()));
                }
            }
        }

        // Flush
        if in_list && let Some(ref key) = current_key {
            map.insert(key.clone(), serde_json::Value::Array(list_items));
        }
        if in_defaults {
            map.insert(
                "defaults".to_string(),
                serde_json::Value::Object(defaults_map),
            );
        }

        map
    }
}
