use crate::models::{SessionDetail, SessionStatus, SessionSummary};
use crate::path_utils::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How recently the JSONL must have been modified to consider a session "active" vs "idle".
const ACTIVE_THRESHOLD: Duration = Duration::from_secs(120);

/// Count all descendant processes of a given PID by scanning /proc.
/// Returns the number of descendants (children, grandchildren, etc.).
/// A baseline idle Claude session has exactly 1 descendant (the node child).
fn count_descendants(root_pid: u64) -> usize {
    // Build a pid → parent map from /proc
    let Ok(entries) = fs::read_dir("/proc") else {
        return 0;
    };
    let mut parent_of: HashMap<u64, u64> = HashMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid_str) = name.to_str() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u64>() else {
            continue;
        };
        let stat_path = format!("/proc/{pid}/stat");
        if let Ok(stat) = fs::read_to_string(&stat_path) {
            // Format: "pid (comm) state ppid ..."
            // comm can contain spaces/parens, so find the last ')' first
            if let Some(after_comm) = stat.rfind(')') {
                let fields: Vec<&str> = stat[after_comm + 2..].split_whitespace().collect();
                // fields[0] = state, fields[1] = ppid
                if let Some(ppid) = fields.get(1).and_then(|s| s.parse::<u64>().ok()) {
                    parent_of.insert(pid, ppid);
                }
            }
        }
    }
    // Walk the tree: count everything whose ancestor chain includes root_pid
    let mut count = 0;
    for &pid in parent_of.keys() {
        let mut cur = pid;
        while let Some(&ppid) = parent_of.get(&cur) {
            if ppid == root_pid {
                count += 1;
                break;
            }
            if ppid <= 1 {
                break;
            }
            cur = ppid;
        }
    }
    count
}

struct RunningSession {
    session_id: String,
    cwd: String,
    started_at: u64,
    pid: u64,
}

pub struct SessionReader {
    session_meta_dir: PathBuf,
    facets_dir: PathBuf,
    projects_dir: PathBuf,
    sessions_dir: PathBuf,
    slug_cache: RefCell<HashMap<String, String>>,
    slug_cache_file: PathBuf,
}

impl SessionReader {
    pub fn new(claude_home: &Path) -> Self {
        // Cache dir: prefer ~/.cache/chist, fall back to legacy ~/.cache/claudehist if present.
        let home = dirs::home_dir().unwrap_or_default();
        let new_cache = home.join(".cache/chist");
        let legacy_cache = home.join(".cache/claudehist");
        let cache_dir = if new_cache.exists() || !legacy_cache.exists() {
            new_cache
        } else {
            legacy_cache
        };
        let slug_cache_file = cache_dir.join("slug_cache.json");

        let slug_cache = Self::load_slug_cache(&slug_cache_file);

        SessionReader {
            session_meta_dir: claude_home.join("usage-data/session-meta"),
            facets_dir: claude_home.join("usage-data/facets"),
            projects_dir: claude_home.join("projects"),
            sessions_dir: claude_home.join("sessions"),
            slug_cache: RefCell::new(slug_cache),
            slug_cache_file,
        }
    }

    fn load_slug_cache(path: &Path) -> HashMap<String, String> {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_slug_cache(&self) {
        if let Some(parent) = self.slug_cache_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&*self.slug_cache.borrow()) {
            let _ = fs::write(&self.slug_cache_file, json);
        }
    }

    /// Returns running sessions (PID alive) from lock files.
    fn get_running_sessions(&self) -> Vec<RunningSession> {
        let mut running = Vec::new();
        let Ok(entries) = fs::read_dir(&self.sessions_dir) else {
            return running;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(data) = serde_json::from_str::<Value>(&content) else {
                continue;
            };
            let Some(pid) = data.get("pid").and_then(|v| v.as_u64()) else {
                continue;
            };
            if !Path::new(&format!("/proc/{pid}")).exists() {
                continue;
            }
            let sid = data
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cwd = data
                .get("cwd")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let started_at = data.get("startedAt").and_then(|v| v.as_u64()).unwrap_or(0);
            if !sid.is_empty() {
                running.push(RunningSession {
                    session_id: sid,
                    cwd,
                    started_at,
                    pid,
                });
            }
        }
        running
    }

    /// Extract map of running session ID → PID.
    fn running_session_map(running: &[RunningSession]) -> HashMap<String, u64> {
        running
            .iter()
            .map(|r| (r.session_id.clone(), r.pid))
            .collect()
    }

