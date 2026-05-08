use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Session process status: Running (actively working), Idle (alive but inactive), or Stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Running,
    Idle,
    #[default]
    Stopped,
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionStatus::Running => write!(f, "Running"),
            SessionStatus::Idle => write!(f, "Idle"),
            SessionStatus::Stopped => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub slug: Option<String>,
    pub project_path: String,
    pub project_name: String,
    pub start_time: String,
    pub duration_minutes: u64,
    pub user_message_count: u64,
    pub assistant_message_count: u64,
    pub first_prompt: String,
    pub last_message: Option<String>,
    pub summary: Option<String>,
    pub file_size_bytes: Option<u64>,
    pub git_branch: Option<String>,
    #[serde(default)]
    pub status: SessionStatus,
    #[serde(default)]
    pub last_activity: String,
}

impl SessionSummary {
    pub fn total_messages(&self) -> u64 {
        self.user_message_count + self.assistant_message_count
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetail {
    pub session_id: String,
    pub slug: Option<String>,
    pub project_path: String,
    pub project_name: String,
    pub start_time: String,
    pub duration_minutes: u64,
    pub user_message_count: u64,
    pub assistant_message_count: u64,
    pub first_prompt: String,
    pub summary: Option<String>,

    // Git activity
    pub git_branch: Option<String>,
    pub git_commits: u64,
    pub git_pushes: u64,

    // Tool usage
    pub tool_counts: HashMap<String, u64>,
    pub languages: HashMap<String, u64>,
    pub tool_errors: u64,
    pub tool_error_categories: HashMap<String, u64>,

    // Code changes
    pub lines_added: u64,
    pub lines_removed: u64,
    pub files_modified: u64,

    // Session metadata
    pub user_interruptions: u64,
    pub uses_task_agent: bool,
    pub uses_mcp: bool,
    pub uses_web_search: bool,
    pub uses_web_fetch: bool,

    // Token usage
    pub input_tokens: u64,
    pub output_tokens: u64,

    // Facets
    pub underlying_goal: Option<String>,
    pub outcome: Option<String>,
    pub claude_helpfulness: Option<String>,
    pub session_type: Option<String>,
}

impl SessionDetail {
    pub fn id_alias(&self) -> &str {
        if let Some(ref slug) = self.slug {
            slug
        } else if self.session_id.len() >= 8 {
            &self.session_id[..8]
        } else {
            &self.session_id
        }
    }

    pub fn total_messages(&self) -> u64 {
        self.user_message_count + self.assistant_message_count
    }
}
