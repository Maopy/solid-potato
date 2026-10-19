use super::engine::Audit;
use super::fill::FillAudit;
use super::protocol::ProbeEvent;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use std::path::Path;

pub struct ProbeJournal {
    connection: Connection,
}

impl ProbeJournal {
    pub fn open(path: &Path) -> Result<Self> {
        let connection =
            Connection::open(path).context("opening separate moderation probe journal")?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA max_page_count=2147483646;
             PRAGMA wal_autocheckpoint=100;
             CREATE TABLE IF NOT EXISTS raw_messages (
                 run_id TEXT NOT NULL, connection_id TEXT NOT NULL, seq INTEGER NOT NULL,
                 capture_id TEXT NOT NULL, captured_at_ms INTEGER NOT NULL,
                 channel TEXT NOT NULL, category TEXT NOT NULL, encoding TEXT NOT NULL,
                 part INTEGER NOT NULL, parts INTEGER NOT NULL, data TEXT NOT NULL,
                 received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                 PRIMARY KEY(run_id, connection_id, seq)
             );
             CREATE INDEX IF NOT EXISTS raw_messages_capture ON raw_messages(run_id,connection_id,capture_id,part);
             CREATE TABLE IF NOT EXISTS probe_events (
                 run_id TEXT NOT NULL, connection_id TEXT NOT NULL,
                 seq INTEGER NOT NULL, event_json TEXT NOT NULL,
                 received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                 PRIMARY KEY(run_id, connection_id, seq)
             );
             CREATE TABLE IF NOT EXISTS preview_audits (
                 id INTEGER PRIMARY KEY, run_id TEXT NOT NULL, connection_id TEXT NOT NULL,
                 audit_json TEXT NOT NULL,
                 received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             );
             CREATE TABLE IF NOT EXISTS candidate_draft_audits (
                 id INTEGER PRIMARY KEY, run_id TEXT NOT NULL, connection_id TEXT NOT NULL,
                 audit_json TEXT NOT NULL,
                 received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             );
             CREATE TABLE IF NOT EXISTS delivery_audits (
                 run_id TEXT NOT NULL, connection_id TEXT NOT NULL, action_id TEXT NOT NULL,
                 audit_json TEXT NOT NULL,
                 received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                 PRIMARY KEY(run_id,connection_id,action_id)
             );
             CREATE TABLE IF NOT EXISTS fill_audits (
                 id INTEGER PRIMARY KEY, run_id TEXT NOT NULL, connection_id TEXT NOT NULL,
                 audit_json TEXT NOT NULL,
                 received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             );",
        )?;
        super::retention::initialize(&connection)?;
        Ok(Self { connection })
    }

    pub fn cleanup(
        &mut self,
        policy: &super::retention::RetentionPolicy,
        now_ms: i64,
        reclaim: bool,
    ) -> Result<super::retention::CleanupReport> {
        let report = super::retention::clean(&mut self.connection, policy, now_ms)?;
        if policy.enabled && reclaim {
            let free: i64 = self
                .connection
                .query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            let pages: i64 = self
                .connection
                .query_row("PRAGMA page_count", [], |r| r.get(0))?;
            let size: i64 = self
                .connection
                .query_row("PRAGMA page_size", [], |r| r.get(0))?;
            if free * size >= 16 * 1024 * 1024 && free * 4 >= pages {
                self.connection.execute_batch("VACUUM;")?;
            }
            self.connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        Ok(report)
    }

    pub fn append(&mut self, run: &str, connection: &str, events: &[ProbeEvent]) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for event in events {
            let seq = i64::try_from(event.seq).context("sequence exceeds SQLite range")?;
            let serialized = serde_json::to_string(event)?;
            let previous: Option<String> = transaction
                .query_row(
                    "SELECT event_json FROM probe_events WHERE run_id=?1 AND connection_id=?2 AND seq=?3",
                    params![run, connection, seq], |row| row.get(0),
                ).optional()?;
            if let Some(previous) = previous {
                if previous != serialized {
                    bail!("sequence reuse with different event");
                }
                continue;
            }
            transaction.execute(
                "INSERT INTO probe_events(run_id,connection_id,seq,event_json) VALUES(?1,?2,?3,?4)",
                params![run, connection, seq, serialized],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn append_raw(
        &mut self,
        run: &str,
        connection: &str,
        chunks: &[super::protocol::RawMessageChunk],
    ) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for chunk in chunks {
            chunk.validate()?;
            let previous: Option<(String,i64,String,String,String,u32,u32,String)> = transaction.query_row(
                "SELECT capture_id,captured_at_ms,channel,category,encoding,part,parts,data FROM raw_messages WHERE run_id=?1 AND connection_id=?2 AND seq=?3",
                params![run,connection,chunk.seq as i64], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))
            ).optional()?;
            if let Some(previous) = previous {
                if previous
                    != (
                        chunk.capture_id.clone(),
                        chunk.captured_at_ms as i64,
                        chunk.channel.clone(),
                        chunk.category.clone(),
                        chunk.encoding.clone(),
                        chunk.part,
                        chunk.parts,
                        chunk.data.clone(),
                    )
                {
                    bail!("raw sequence reuse with different content");
                }
                continue;
            }
            transaction.execute("INSERT INTO raw_messages(run_id,connection_id,seq,capture_id,captured_at_ms,channel,category,encoding,part,parts,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![run,connection,chunk.seq as i64,chunk.capture_id,chunk.captured_at_ms as i64,chunk.channel,chunk.category,chunk.encoding,chunk.part,chunk.parts,chunk.data])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn append_audits(&mut self, run: &str, connection: &str, logs: &[Audit]) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for log in logs {
            transaction.execute(
                "INSERT INTO preview_audits(run_id,connection_id,audit_json) VALUES(?1,?2,?3)",
                params![run, connection, serde_json::to_string(log)?],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn append_candidate_drafts(
        &mut self,
        run: &str,
        connection: &str,
        logs: &[serde_json::Value],
    ) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for log in logs {
            transaction.execute("INSERT INTO candidate_draft_audits(run_id,connection_id,audit_json) VALUES(?1,?2,?3)", params![run, connection, serde_json::to_string(log)?])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn append_delivery(
        &mut self,
        run: &str,
        connection: &str,
        report: &super::delivery::DeliveryReport,
    ) -> Result<()> {
        let encoded = serde_json::to_string(report)?;
        let previous:Option<String>=self.connection.query_row("SELECT audit_json FROM delivery_audits WHERE run_id=?1 AND connection_id=?2 AND action_id=?3",params![run,connection,report.action_id],|r|r.get(0)).optional()?;
        if let Some(previous) = previous {
            if previous != encoded {
                bail!("conflicting delivery evidence");
            }
            return Ok(());
        }
        self.connection.execute("INSERT INTO delivery_audits(run_id,connection_id,action_id,audit_json) VALUES(?1,?2,?3,?4)",params![run,connection,report.action_id,encoded])?;
        Ok(())
    }

    pub fn append_fill_audits(
        &mut self,
        run: &str,
        connection: &str,
        logs: &[FillAudit],
    ) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for log in logs {
            transaction.execute(
                "INSERT INTO fill_audits(run_id,connection_id,audit_json) VALUES(?1,?2,?3)",
                params![run, connection, serde_json::to_string(log)?],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

use rusqlite::OptionalExtension;
