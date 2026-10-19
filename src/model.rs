use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    Douyin,
    WeChatChannels,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Douyin => "douyin",
            Self::WeChatChannels => "wechat_channels",
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Idle,
    Resolving,
    Recording,
    Retrying,
    Completed,
    Failed,
    Stopped,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Resolving => "resolving",
            Self::Recording => "recording",
            Self::Retrying => "retrying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingTask {
    pub id: Uuid,
    pub platform: Platform,
    /// The public room URL entered by the user, retained for future resolution.
    pub room_url: Url,
    pub display_name: String,
    pub status: TaskStatus,
    pub output_dir: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ResolvedStream {
    pub stream_url: Url,
    pub title: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSession {
    pub id: Uuid,
    pub task_id: Uuid,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub output_path: Option<PathBuf>,
    pub error_message: Option<String>,
}
