use std::fs;
use std::path::Path;

/// Convert a project path to Claude's directory naming convention.
/// Claude replaces every non-alphanumeric character with a dash.
/// Example: /home/cogito/dev/foo.bar -> -home-cogito-dev-foo-bar
pub fn path_to_claude_dir_name(project_path: &str) -> String {
    project_path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Convert Claude's directory name back to a path.
///
/// The encoding is lossy — `.`, `_`, spaces and `/` all become `-` — so each
/// component is recovered by looking for a real directory entry whose own
/// encoded form matches the next run of parts, longest match first. Parts with
/// no match on disk fall back to a plain dash-joined component.
pub fn claude_dir_name_to_path(dir_name: &str) -> String {
    let stripped = dir_name.strip_prefix('-').unwrap_or(dir_name);
    let parts: Vec<&str> = stripped.split('-').collect();

    let mut result_parts: Vec<String> = Vec::new();
    let mut i = 0;

    while i < parts.len() {
        let parent = format!("/{}", result_parts.join("/"));
        match longest_entry_match(&parent, &parts[i..]) {
            Some((name, len)) => {
                result_parts.push(name);
                i += len;
            }
            None => {
                result_parts.push(parts[i].to_string());
                i += 1;
            }
        }
    }

    format!("/{}", result_parts.join("/"))
}

/// Find the entry in `parent` whose encoded name consumes the most leading
/// `parts`. Returns the real entry name and how many parts it accounts for.
fn longest_entry_match(parent: &str, parts: &[&str]) -> Option<(String, usize)> {
    let entries = fs::read_dir(Path::new(parent)).ok()?;
    let mut best: Option<(String, usize)> = None;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let encoded = path_to_claude_dir_name(&name);
        let len = encoded.split('-').count();
        if len > parts.len() || encoded.split('-').ne(parts[..len].iter().copied()) {
            continue;
        }
        if best.as_ref().is_none_or(|(_, best_len)| len > *best_len) {
            best = Some((name, len));
        }
    }

    best
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
    use std::fs;

    /// Create a unique scratch dir under the system temp dir.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("chist-pathtest-{}-{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Claude's own encoding, spelled out independently of the code under test.
    fn claude_encode(path: &str) -> String {
        path.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    }

    fn round_trip(real: &std::path::Path) -> String {
        let encoded = claude_encode(&real.to_string_lossy());
        assert_eq!(encoded, path_to_claude_dir_name(&real.to_string_lossy()));
        claude_dir_name_to_path(&encoded)
    }

    #[test]
    fn path_to_dir_name_encodes_non_alphanumerics() {
        assert_eq!(
            path_to_claude_dir_name("/home/foo/bar.baz.qux"),
            "-home-foo-bar-baz-qux"
        );
        assert_eq!(path_to_claude_dir_name("/home/a_b/c d"), "-home-a-b-c-d");
    }

    #[test]
    fn dir_name_round_trips_path_with_dots() {
        let root = scratch("dots");
        let real = root.join("fior").join("fiordc.aigateway.fior.group");
        fs::create_dir_all(&real).unwrap();
        assert_eq!(round_trip(&real), real.to_string_lossy());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn dir_name_round_trips_path_with_dashes() {
        let root = scratch("dashes");
        let real = root.join("dev").join("fior-mobileshield-v2");
        fs::create_dir_all(&real).unwrap();
        assert_eq!(round_trip(&real), real.to_string_lossy());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn dir_name_round_trips_long_name_with_spaces_and_parens() {
        let root = scratch("spaces");
        let real = root.join("auditor-pack-Sealed-evidence-bundle-20260814093433 (3)");
        fs::create_dir_all(&real).unwrap();
        assert_eq!(round_trip(&real), real.to_string_lossy());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn dir_name_for_missing_path_falls_back_to_dashes() {
        assert_eq!(
            claude_dir_name_to_path("-nonexistent-chist-project-dir"),
            "/nonexistent/chist/project/dir"
        );
    }

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
