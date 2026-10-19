//! Local journal retention; whole raw captures are removed together.
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetentionPolicy {
    pub enabled: bool,
    pub raw_days: u32,
    pub raw_max_mib: u32,
    pub audit_days: u32,
    pub preserved_runs: Vec<String>,
}
impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            raw_days: 7,
            raw_max_mib: 2048,
            audit_days: 30,
            preserved_runs: vec![],
        }
    }
}
impl RetentionPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.raw_days == 0
            || self.raw_days > 3650
            || self.audit_days == 0
            || self.audit_days > 3650
            || self.raw_max_mib == 0
            || self.raw_max_mib > 1_048_576
            || self.preserved_runs.len() > 500
            || self
                .preserved_runs
                .iter()
                .any(|r| !super::protocol::identifier(r, 128))
        {
            bail!("invalid storage retention policy");
        }
        Ok(())
    }
}
#[derive(Default, Debug, Serialize)]
pub struct CleanupReport {
    pub raw_captures_deleted: u64,
    pub audit_rows_deleted: u64,
    pub raw_estimated_bytes: u64,
    pub over_limit: bool,
}

pub fn initialize(c: &Connection) -> Result<()> {
    let existed: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='raw_capture_usage')",
        [],
        |r| r.get(0),
    )?;
    c.execute_batch("BEGIN IMMEDIATE;")?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS raw_capture_usage (
      run_id TEXT NOT NULL,connection_id TEXT NOT NULL,capture_id TEXT NOT NULL,
      estimated_bytes INTEGER NOT NULL,chunks INTEGER NOT NULL,parts INTEGER NOT NULL,received_at TEXT NOT NULL,
      PRIMARY KEY(run_id,connection_id,capture_id));
      CREATE INDEX IF NOT EXISTS raw_capture_age ON raw_capture_usage(received_at,run_id,connection_id,capture_id);
      CREATE TEMP TABLE IF NOT EXISTS preserved_runs(run_id TEXT PRIMARY KEY);
      CREATE TRIGGER IF NOT EXISTS raw_usage_insert AFTER INSERT ON raw_messages BEGIN
        INSERT INTO raw_capture_usage VALUES(NEW.run_id,NEW.connection_id,NEW.capture_id,length(NEW.data)+512,1,NEW.parts,NEW.received_at)
        ON CONFLICT(run_id,connection_id,capture_id) DO UPDATE SET estimated_bytes=estimated_bytes+length(NEW.data)+512,chunks=chunks+1,parts=max(parts,NEW.parts),received_at=max(received_at,NEW.received_at);
      END;
      CREATE TRIGGER IF NOT EXISTS raw_usage_delete AFTER DELETE ON raw_messages BEGIN
        UPDATE raw_capture_usage SET estimated_bytes=estimated_bytes-length(OLD.data)-512,chunks=chunks-1 WHERE run_id=OLD.run_id AND connection_id=OLD.connection_id AND capture_id=OLD.capture_id;
        DELETE FROM raw_capture_usage WHERE run_id=OLD.run_id AND connection_id=OLD.connection_id AND capture_id=OLD.capture_id AND chunks=0;
      END;")?;
    if !existed {
        c.execute_batch("INSERT INTO raw_capture_usage SELECT run_id,connection_id,capture_id,sum(length(data)+512),count(*),max(parts),max(received_at) FROM raw_messages GROUP BY run_id,connection_id,capture_id;")?;
    }
    for table in [
        "probe_events",
        "preview_audits",
        "candidate_draft_audits",
        "fill_audits",
        "delivery_audits",
    ] {
        c.execute_batch(&format!(
            "CREATE INDEX IF NOT EXISTS {table}_age ON {table}(received_at);"
        ))?;
    }
    c.execute_batch("COMMIT;")?;
    Ok(())
}
pub fn clean(c: &mut Connection, p: &RetentionPolicy, now_ms: i64) -> Result<CleanupReport> {
    p.validate()?;
    let mut report = CleanupReport::default();
    if !p.enabled {
        return Ok(report);
    }
    let tx = c.transaction()?;
    tx.execute("DELETE FROM preserved_runs", [])?;
    for run in &p.preserved_runs {
        tx.execute("INSERT OR IGNORE INTO preserved_runs VALUES(?1)", [run])?;
    }
    let cutoff = now_ms.saturating_sub(i64::from(p.raw_days) * 86_400_000);
    report.raw_captures_deleted=tx.query_row("SELECT count(*) FROM raw_capture_usage WHERE received_at<strftime('%Y-%m-%dT%H:%M:%fZ',?1/1000.0,'unixepoch') AND run_id NOT IN (SELECT run_id FROM preserved_runs)",[cutoff],|r|r.get::<_,i64>(0))? as u64;
    tx.execute("DELETE FROM raw_messages WHERE (run_id,connection_id,capture_id) IN (SELECT run_id,connection_id,capture_id FROM raw_capture_usage WHERE received_at<strftime('%Y-%m-%dT%H:%M:%fZ',?1/1000.0,'unixepoch') AND run_id NOT IN (SELECT run_id FROM preserved_runs))",[cutoff])?;
    let mut total: i64 = tx.query_row(
        "SELECT coalesce(sum(estimated_bytes),0) FROM raw_capture_usage",
        [],
        |r| r.get(0),
    )?;
    let limit = i64::from(p.raw_max_mib) * 1024 * 1024;
    while total > limit {
        let oldest:Option<(String,String,String,i64)>=tx.query_row("SELECT run_id,connection_id,capture_id,estimated_bytes FROM raw_capture_usage WHERE chunks=parts AND run_id NOT IN (SELECT run_id FROM preserved_runs) ORDER BY received_at,run_id,connection_id,capture_id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let Some((run, conn, id, size)) = oldest else {
            break;
        };
        tx.execute(
            "DELETE FROM raw_messages WHERE run_id=?1 AND connection_id=?2 AND capture_id=?3",
            params![run, conn, id],
        )?;
        total -= size;
        report.raw_captures_deleted += 1;
    }
    let cutoff = now_ms.saturating_sub(i64::from(p.audit_days) * 86_400_000);
    for table in [
        "probe_events",
        "preview_audits",
        "candidate_draft_audits",
        "fill_audits",
        "delivery_audits",
    ] {
        report.audit_rows_deleted+=tx.execute(&format!("DELETE FROM {table} WHERE received_at<strftime('%Y-%m-%dT%H:%M:%fZ',?1/1000.0,'unixepoch') AND run_id NOT IN (SELECT run_id FROM preserved_runs)"),[cutoff])? as u64;
    }
    report.raw_estimated_bytes = total as u64;
    report.over_limit = total > limit;
    tx.commit()?;
    Ok(report)
}
