use crate::model::{Platform, RecordingSession, RecordingTask, TaskStatus};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use uuid::Uuid;

pub struct TaskStore {
    connection: Connection,
    path: std::path::PathBuf,
}

impl TaskStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open(&path).context("opening task database")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS recording_tasks (
                id TEXT PRIMARY KEY, platform TEXT NOT NULL, room_url TEXT NOT NULL,
                display_name TEXT NOT NULL, status TEXT NOT NULL, output_dir TEXT NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS recording_sessions (
                id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES recording_tasks(id),
                title TEXT NOT NULL, started_at TEXT NOT NULL, finished_at TEXT,
                output_path TEXT, error_message TEXT
            );
            CREATE INDEX IF NOT EXISTS recording_sessions_task_started
              ON recording_sessions(task_id, started_at DESC);
            CREATE TABLE IF NOT EXISTS wechat_anchor_bindings (
                task_id TEXT PRIMARY KEY REFERENCES recording_tasks(id),
                username TEXT NOT NULL UNIQUE,
                nickname TEXT NOT NULL,
                bound_at TEXT NOT NULL
            );",
        )?;
        Ok(Self { connection, path })
    }

    pub fn reopen(&self) -> Result<Self> {
        Self::open(&self.path)
    }

    pub fn insert(&self, task: &RecordingTask) -> Result<()> {
        self.connection.execute(
            "INSERT INTO recording_tasks VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                task.id.to_string(),
                task.platform.as_str(),
                task.room_url.as_str(),
                task.display_name,
                task.status.as_str(),
                task.output_dir.to_string_lossy(),
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<RecordingTask>> {
        let mut statement = self.connection.prepare("SELECT id, platform, room_url, display_name, status, output_dir, created_at, updated_at FROM recording_tasks ORDER BY created_at DESC")?;
        statement
            .query_map([], row_to_task)?
            .collect::<Result<Vec<_>, _>>()
            .context("reading recording tasks")
    }

    pub fn update_status(&self, id: Uuid, status: TaskStatus) -> Result<()> {
        self.connection.execute(
            "UPDATE recording_tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), Utc::now().to_rfc3339(), id.to_string()],
        )?;
        Ok(())
    }

    /// Updates user-managed task fields without changing status or history.
    pub fn update_definition(&self, id: Uuid, display_name: &str, output_dir: &Path) -> Result<()> {
        self.connection.execute(
            "UPDATE recording_tasks SET display_name = ?1, output_dir = ?2, updated_at = ?3 WHERE id = ?4",
            params![display_name, output_dir.to_string_lossy(), Utc::now().to_rfc3339(), id.to_string()],
        )?;
        Ok(())
    }

    /// Removes a task and its local recording metadata.
    ///
    /// Media files are intentionally not removed: deleting a task from the UI must
    /// never make an already recorded stream unrecoverable.
    pub fn delete(&self, id: Uuid) -> Result<()> {
        let task_id = id.to_string();
        self.connection.execute(
            "DELETE FROM wechat_anchor_bindings WHERE task_id = ?1",
            [&task_id],
        )?;
        self.connection.execute(
            "DELETE FROM recording_sessions WHERE task_id = ?1",
            [&task_id],
        )?;
        self.connection
            .execute("DELETE FROM recording_tasks WHERE id = ?1", [&task_id])?;
        Ok(())
    }

    pub fn get(&self, id: Uuid) -> Result<Option<RecordingTask>> {
        self.connection.query_row(
            "SELECT id, platform, room_url, display_name, status, output_dir, created_at, updated_at FROM recording_tasks WHERE id = ?1",
            [id.to_string()], row_to_task,
        ).optional().context("reading recording task")
    }

    pub fn begin_session(
        &self,
        task_id: Uuid,
        title: &str,
        started_at: DateTime<Utc>,
    ) -> Result<Uuid> {
        let id = Uuid::new_v4();
        self.connection.execute(
            "INSERT INTO recording_sessions (id, task_id, title, started_at) VALUES (?1, ?2, ?3, ?4)",
            params![id.to_string(), task_id.to_string(), title, started_at.to_rfc3339()],
        )?;
        Ok(id)
    }

    pub fn finish_session(
        &self,
        id: Uuid,
        output_path: Option<&Path>,
        error_message: Option<&str>,
    ) -> Result<()> {
        self.connection.execute(
            "UPDATE recording_sessions SET finished_at = ?1, output_path = ?2, error_message = ?3 WHERE id = ?4",
            params![Utc::now().to_rfc3339(), output_path.map(|path| path.to_string_lossy().to_string()), error_message, id.to_string()],
        )?;
        Ok(())
    }

    pub fn sessions_for_task(&self, task_id: Uuid) -> Result<Vec<RecordingSession>> {
        let mut statement = self.connection.prepare("SELECT id, task_id, title, started_at, finished_at, output_path, error_message FROM recording_sessions WHERE task_id = ?1 ORDER BY started_at DESC")?;
        statement
            .query_map([task_id.to_string()], row_to_session)?
            .collect::<Result<Vec<_>, _>>()
            .context("reading recording sessions")
    }

    pub fn bind_wechat_anchor(&self, task_id: Uuid, username: &str, nickname: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO wechat_anchor_bindings (task_id, username, nickname, bound_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(task_id) DO UPDATE SET username = excluded.username, nickname = excluded.nickname, bound_at = excluded.bound_at",
            params![task_id.to_string(), username, nickname, Utc::now().to_rfc3339()],
        ).context("saving WeChat Channels anchor binding")?;
        Ok(())
    }

    pub fn wechat_task_for_anchor(&self, username: &str) -> Result<Option<Uuid>> {
        self.connection
            .query_row(
                "SELECT task_id FROM wechat_anchor_bindings WHERE username = ?1",
                [username],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("reading WeChat Channels anchor binding")?
            .map(|value| value.parse().context("parsing bound task ID"))
            .transpose()
    }
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingTask> {
    fn parse<T: std::str::FromStr>(value: String, column: usize) -> rusqlite::Result<T>
    where
        T::Err: std::fmt::Display,
    {
        value.parse::<T>().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(error.to_string())),
            )
        })
    }
    let platform = match row.get::<_, String>(1)?.as_str() {
        "douyin" => Platform::Douyin,
        "wechat_channels" => Platform::WeChatChannels,
        _other => {
            return Err(rusqlite::Error::InvalidColumnType(
                1,
                "platform".to_owned(),
                rusqlite::types::Type::Text,
            ));
        }
    };
    let status = match row.get::<_, String>(4)?.as_str() {
        "idle" => TaskStatus::Idle,
        "resolving" => TaskStatus::Resolving,
        "recording" => TaskStatus::Recording,
        "retrying" => TaskStatus::Retrying,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "stopped" => TaskStatus::Stopped,
        _ => {
            return Err(rusqlite::Error::InvalidColumnType(
                4,
                "status".to_owned(),
                rusqlite::types::Type::Text,
            ));
        }
    };
    Ok(RecordingTask {
        id: parse(row.get(0)?, 0)?,
        platform,
        room_url: parse(row.get(2)?, 2)?,
        display_name: row.get(3)?,
        status,
        output_dir: row.get::<_, String>(5)?.into(),
        created_at: parse::<DateTime<Utc>>(row.get(6)?, 6)?,
        updated_at: parse::<DateTime<Utc>>(row.get(7)?, 7)?,
    })
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingSession> {
    fn parse<T: std::str::FromStr>(value: String, column: usize) -> rusqlite::Result<T>
    where
        T::Err: std::fmt::Display,
    {
        value.parse::<T>().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(error.to_string())),
            )
        })
    }
    Ok(RecordingSession {
        id: parse(row.get(0)?, 0)?,
        task_id: parse(row.get(1)?, 1)?,
        title: row.get(2)?,
        started_at: parse(row.get(3)?, 3)?,
        finished_at: row
            .get::<_, Option<String>>(4)?
            .map(|value| parse(value, 4))
            .transpose()?,
        output_path: row.get::<_, Option<String>>(5)?.map(Into::into),
        error_message: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use url::Url;

    #[test]
    fn deleting_task_removes_its_metadata() -> Result<()> {
        let directory = tempdir()?;
        let store = TaskStore::open(directory.path().join("tasks.sqlite3"))?;
        let now = Utc::now();
        let task = RecordingTask {
            id: Uuid::new_v4(),
            platform: Platform::Douyin,
            room_url: Url::parse("https://live.douyin.com/example")?,
            display_name: "example".into(),
            status: TaskStatus::Idle,
            output_dir: directory.path().join("recordings"),
            created_at: now,
            updated_at: now,
        };
        store.insert(&task)?;
        store.begin_session(task.id, "example", now)?;
        store.bind_wechat_anchor(task.id, "anchor", "主播")?;

        store.delete(task.id)?;

        assert!(store.get(task.id)?.is_none());
        assert!(store.sessions_for_task(task.id)?.is_empty());
        assert!(store.wechat_task_for_anchor("anchor")?.is_none());
        Ok(())
    }
}
