use crate::models::{SessionDetail, SessionSummary};
use comfy_table::{ContentArrangement, Table};

pub fn format_list_table(sessions: &[SessionSummary]) -> String {
    if sessions.is_empty() {
        return "No sessions found.".to_string();
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.load_preset("                   ");
    table.set_header(vec![
        "ID", "Alias", "Project", "Branch", "Status", "Size", "Msgs", "Updated", "Last Msg",
    ]);

    for session in sessions {
        let id_short = if session.session_id.len() >= 8 {
            session.session_id[..8].to_string()
        } else {
            session.session_id.clone()
        };

        let alias = session.slug.as_deref().unwrap_or_default().to_string();

        let project = {
            let p = &session.project_path;
            if p.len() > 45 {
                format!("...{}", &p[p.len() - 42..])
            } else {
                p.to_string()
            }
        };

        let updated_src = if !session.last_activity.is_empty() {
            &session.last_activity
        } else {
            &session.start_time
        };
        let updated = if updated_src.len() >= 16 {
            // ISO format: 2024-01-15T10:30:00Z -> 2024-01-15 10:30
            updated_src[..16].replace('T', " ")
        } else {
            updated_src.clone()
        };

        let size_str = match session.file_size_bytes {
            Some(b) if b >= 1_048_576 => format!("{:.1}MB", b as f64 / 1_048_576.0),
            Some(b) if b >= 1024 => format!("{:.1}KB", b as f64 / 1024.0),
            Some(b) => format!("{b}B"),
            None => String::new(),
        };

        let branch = session
            .git_branch
            .as_deref()
            .map(|b| if b.len() > 12 { &b[..12] } else { b })
            .unwrap_or("");

        let message = session
            .last_message
            .as_deref()
            .unwrap_or(&session.first_prompt);
        let message = if message.len() > 40 {
            format!("{}...", &message[..37])
        } else {
            message.to_string()
        };

        let status = session.status.to_string();

        table.add_row(vec![
            &id_short,
            &alias,
            &project,
            branch,
            &status,
            &size_str,
            &session.total_messages().to_string(),
            &updated,
            &message,
        ]);
    }

    table.to_string()
}

pub fn format_list_json(sessions: &[SessionSummary]) -> String {
    serde_json::to_string_pretty(sessions).unwrap_or_else(|_| "[]".to_string())
}

pub fn format_detail_text(session: &SessionDetail) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "Session: {} ({})",
        session.id_alias(),
        session.session_id
    ));
    lines.push(String::new());

    lines.push(format!("Project: {}", session.project_path));
    if let Some(ref summary) = session.summary {
        lines.push(format!("Summary: {summary}"));
    }

    let started = if session.start_time.len() >= 19 {
        session.start_time[..19].replace('T', " ")
    } else {
        session.start_time.clone()
    };
    lines.push(format!("Started: {started}"));
    lines.push(format!("Duration: {} minutes", session.duration_minutes));
    lines.push(String::new());

    lines.push("Messages:".to_string());
    lines.push(format!("  User: {}", session.user_message_count));
    lines.push(format!("  Assistant: {}", session.assistant_message_count));
    lines.push(format!("  Total: {}", session.total_messages()));
    lines.push(String::new());

    if !session.tool_counts.is_empty() {
        lines.push("Tools Used:".to_string());
        let mut tools: Vec<_> = session.tool_counts.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1));
        for (tool, count) in tools {
            lines.push(format!("  {tool}: {count}"));
        }
        lines.push(String::new());
    }

    if !session.languages.is_empty() {
        lines.push("Languages:".to_string());
        let mut langs: Vec<_> = session.languages.iter().collect();
        langs.sort_by(|a, b| b.1.cmp(a.1));
        for (lang, count) in langs {
            lines.push(format!("  {lang}: {count}"));
        }
        lines.push(String::new());
    }

    if session.git_branch.is_some() || session.git_commits > 0 || session.git_pushes > 0 {
        lines.push("Git Activity:".to_string());
        if let Some(ref branch) = session.git_branch {
            lines.push(format!("  Branch: {branch}"));
        }
        lines.push(format!("  Commits: {}", session.git_commits));
        lines.push(format!("  Pushes: {}", session.git_pushes));
        lines.push(String::new());
    }

    if session.lines_added > 0 || session.lines_removed > 0 || session.files_modified > 0 {
        lines.push("Code Changes:".to_string());
        lines.push(format!("  Lines added: {}", session.lines_added));
        lines.push(format!("  Lines removed: {}", session.lines_removed));
        lines.push(format!("  Files modified: {}", session.files_modified));
        lines.push(String::new());
    }

    let mut features = Vec::new();
    if session.uses_task_agent {
        features.push("Task Agent");
    }
    if session.uses_mcp {
        features.push("MCP");
    }
    if session.uses_web_search {
        features.push("Web Search");
    }
    if session.uses_web_fetch {
        features.push("Web Fetch");
    }
    if !features.is_empty() {
        lines.push(format!("Features Used: {}", features.join(", ")));
        lines.push(String::new());
    }

    if session.tool_errors > 0 || session.user_interruptions > 0 {
        lines.push("Issues:".to_string());
        if session.tool_errors > 0 {
            lines.push(format!("  Tool errors: {}", session.tool_errors));
            for (cat, count) in &session.tool_error_categories {
                lines.push(format!("    {cat}: {count}"));
            }
        }
        if session.user_interruptions > 0 {
            lines.push(format!(
                "  User interruptions: {}",
                session.user_interruptions
            ));
        }
        lines.push(String::new());
    }

    if session.input_tokens > 0 || session.output_tokens > 0 {
        lines.push("Token Usage:".to_string());
        lines.push(format!("  Input: {}", format_number(session.input_tokens)));
        lines.push(format!(
            "  Output: {}",
            format_number(session.output_tokens)
        ));
        lines.push(format!(
            "  Total: {}",
            format_number(session.input_tokens + session.output_tokens)
        ));
        lines.push(String::new());
    }

    if session.underlying_goal.is_some() {
        lines.push("Session Analysis:".to_string());
        if let Some(ref goal) = session.underlying_goal {
            lines.push(format!("  Goal: {goal}"));
        }
        if let Some(ref outcome) = session.outcome {
            lines.push(format!("  Outcome: {outcome}"));
        }
        if let Some(ref helpfulness) = session.claude_helpfulness {
            lines.push(format!("  Claude Helpfulness: {helpfulness}"));
        }
        if let Some(ref session_type) = session.session_type {
            lines.push(format!("  Session Type: {session_type}"));
        }
        lines.push(String::new());
    }

    lines.push("First Prompt:".to_string());
    lines.push(format!("  {}", session.first_prompt));

    lines.join("\n")
}

