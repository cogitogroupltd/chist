use std::path::Path;

/// Convert a project path to Claude's directory naming convention.
/// Example: /home/cogito/dev/foo -> -home-cogito-dev-foo
pub fn path_to_claude_dir_name(project_path: &str) -> String {
    project_path.replace('/', "-")
}

/// Convert Claude's directory name back to a path.
/// Uses greedy algorithm to find longest existing path prefix.
pub fn claude_dir_name_to_path(dir_name: &str) -> String {
    let stripped = dir_name.strip_prefix('-').unwrap_or(dir_name);
    let parts: Vec<&str> = stripped.split('-').collect();

    let mut result_parts: Vec<String> = Vec::new();
    let mut i = 0;

    while i < parts.len() {
        let mut best_match = parts[i].to_string();
        let mut best_len = 1;

        // Try combining with subsequent parts (directory names with dashes)
        let max_j = (i + 6).min(parts.len() + 1);
        for j in (i + 1)..max_j {
            let candidate: String = parts[i..j].join("-");
            let test_path = format!("/{}", {
                let mut v = result_parts.clone();
                v.push(candidate.clone());
                v.join("/")
            });
            if Path::new(&test_path).exists() {
                best_match = candidate;
                best_len = j - i;
            }
        }

        result_parts.push(best_match);
        i += best_len;
    }

    format!("/{}", result_parts.join("/"))
}

/// Extract project name (last 2 path components).
pub fn get_project_name(project_path: &str) -> String {
    let parts: Vec<&str> = project_path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else if let Some(last) = parts.last() {
        last.to_string()
    } else {
        String::new()
    }
}

/// Check if project path matches any allowed glob patterns.
pub fn matches_allowed_projects(project_path: &str, allowed_patterns: Option<&[String]>) -> bool {
    let Some(patterns) = allowed_patterns else {
        return true;
    };
    if patterns.is_empty() {
        return true;
    }

    let home = dirs::home_dir().unwrap_or_default();
    let home_str = home.to_string_lossy();

    for pattern in patterns {
        let expanded = pattern.replace('~', &home_str);
        if let Ok(glob_pattern) = glob::Pattern::new(&expanded)
            && glob_pattern.matches(project_path)
        {
            return true;
        }
    }
    false
}

/// Check if a project path is under /tmp (but NOT ~/tmp).
/// Returns true if the session should be hidden (is a /tmp session).
pub fn is_tmp_session(project_path: &str) -> bool {
    // /tmp/... should be hidden, but /home/*/tmp/... should NOT
    project_path.starts_with("/tmp/") || project_path == "/tmp"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_to_dir_name_replaces_slashes() {
        assert_eq!(path_to_claude_dir_name("/home/foo/bar"), "-home-foo-bar");
        assert_eq!(path_to_claude_dir_name("/tmp"), "-tmp");
    }

    #[test]
    fn project_name_uses_last_two_components() {
        assert_eq!(get_project_name("/home/alice/dev/widget"), "dev/widget");
        assert_eq!(get_project_name("/home/alice"), "home/alice");
        assert_eq!(get_project_name("/widget"), "widget");
        assert_eq!(get_project_name(""), "");
    }

    #[test]
    fn tmp_check_excludes_user_tmp() {
        assert!(is_tmp_session("/tmp/foo"));
        assert!(is_tmp_session("/tmp"));
        assert!(!is_tmp_session("/home/alice/tmp/foo"));
        assert!(!is_tmp_session("/var/tmp/foo"));
    }

    #[test]
    fn allowed_projects_none_means_anything_goes() {
        assert!(matches_allowed_projects("/anywhere", None));
        assert!(matches_allowed_projects("/anywhere", Some(&[])));
    }
}