    fn get_slug_from_jsonl(&self, session_id: &str, project_path: &str) -> Option<String> {
        let dir_name = path_to_claude_dir_name(project_path);
        let jsonl_path = self
            .projects_dir
            .join(&dir_name)
            .join(format!("{session_id}.jsonl"));

        let file = fs::File::open(&jsonl_path).ok()?;
        let reader = BufReader::new(file);

        let mut auto_slug: Option<String> = None;
        let mut custom_title: Option<String> = None;

        for (i, line) in reader.lines().enumerate() {
            let Ok(line) = line else { continue };

            // Auto-generated slug appears in the first ~20 lines
            if i <= 20 && auto_slug.is_none() {
                if let Ok(data) = serde_json::from_str::<Value>(&line)
                    && let Some(slug) = data.get("slug").and_then(|v| v.as_str())
                {
                    auto_slug = Some(slug.to_string());
                }
                continue;
            }

            // Only parse lines that contain "custom-title" (cheap string check
            // avoids JSON-parsing every line in large session files).
            if line.contains("\"custom-title\"")
                && let Ok(data) = serde_json::from_str::<Value>(&line)
                && data.get("type").and_then(|v| v.as_str()) == Some("custom-title")
                && let Some(title) = data.get("customTitle").and_then(|v| v.as_str())
            {
                custom_title = Some(title.to_string());
            }
        }
        custom_title.or(auto_slug)
    }