pub fn format_detail_json(session: &SessionDetail) -> String {
    serde_json::to_string_pretty(session).unwrap_or_else(|_| "{}".to_string())
}

pub fn format_detail_yaml(session: &SessionDetail) -> String {
    // Simple YAML-like output without pulling in a yaml crate
    let json = serde_json::to_value(session).unwrap_or_default();
    json_to_yaml(&json, 0)
}

fn json_to_yaml(value: &serde_json::Value, indent: usize) -> String {
    let prefix = " ".repeat(indent);
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => {
            if s.contains('\n') || s.contains(':') || s.contains('#') {
                format!(
                    "|\n{}{}",
                    " ".repeat(indent + 2),
                    s.replace('\n', &format!("\n{}", " ".repeat(indent + 2)))
                )
            } else {
                s.clone()
            }
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                return "[]".to_string();
            }
            let mut lines = Vec::new();
            for item in arr {
                lines.push(format!("{prefix}- {}", json_to_yaml(item, indent + 2)));
            }
            lines.join("\n")
        }
        serde_json::Value::Object(obj) => {
            if obj.is_empty() {
                return "{}".to_string();
            }
            let mut lines = Vec::new();
            for (k, v) in obj {
                match v {
                    serde_json::Value::Object(_) | serde_json::Value::Array(_)
                        if !v.as_object().is_some_and(|o| o.is_empty())
                            && !v.as_array().is_some_and(|a| a.is_empty()) =>
                    {
                        lines.push(format!("{prefix}{k}:"));
                        lines.push(json_to_yaml(v, indent + 2));
                    }
                    _ => {
                        lines.push(format!("{prefix}{k}: {}", json_to_yaml(v, indent + 2)));
                    }
                }
            }
            lines.join("\n")
        }
    }
}

fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