    fn get_last_message_from_jsonl(&self, session_id: &str, project_path: &str) -> Option<String> {
        let dir_name = path_to_claude_dir_name(project_path);
        let jsonl_path = self
            .projects_dir
            .join(&dir_name)
            .join(format!("{session_id}.jsonl"));

        let file = fs::File::open(&jsonl_path).ok()?;
        let reader = BufReader::new(file);
        let mut last_user_message: Option<String> = None;

        for line in reader.lines().map_while(Result::ok) {
            if let Ok(data) = serde_json::from_str::<Value>(&line)
                && data.get("type").and_then(|v| v.as_str()) == Some("user")
                && let Some(content) = data
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
            {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    last_user_message = Some(trimmed.to_string());
                }
            }
        }
        last_user_message
    }

    fn get_last_timestamp_from_jsonl(
        &self,
        session_id: &str,
        project_path: &str,
    ) -> Option<String> {
        let dir_name = path_to_claude_dir_name(project_path);
        let jsonl_path = self
            .projects_dir
            .join(&dir_name)
            .join(format!("{session_id}.jsonl"));

        let file = fs::File::open(&jsonl_path).ok()?;
        let reader = BufReader::new(file);
        let mut last_ts: Option<String> = None;

        for line in reader.lines().map_while(Result::ok) {
            if let Ok(data) = serde_json::from_str::<Value>(&line)
                && let Some(ts) = data.get("timestamp").and_then(|v| v.as_str())
                && !ts.is_empty()
            {
                last_ts = Some(ts.to_string());
            }
        }
        last_ts
    }

    fn get_slug_for_session(&self, session_id: &str, project_path: &str) -> Option<String> {
        // Always read from JSONL — custom titles (/rename) can be added after
        // the slug was cached, so the cache may be stale.
        if let Some(slug) = self.get_slug_from_jsonl(session_id, project_path) {
            let mut cache = self.slug_cache.borrow_mut();
            if cache.get(session_id) != Some(&slug) {
                cache.insert(session_id.to_string(), slug.clone());
                drop(cache);
                self.save_slug_cache();
            }
            return Some(slug);
        }

        // Fall back to cache for sessions whose JSONL may have been removed
        if let Some(slug) = self.slug_cache.borrow().get(session_id) {
            return Some(slug.clone());
        }

        None
    }

    fn find_session_by_slug(&self, slug: &str) -> Option<String> {
        // Check cache first (reverse lookup)
        for (session_id, cached_slug) in self.slug_cache.borrow().iter() {
            if cached_slug == slug {
                return Some(session_id.clone());
            }
        }

        // Scan session-meta files
        if !self.session_meta_dir.exists() {
            return None;
        }

        let entries: Vec<_> = fs::read_dir(&self.session_meta_dir)
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .collect();

        for entry in entries {
            let session_id = entry
                .path()
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            if let Ok(content) = fs::read_to_string(entry.path())
                && let Ok(data) = serde_json::from_str::<Value>(&content)
            {
                let project_path = data
                    .get("project_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(found) = self.get_slug_for_session(&session_id, project_path)
                    && found == slug
                {
                    return Some(session_id);
                }
            }
        }
        None
    }

    fn find_session_by_prefix(&self, prefix: &str) -> Option<String> {
        if self.session_meta_dir.exists()
            && let Some(found) = fs::read_dir(&self.session_meta_dir)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let stem = e.path().file_stem()?.to_string_lossy().to_string();
                    if stem.starts_with(prefix) {
                        Some(stem)
                    } else {
                        None
                    }
                })
                .next()
        {
            return Some(found);
        }
        // Fallback: meta cache missing this id — scan project JSONLs.
        self.find_session_in_projects(prefix).map(|(id, _)| id)
    }

    /// Scan `projects_dir/*/{prefix}*.jsonl` for an actual session file.
    /// Returns `(session_id, project_path)` for the first match.
    fn find_session_in_projects(&self, prefix: &str) -> Option<(String, String)> {
        if !self.projects_dir.exists() {
            return None;
        }
        for project_entry in fs::read_dir(&self.projects_dir).ok()?.flatten() {
            let dir_path = project_entry.path();
            if !dir_path.is_dir() {
                continue;
            }
            let dir_name = project_entry.file_name().to_string_lossy().to_string();
            if dir_name.starts_with('.') {
                continue;
            }
            let project_path = claude_dir_name_to_path(&dir_name);
            let Ok(jsonl_entries) = fs::read_dir(&dir_path) else {
                continue;
            };
            for jsonl_entry in jsonl_entries.flatten() {
                let path = jsonl_entry.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                    continue;
                };
                if stem.starts_with("agent-") {
                    continue;
                }
                if stem.starts_with(prefix) {
                    return Some((stem, project_path));
                }
            }
        }
        None
    }

    /// Build a `SessionDetail` directly from a session's JSONL when no meta cache
    /// entry is available. Populates message counts, first prompt, start time, git
    /// branch, summary, token usage and tool counts by streaming the file.
    fn build_detail_from_jsonl(
        &self,
        session_id: &str,
        project_path: &str,
    ) -> Option<SessionDetail> {
        let dir_name = path_to_claude_dir_name(project_path);
        let jsonl_path = self
            .projects_dir
            .join(&dir_name)
            .join(format!("{session_id}.jsonl"));
        let file = fs::File::open(&jsonl_path).ok()?;
        let buf = BufReader::new(file);

        let mut first_prompt = String::new();
        let mut user_count: u64 = 0;
        let mut assistant_count: u64 = 0;
        let mut start_time = String::new();
        let mut git_branch: Option<String> = None;
        let mut summary: Option<String> = None;
        let mut input_tokens: u64 = 0;
        let mut output_tokens: u64 = 0;
        let mut tool_counts: HashMap<String, u64> = HashMap::new();

        for line in buf.lines().map_while(Result::ok) {
            let Ok(data) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if start_time.is_empty()
                && let Some(ts) = data.get("timestamp").and_then(|v| v.as_str())
            {
                start_time = ts.to_string();
            }
            if git_branch.is_none()
                && let Some(b) = data.get("gitBranch").and_then(|v| v.as_str())
            {
                git_branch = Some(b.to_string());
            }
            if summary.is_none()
                && let Some(s) = data.get("summary").and_then(|v| v.as_str())
            {
                summary = Some(s.to_string());
            }
            match data.get("type").and_then(|v| v.as_str()) {
                Some("user") => {
                    user_count += 1;
                    if first_prompt.is_empty()
                        && let Some(c) = data
                            .get("message")
                            .and_then(|m| m.get("content"))
                            .and_then(|c| c.as_str())
                    {
                        let t = c.trim();
                        if !t.is_empty() {
                            first_prompt = t.to_string();
                        }
                    }
                }
                Some("assistant") => {
                    assistant_count += 1;
                    if let Some(usage) = data.get("message").and_then(|m| m.get("usage")) {
                        input_tokens += usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        output_tokens += usage
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                    }
                    if let Some(arr) = data
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_array())
                    {
                        for item in arr {
                            if item.get("type").and_then(|v| v.as_str()) == Some("tool_use")
                                && let Some(name) = item.get("name").and_then(|v| v.as_str())
                            {
                                *tool_counts.entry(name.to_string()).or_insert(0) += 1;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let slug = self.get_slug_for_session(session_id, project_path);

        Some(SessionDetail {
            session_id: session_id.to_string(),
            slug,
            project_path: project_path.to_string(),
            project_name: get_project_name(project_path),
            start_time,
            duration_minutes: 0,
            user_message_count: user_count,
            assistant_message_count: assistant_count,
            first_prompt,
            summary,
            git_branch,
            git_commits: 0,
            git_pushes: 0,
            tool_counts,
            languages: HashMap::new(),
            tool_errors: 0,
            tool_error_categories: HashMap::new(),
            lines_added: 0,
            lines_removed: 0,
            files_modified: 0,
            user_interruptions: 0,
            uses_task_agent: false,
            uses_mcp: false,
            uses_web_search: false,
            uses_web_fetch: false,
            input_tokens,
            output_tokens,
            underlying_goal: None,
            outcome: None,
            claude_helpfulness: None,
            session_type: None,
        })
    }

    fn get_git_branch(&self, session_id: &str, project_path: &str) -> Option<String> {
        let dir_name = path_to_claude_dir_name(project_path);
        let jsonl_path = self
            .projects_dir
            .join(&dir_name)
            .join(format!("{session_id}.jsonl"));

        let file = fs::File::open(&jsonl_path).ok()?;
        let reader = BufReader::new(file);

        for (i, line) in reader.lines().enumerate() {
            if i > 20 {
                break;
            }
            if let Ok(line) = line
                && let Ok(data) = serde_json::from_str::<Value>(&line)
                && let Some(branch) = data.get("gitBranch").and_then(|v| v.as_str())
            {
                return Some(branch.to_string());
            }
        }
        None
    }

    fn load_facets(&self, session_id: &str) -> Option<Value> {
        let path = self.facets_dir.join(format!("{session_id}.json"));
        let content = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn get_jsonl_file_size(&self, session_id: &str, project_path: &str) -> Option<u64> {
        let dir_name = path_to_claude_dir_name(project_path);
        let jsonl_path = self
            .projects_dir
            .join(&dir_name)
            .join(format!("{session_id}.jsonl"));
        fs::metadata(&jsonl_path).ok().map(|m| m.len())
    }

    /// Determine whether a running session is actively working or idle at the prompt.
    ///
    /// A session is "Running" if either:
    /// - The JSONL was modified within ACTIVE_THRESHOLD (Claude is reading/writing messages), OR
    /// - The process tree has descendants beyond the basic claude+node pair
    ///   (a tool subprocess like bash/sleep/curl is executing)
    ///
    /// Otherwise the session is "Idle" — alive but sitting at the prompt.
    fn status_for_running_session(
        &self,
        session_id: &str,
        project_path: &str,
        pid: u64,
    ) -> SessionStatus {
        // Signal 1: JSONL recently modified → actively exchanging messages
        let dir_name = path_to_claude_dir_name(project_path);
        let jsonl_path = self
            .projects_dir
            .join(&dir_name)
            .join(format!("{session_id}.jsonl"));
        if let Ok(meta) = fs::metadata(&jsonl_path)
            && let Ok(modified) = meta.modified()
            && let Ok(elapsed) = SystemTime::now().duration_since(modified)
            && elapsed < ACTIVE_THRESHOLD
        {
            return SessionStatus::Running;
        }

        // Signal 2: process tree has more than the baseline claude + node pair.
        // When Claude is executing a tool (Bash, etc.) there will be grandchild
        // processes (shell, sleep, curl, python, etc.).
        if count_descendants(pid) > 1 {
            return SessionStatus::Running;
        }

        SessionStatus::Idle
    }

    /// Should this session be included given the tmp filter?
    fn should_include(&self, project_path: &str, include_tmp: bool) -> bool {
        if include_tmp {
            return true;
        }
        !is_tmp_session(project_path)
    }

    pub fn list_sessions(
        &self,
        limit: Option<usize>,
        project_filter: Option<&str>,
        allowed_projects: Option<&[String]>,
        include_tmp: bool,
    ) -> Vec<SessionSummary> {
        // We need mutability for slug cache updates

        let running_sessions = self.get_running_sessions();
        let running_map = Self::running_session_map(&running_sessions);
        let mut sessions = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // Read sessions from metadata files
        if self.session_meta_dir.exists()
            && let Ok(entries) = fs::read_dir(&self.session_meta_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }

                let Ok(content) = fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(data) = serde_json::from_str::<Value>(&content) else {
                    continue;
                };

                let session_id = data
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or(""))
                    .to_string();

                let project_path = data
                    .get("project_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if !self.should_include(&project_path, include_tmp) {
                    continue;
                }

                if !matches_allowed_projects(&project_path, allowed_projects) {
                    continue;
                }

                let project_name = get_project_name(&project_path);

                if let Some(filter) = project_filter
                    && !project_name.to_lowercase().contains(&filter.to_lowercase())
                {
                    continue;
                }

                let slug = self.get_slug_for_session(&session_id, &project_path);
                let last_msg = self
                    .get_last_message_from_jsonl(&session_id, &project_path)
                    .map(|m| truncate(&m, 100));
                let cache_start_time = data
                    .get("start_time")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let last_activity = self
                    .get_last_timestamp_from_jsonl(&session_id, &project_path)
                    .unwrap_or_else(|| cache_start_time.clone());
                let first_prompt = truncate(
                    data.get("first_prompt")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    100,
                );
                let file_size = self.get_jsonl_file_size(&session_id, &project_path);
                let git_branch = self.get_git_branch(&session_id, &project_path);

                let status = if let Some(&pid) = running_map.get(&session_id) {
                    self.status_for_running_session(&session_id, &project_path, pid)
                } else {
                    SessionStatus::Stopped
                };
                sessions.push(SessionSummary {
                    session_id: session_id.clone(),
                    slug,
                    project_path,
                    project_name,
                    start_time: cache_start_time,
                    duration_minutes: data
                        .get("duration_minutes")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    user_message_count: data
                        .get("user_message_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    assistant_message_count: data
                        .get("assistant_message_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    first_prompt,
                    last_message: last_msg,
                    summary: data
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    file_size_bytes: file_size,
                    git_branch,
                    status,
                    last_activity,
                });
                seen.insert(session_id);
            }
        }

        // Scan project dirs for JSONL-only sessions
        if self.projects_dir.exists()
            && let Ok(entries) = fs::read_dir(&self.projects_dir)
        {
            for entry in entries.flatten() {
                let dir_path = entry.path();
                if !dir_path.is_dir() {
                    continue;
                }
                let dir_name = entry.file_name().to_string_lossy().to_string();
                if dir_name.starts_with('.') {
                    continue;
                }

                let project_path = claude_dir_name_to_path(&dir_name);

                if !self.should_include(&project_path, include_tmp) {
                    continue;
                }

                if !matches_allowed_projects(&project_path, allowed_projects) {
                    continue;
                }

                let project_name = get_project_name(&project_path);
                if let Some(filter) = project_filter
                    && !project_name.to_lowercase().contains(&filter.to_lowercase())
                {
                    continue;
                }

                let Ok(jsonl_entries) = fs::read_dir(&dir_path) else {
                    continue;
                };

                for jsonl_entry in jsonl_entries.flatten() {
                    let jsonl_path = jsonl_entry.path();
                    if jsonl_path.extension().is_none_or(|e| e != "jsonl") {
                        continue;
                    }

                    let session_id = jsonl_path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();

                    if session_id.starts_with("agent-") || seen.contains(&session_id) {
                        continue;
                    }

                    // Parse JSONL for basic info
                    let Ok(file) = fs::File::open(&jsonl_path) else {
                        continue;
                    };
                    let buf = BufReader::new(file);

                    let slug = self.get_slug_for_session(&session_id, &project_path);
                    let mut first_prompt = String::new();
                    let mut last_msg: Option<String> = None;
                    let mut user_count = 0u64;
                    let mut assistant_count = 0u64;
                    let mut start_time = String::new();
                    let mut last_activity = String::new();
                    let mut git_branch: Option<String> = None;

                    for line in buf.lines().map_while(Result::ok) {
                        if let Ok(data) = serde_json::from_str::<Value>(&line) {
                            if let Some(ts) = data.get("timestamp").and_then(|v| v.as_str())
                                && !ts.is_empty()
                            {
                                if start_time.is_empty() {
                                    start_time = ts.to_string();
                                }
                                last_activity = ts.to_string();
                            }
                            if git_branch.is_none()
                                && let Some(b) = data.get("gitBranch").and_then(|v| v.as_str())
                            {
                                git_branch = Some(b.to_string());
                            }
                            match data.get("type").and_then(|v| v.as_str()) {
                                Some("user") => {
                                    user_count += 1;
                                    if let Some(content) = data
                                        .get("message")
                                        .and_then(|m| m.get("content"))
                                        .and_then(|c| c.as_str())
                                    {
                                        let t = content.trim();
                                        if !t.is_empty() {
                                            if first_prompt.is_empty() {
                                                first_prompt = t.to_string();
                                            }
                                            last_msg = Some(t.to_string());
                                        }
                                    }
                                }
                                Some("assistant") => assistant_count += 1,
                                _ => {}
                            }
                        }
                    }

                    let file_size = fs::metadata(&jsonl_path).ok().map(|m| m.len());

                    let status = if let Some(&pid) = running_map.get(&session_id) {
                        self.status_for_running_session(&session_id, &project_path, pid)
                    } else {
                        SessionStatus::Stopped
                    };
                    sessions.push(SessionSummary {
                        session_id: session_id.clone(),
                        slug,
                        project_path: project_path.clone(),
                        project_name: project_name.clone(),
                        start_time: start_time.clone(),
                        duration_minutes: 0,
                        user_message_count: user_count,
                        assistant_message_count: assistant_count,
                        first_prompt: truncate(&first_prompt, 100),
                        last_message: last_msg.map(|m| truncate(&m, 100)),
                        summary: None,
                        file_size_bytes: file_size,
                        git_branch,
                        status,
                        last_activity: if last_activity.is_empty() {
                            start_time
                        } else {
                            last_activity
                        },
                    });
                    seen.insert(session_id);
                }
            }
        }

        // Inject running sessions that weren't found via meta/JSONL scan
        for rs in &running_sessions {
            if seen.contains(&rs.session_id) || rs.cwd.is_empty() {
                continue;
            }
            if !self.should_include(&rs.cwd, include_tmp) {
                continue;
            }
            if !matches_allowed_projects(&rs.cwd, allowed_projects) {
                continue;
            }
            if let Some(filter) = project_filter {
                let name = get_project_name(&rs.cwd);
                if !name.to_lowercase().contains(&filter.to_lowercase()) {
                    continue;
                }
            }
            let slug = self.get_slug_for_session(&rs.session_id, &rs.cwd);
            let git_branch = self.get_git_branch(&rs.session_id, &rs.cwd);
            let file_size = self.get_jsonl_file_size(&rs.session_id, &rs.cwd);
            // Convert epoch ms to ISO string
            let start_time = if rs.started_at > 0 {
                chrono::DateTime::from_timestamp_millis(rs.started_at as i64)
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let status = self.status_for_running_session(&rs.session_id, &rs.cwd, rs.pid);
            sessions.push(SessionSummary {
                session_id: rs.session_id.clone(),
                slug,
                project_path: rs.cwd.clone(),
                project_name: get_project_name(&rs.cwd),
                start_time: start_time.clone(),
                duration_minutes: 0,
                user_message_count: 0,
                assistant_message_count: 0,
                first_prompt: String::new(),
                last_message: None,
                summary: None,
                file_size_bytes: file_size,
                git_branch,
                status,
                last_activity: start_time,
            });
            seen.insert(rs.session_id.clone());
        }

        // Sort by last_activity descending (falls back to start_time when last_activity is empty)
        sessions.sort_by(|a, b| {
            let a_key = if a.last_activity.is_empty() {
                &a.start_time
            } else {
                &a.last_activity
            };
            let b_key = if b.last_activity.is_empty() {
                &b.start_time
            } else {
                &b.last_activity
            };
            b_key.cmp(a_key)
        });

        if let Some(limit) = limit {
            sessions.truncate(limit);
        }

        sessions
    }

    pub fn get_session(
        &self,
        id_or_slug: &str,
        allowed_projects: Option<&[String]>,
        include_tmp: bool,
    ) -> Option<SessionDetail> {
        let session_id = if id_or_slug.len() == 36 {
            id_or_slug.to_string()
        } else {
            // Try slug first, then UUID prefix
            self.find_session_by_slug(id_or_slug)
                .or_else(|| self.find_session_by_prefix(id_or_slug))?
        };

        let meta_file = self.session_meta_dir.join(format!("{session_id}.json"));
        let (data, project_path) = match fs::read_to_string(&meta_file)
            .ok()
            .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        {
            Some(data) => {
                let pp = data
                    .get("project_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                (data, pp)
            }
            None => {
                // Fallback: meta cache missing — build directly from the JSONL on disk.
                let (_, project_path) = self.find_session_in_projects(&session_id)?;
                if !self.should_include(&project_path, include_tmp) {
                    return None;
                }
                if !matches_allowed_projects(&project_path, allowed_projects) {
                    return None;
                }
                return self.build_detail_from_jsonl(&session_id, &project_path);
            }
        };

        if !self.should_include(&project_path, include_tmp) {
            return None;
        }

        if !matches_allowed_projects(&project_path, allowed_projects) {
            return None;
        }

        let slug = self.get_slug_for_session(&session_id, &project_path);
        let git_branch = self.get_git_branch(&session_id, &project_path);
        let facets = self.load_facets(&session_id);

        let tool_counts: HashMap<String, u64> = data
            .get("tool_counts")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_u64()?)))
                    .collect()
            })
            .unwrap_or_default();

        let languages: HashMap<String, u64> = data
            .get("languages")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_u64()?)))
                    .collect()
            })
            .unwrap_or_default();

        let tool_error_categories: HashMap<String, u64> = data
            .get("tool_error_categories")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_u64()?)))
                    .collect()
            })
            .unwrap_or_default();

        Some(SessionDetail {
            session_id,
            slug,
            project_path: project_path.clone(),
            project_name: get_project_name(&project_path),
            start_time: str_field(&data, "start_time"),
            duration_minutes: u64_field(&data, "duration_minutes"),
            user_message_count: u64_field(&data, "user_message_count"),
            assistant_message_count: u64_field(&data, "assistant_message_count"),
            first_prompt: str_field(&data, "first_prompt"),
            summary: data
                .get("summary")
                .and_then(|v| v.as_str())
                .map(String::from),
            git_branch,
            git_commits: u64_field(&data, "git_commits"),
            git_pushes: u64_field(&data, "git_pushes"),
            tool_counts,
            languages,
            tool_errors: u64_field(&data, "tool_errors"),
            tool_error_categories,
            lines_added: u64_field(&data, "lines_added"),
            lines_removed: u64_field(&data, "lines_removed"),
            files_modified: u64_field(&data, "files_modified"),
            user_interruptions: u64_field(&data, "user_interruptions"),
            uses_task_agent: bool_field(&data, "uses_task_agent"),
            uses_mcp: bool_field(&data, "uses_mcp"),
            uses_web_search: bool_field(&data, "uses_web_search"),
            uses_web_fetch: bool_field(&data, "uses_web_fetch"),
            input_tokens: u64_field(&data, "input_tokens"),
            output_tokens: u64_field(&data, "output_tokens"),
            underlying_goal: facets
                .as_ref()
                .and_then(|f| f.get("underlying_goal"))
                .and_then(|v| v.as_str())
                .map(String::from),
            outcome: facets
                .as_ref()
                .and_then(|f| f.get("outcome"))
                .and_then(|v| v.as_str())
                .map(String::from),
            claude_helpfulness: facets
                .as_ref()
                .and_then(|f| f.get("claude_helpfulness"))
                .and_then(|v| v.as_str())
                .map(String::from),
            session_type: facets
                .as_ref()
                .and_then(|f| f.get("session_type"))
                .and_then(|v| v.as_str())
                .map(String::from),
        })
    }

    pub fn get_last_session(
        &self,
        allowed_projects: Option<&[String]>,
        include_tmp: bool,
    ) -> Option<SessionDetail> {
        let sessions = self.list_sessions(Some(1), None, allowed_projects, include_tmp);
        let first = sessions.first()?;
        self.get_session(&first.session_id, allowed_projects, include_tmp)
    }

    pub fn search_sessions(
        &self,
        pattern: &str,
        use_regex: bool,
        case_insensitive: bool,
        limit: Option<usize>,
        allowed_projects: Option<&[String]>,
        include_tmp: bool,
    ) -> Vec<SessionSummary> {
        let regex_pattern = if use_regex {
            pattern.to_string()
        } else {
            regex::escape(pattern)
        };

        let re = if case_insensitive {
            regex::RegexBuilder::new(&regex_pattern)
                .case_insensitive(true)
                .build()
        } else {
            regex::Regex::new(&regex_pattern)
        };

        let Ok(re) = re else {
            // Fall back to escaped literal
            let escaped = regex::escape(pattern);
            let Ok(re) = regex::RegexBuilder::new(&escaped)
                .case_insensitive(case_insensitive)
                .build()
            else {
                return Vec::new();
            };
            return self.search_with_regex(&re, limit, allowed_projects, include_tmp);
        };

        self.search_with_regex(&re, limit, allowed_projects, include_tmp)
    }

    fn search_with_regex(
        &self,
        re: &regex::Regex,
        limit: Option<usize>,
        allowed_projects: Option<&[String]>,
        include_tmp: bool,
    ) -> Vec<SessionSummary> {
        let running_sessions = self.get_running_sessions();
        let running_map = Self::running_session_map(&running_sessions);
        let mut matching = Vec::new();
        let mut found_ids: HashSet<String> = HashSet::new();

        if !self.projects_dir.exists() {
            return matching;
        }

        let Ok(project_entries) = fs::read_dir(&self.projects_dir) else {
            return matching;
        };

        for entry in project_entries.flatten() {
            let dir_path = entry.path();
            if !dir_path.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if dir_name.starts_with('.') {
                continue;
            }

            let project_path = claude_dir_name_to_path(&dir_name);

            if !self.should_include(&project_path, include_tmp) {
                continue;
            }

            if !matches_allowed_projects(&project_path, allowed_projects) {
                continue;
            }

            let Ok(jsonl_entries) = fs::read_dir(&dir_path) else {
                continue;
            };

            for jsonl_entry in jsonl_entries.flatten() {
                let jsonl_path = jsonl_entry.path();
                if jsonl_path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }

                let session_id = jsonl_path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                if session_id.starts_with("agent-") || found_ids.contains(&session_id) {
                    continue;
                }

                // Search user messages
                let Ok(file) = fs::File::open(&jsonl_path) else {
                    continue;
                };
                let buf = BufReader::new(file);
                let mut found = false;

                for line in buf.lines().map_while(Result::ok) {
                    if let Ok(data) = serde_json::from_str::<Value>(&line)
                        && data.get("type").and_then(|v| v.as_str()) == Some("user")
                        && let Some(content) = data.get("message").and_then(|m| m.get("content"))
                    {
                        let text = if let Some(s) = content.as_str() {
                            s.to_string()
                        } else {
                            content.to_string()
                        };
                        if re.is_match(&text) {
                            found = true;
                            break;
                        }
                    }
                }

                if !found {
                    continue;
                }

                found_ids.insert(session_id.clone());

                // Build summary from metadata or JSONL
                let meta_file = self.session_meta_dir.join(format!("{session_id}.json"));
                if let Ok(meta_content) = fs::read_to_string(&meta_file) {
                    if let Ok(data) = serde_json::from_str::<Value>(&meta_content) {
                        let slug = self.get_slug_for_session(&session_id, &project_path);
                        let last_msg = self
                            .get_last_message_from_jsonl(&session_id, &project_path)
                            .map(|m| truncate(&m, 100));
                        let cache_start_time = str_field(&data, "start_time");
                        let last_activity = self
                            .get_last_timestamp_from_jsonl(&session_id, &project_path)
                            .unwrap_or_else(|| cache_start_time.clone());
                        let file_size = fs::metadata(&jsonl_path).ok().map(|m| m.len());
                        let git_branch = self.get_git_branch(&session_id, &project_path);

                        matching.push(SessionSummary {
                            session_id: session_id.clone(),
                            slug,
                            project_path: project_path.clone(),
                            project_name: get_project_name(&project_path),
                            start_time: cache_start_time,
                            duration_minutes: u64_field(&data, "duration_minutes"),
                            user_message_count: u64_field(&data, "user_message_count"),
                            assistant_message_count: u64_field(&data, "assistant_message_count"),
                            first_prompt: truncate(&str_field(&data, "first_prompt"), 100),
                            last_message: last_msg,
                            summary: data
                                .get("summary")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            file_size_bytes: file_size,
                            git_branch,
                            status: if let Some(&pid) = running_map.get(&session_id) {
                                self.status_for_running_session(&session_id, &project_path, pid)
                            } else {
                                SessionStatus::Stopped
                            },
                            last_activity,
                        });
                    }
                } else {
                    // No metadata — parse JSONL
                    let slug = self.get_slug_for_session(&session_id, &project_path);
                    let Ok(file) = fs::File::open(&jsonl_path) else {
                        continue;
                    };
                    let buf = BufReader::new(file);
                    let mut first_prompt = String::new();
                    let mut last_msg: Option<String> = None;
                    let mut user_count = 0u64;
                    let mut assistant_count = 0u64;
                    let mut start_time = String::new();
                    let mut last_activity = String::new();
                    let mut git_branch: Option<String> = None;

                    for line in buf.lines().map_while(Result::ok) {
                        if let Ok(data) = serde_json::from_str::<Value>(&line) {
                            if let Some(ts) = data.get("timestamp").and_then(|v| v.as_str())
                                && !ts.is_empty()
                            {
                                if start_time.is_empty() {
                                    start_time = ts.to_string();
                                }
                                last_activity = ts.to_string();
                            }
                            if git_branch.is_none()
                                && let Some(b) = data.get("gitBranch").and_then(|v| v.as_str())
                            {
                                git_branch = Some(b.to_string());
                            }
                            match data.get("type").and_then(|v| v.as_str()) {
                                Some("user") => {
                                    user_count += 1;
                                    if let Some(c) = data
                                        .get("message")
                                        .and_then(|m| m.get("content"))
                                        .and_then(|c| c.as_str())
                                    {
                                        let t = c.trim();
                                        if !t.is_empty() {
                                            if first_prompt.is_empty() {
                                                first_prompt = t.to_string();
                                            }
                                            last_msg = Some(t.to_string());
                                        }
                                    }
                                }
                                Some("assistant") => assistant_count += 1,
                                _ => {}
                            }
                        }
                    }

                    let file_size = fs::metadata(&jsonl_path).ok().map(|m| m.len());

                    matching.push(SessionSummary {
                        session_id: session_id.clone(),
                        slug,
                        project_path: project_path.clone(),
                        project_name: get_project_name(&project_path),
                        start_time: start_time.clone(),
                        duration_minutes: 0,
                        user_message_count: user_count,
                        assistant_message_count: assistant_count,
                        first_prompt: truncate(&first_prompt, 100),
                        last_message: last_msg.map(|m| truncate(&m, 100)),
                        summary: None,
                        file_size_bytes: file_size,
                        git_branch,
                        status: if let Some(&pid) = running_map.get(&session_id) {
                            self.status_for_running_session(&session_id, &project_path, pid)
                        } else {
                            SessionStatus::Stopped
                        },
                        last_activity: if last_activity.is_empty() {
                            start_time
                        } else {
                            last_activity
                        },
                    });
                }
            }
        }

        matching.sort_by(|a, b| {
            let a_key = if a.last_activity.is_empty() {
                &a.start_time
            } else {
                &a.last_activity
            };
            let b_key = if b.last_activity.is_empty() {
                &b.start_time
            } else {
                &b.last_activity
            };
            b_key.cmp(a_key)
        });

        if let Some(limit) = limit {
            matching.truncate(limit);
        }

        matching
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

fn str_field(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn u64_field(data: &Value, key: &str) -> u64 {
    data.get(key).and_then(|v| v.as_u64()).unwrap_or(0)
}

fn bool_field(data: &Value, key: &str) -> bool {
    data.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}
