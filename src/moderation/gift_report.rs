//! Read-only, anonymized count sequences. Numeric relations are observations,
//! never verified quantity semantics or inputs to the moderation rule engine.
use super::{
    events::{valid_id, valid_user_id},
    protocol::ProbeEvent,
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use std::{collections::HashMap, path::Path};

#[derive(Debug, Serialize)]
pub struct GiftReport {
    pub quantity_mode: &'static str,
    pub live_eligible: bool,
    pub run_found: bool,
    pub available_rows: u64,
    pub scanned_rows: usize,
    pub input_truncated: bool,
    pub rejected_rows: usize,
    pub unique_messages: usize,
    pub duplicate_messages: usize,
    pub conflicting_messages: usize,
    pub revision_rows: usize,
    pub unmatched_revisions: usize,
    pub revision_samples_truncated: bool,
    pub revisions: Vec<RevisionSample>,
    pub group_count: usize,
    pub omitted_groups: usize,
    pub groups: Vec<CountGroup>,
}
#[derive(Debug, Serialize)]
pub struct RevisionSample {
    pub document: usize,
    pub source_message: usize,
    pub prior_message_seen: bool,
    pub recipient: Option<usize>,
    pub previous_recipient: Option<usize>,
    pub reward_product_count: u64,
    pub combo_product_count: Option<u64>,
    pub previous_reward_product_count: u64,
    pub previous_combo_product_count: Option<u64>,
    pub is_floatmsg: Option<u64>,
    pub float_type: Option<u64>,
    pub previous_is_floatmsg: Option<u64>,
    pub previous_float_type: Option<u64>,
    pub since_first_candidate_ms: Option<i64>,
}
#[derive(Debug, Serialize)]
pub struct CountGroup {
    pub group: usize,
    pub document: usize,
    pub recipient: Option<usize>,
    pub messages: usize,
    /// Adjacent candidate counts for a sender/product, not a verified combo.
    pub matching_count_rises: usize,
    pub other_count_rises: usize,
    pub non_increases: usize,
    pub zero_matching_previous_total: usize,
    pub zero_other: usize,
    pub samples_truncated: bool,
    pub samples: Vec<CountSample>,
}
#[derive(Debug, Serialize)]
pub struct CountSample {
    pub message: usize,
    pub source_message: usize,
    pub reward_product_count: u64,
    pub combo_product_count: Option<u64>,
    pub is_floatmsg: Option<u64>,
    pub float_type: Option<u64>,
    pub reward_count_change: Option<i64>,
    pub combo_count_change: Option<i64>,
    pub receipt_gap_ms: Option<i64>,
    pub count_pattern: &'static str,
    #[serde(skip)]
    receipt_ms: Option<i64>,
}

pub fn inspect(path: &Path, run_id: Option<&str>, limit: u32) -> Result<GiftReport> {
    if !(1..=10000).contains(&limit) {
        bail!("gift report limit must be between 1 and 10000");
    }
    // Never call ProbeJournal::open: that creates/migrates writable tables.
    let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening gift candidate journal read-only")?;
    database.execute_batch("BEGIN")?;
    let selected: Option<String> = match run_id {
        Some(id) => database
            .query_row(
                "SELECT run_id FROM probe_events WHERE run_id=?1 LIMIT 1",
                [id],
                |r| r.get(0),
            )
            .optional()?,
        None => database
            .query_row(
                "SELECT run_id FROM probe_events ORDER BY rowid DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?,
    };
    let mut report = GiftReport {
        quantity_mode: "unknown",
        live_eligible: false,
        run_found: selected.is_some(),
        available_rows: 0,
        scanned_rows: 0,
        input_truncated: false,
        rejected_rows: 0,
        unique_messages: 0,
        duplicate_messages: 0,
        conflicting_messages: 0,
        revision_rows: 0,
        unmatched_revisions: 0,
        revision_samples_truncated: false,
        revisions: vec![],
        group_count: 0,
        omitted_groups: 0,
        groups: vec![],
    };
    let Some(run) = selected else {
        return Ok(report);
    };
    let filter = "CASE WHEN json_valid(event_json) THEN json_extract(event_json,'$.event_type') END IN ('gift.candidate','gift.candidate_revision')";
    report.available_rows = u64::try_from(database.query_row(
        &format!("SELECT count(*) FROM probe_events WHERE run_id=?1 AND {filter}"),
        [&run],
        |r| r.get::<_, i64>(0),
    )?)?;
    let mut statement=database.prepare(&format!("SELECT connection_id,event_json,received_at FROM probe_events WHERE run_id=?1 AND {filter} ORDER BY rowid DESC LIMIT ?2"))?;
    let mut rows = statement
        .query_map(params![run, limit], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.reverse();
    report.scanned_rows = rows.len();
    report.input_truncated = report.available_rows > rows.len() as u64;
    let mut documents = HashMap::new();
    let mut recipients = HashMap::new();
    let mut message_aliases = HashMap::new();
    let mut first_receipts = HashMap::new();
    let mut groups: HashMap<(String, String, String, Option<String>), usize> = HashMap::new();
    let mut messages: HashMap<
        (String, String, String),
        (String, String, u64, Option<u64>, Option<String>),
    > = HashMap::new();
    for (document, text, received_at) in rows {
        let event = if text.len() <= 16384 {
            serde_json::from_str::<ProbeEvent>(&text).ok()
        } else {
            None
        };
        let Some(event) = event.filter(|e| e.validate().is_ok()) else {
            report.rejected_rows += 1;
            continue;
        };
        let p = &event.payload;
        let identity = event
            .user_id
            .as_deref()
            .zip(event.native_event_id.as_deref());
        let product = p["gift_id"].as_str().zip(p["gift_name"].as_str());
        let (Some((user, id)), Some((gift, name)), Some(count)) =
            (identity, product, p["reward_product_count"].as_u64())
        else {
            report.rejected_rows += 1;
            continue;
        };
        let combo = p.get("combo_product_count").and_then(|v| v.as_u64());
        let is_floatmsg = p.get("is_floatmsg").and_then(|v| v.as_u64());
        let float_type = p.get("float_type").and_then(|v| v.as_u64());
        let recipient = p.get("recipient_user_id").and_then(|v| v.as_str());
        let revision = event.event_type == "gift.candidate_revision";
        if event.source_kind != "network"
            || p["decoder"] != "wechat.app20009.reward.candidate.v1"
            || p["message_type"] != 20009
            || p["quantity_mode"] != "unknown"
            || p["confidence"] != "candidate"
            || p["live_eligible"] != false
            || p["room_verified"] != false
            || !valid_user_id(user)
            || !valid_id(id)
            || !valid_id(gift)
            || user.trim().is_empty()
            || id.trim().is_empty()
            || gift.trim().is_empty()
            || name.trim().is_empty()
            || name.chars().count() > 64
            || count > 1000000
            || (!revision && count == 0)
            || combo.is_some_and(|v| v > 1000000)
            || (p.get("combo_product_count").is_some() && combo.is_none())
            || p.get("recipient_user_id").is_some_and(|_| {
                recipient.is_none_or(|id| !valid_user_id(id) || id.trim().is_empty())
            })
            || ["is_floatmsg", "float_type"].iter().any(|key| {
                p.get(*key)
                    .is_some_and(|v| v.as_u64().is_none_or(|n| n > 65535))
            })
        {
            report.rejected_rows += 1;
            continue;
        }
        let key = (document.clone(), user.to_owned(), id.to_owned());
        let receipt_ms = chrono::DateTime::parse_from_rfc3339(&received_at)
            .ok()
            .map(|t| t.timestamp_millis());
        if revision {
            let old = p["previous_reward_product_count"].as_u64();
            let old_combo = p
                .get("previous_combo_product_count")
                .and_then(|v| v.as_u64());
            let old_target = p.get("previous_recipient_user_id").and_then(|v| v.as_str());
            if old.is_none_or(|n| !(1..=1000000).contains(&n))
                || p["reason"] != "same_message_changed"
                || p.get("previous_combo_product_count")
                    .is_some_and(|v| v.as_u64().is_none_or(|n| n > 1000000))
                || p.get("previous_recipient_user_id").is_some_and(|_| {
                    old_target.is_none_or(|id| !valid_user_id(id) || id.trim().is_empty())
                })
                || ["previous_is_floatmsg", "previous_float_type"]
                    .iter()
                    .any(|key| {
                        p.get(*key)
                            .is_some_and(|v| v.as_u64().is_none_or(|n| n > 65535))
                    })
            {
                report.rejected_rows += 1;
                continue;
            }
            let next = documents.len() + 1;
            let doc = *documents.entry(document.clone()).or_insert(next);
            let next = message_aliases.len() + 1;
            let message = *message_aliases.entry(key.clone()).or_insert(next);
            let mut target_alias = |target: Option<&str>| {
                target.map(|id| {
                    let next = recipients.len() + 1;
                    *recipients
                        .entry((document.clone(), id.to_owned()))
                        .or_insert(next)
                })
            };
            let target = target_alias(recipient);
            let previous_target = target_alias(old_target);
            let prior = messages.contains_key(&key);
            let since = first_receipts
                .get(&key)
                .copied()
                .flatten()
                .zip(receipt_ms)
                .and_then(|(old, new): (i64, i64)| new.checked_sub(old));
            report.revision_rows += 1;
            if !prior {
                report.unmatched_revisions += 1;
            }
            report.revisions.push(RevisionSample {
                document: doc,
                source_message: message,
                prior_message_seen: prior,
                recipient: target,
                previous_recipient: previous_target,
                reward_product_count: count,
                combo_product_count: combo,
                previous_reward_product_count: old.unwrap(),
                previous_combo_product_count: old_combo,
                is_floatmsg,
                float_type,
                previous_is_floatmsg: p.get("previous_is_floatmsg").and_then(|v| v.as_u64()),
                previous_float_type: p.get("previous_float_type").and_then(|v| v.as_u64()),
                since_first_candidate_ms: since,
            });
            if report.revisions.len() > 50 {
                report.revisions.remove(0);
                report.revision_samples_truncated = true;
            }
            continue;
        }
        let fingerprint = (
            gift.to_owned(),
            name.to_owned(),
            count,
            combo,
            recipient.map(str::to_owned),
        );
        if let Some(old) = messages.get(&key) {
            if old == &fingerprint {
                report.duplicate_messages += 1;
            } else {
                report.conflicting_messages += 1;
            }
            continue;
        }
        messages.insert(key, fingerprint);
        let key = (document.clone(), user.to_owned(), id.to_owned());
        first_receipts.insert(key.clone(), receipt_ms);
        let next = message_aliases.len() + 1;
        let source_message = *message_aliases.entry(key).or_insert(next);
        report.unique_messages += 1;
        let next_doc = documents.len() + 1;
        let doc = *documents.entry(document.clone()).or_insert(next_doc);
        let recipient_alias = recipient.map(|id| {
            let next = recipients.len() + 1;
            *recipients
                .entry((document.clone(), id.to_owned()))
                .or_insert(next)
        });
        let index = *groups
            .entry((
                document,
                user.into(),
                gift.into(),
                recipient.map(str::to_owned),
            ))
            .or_insert_with(|| {
                let index = report.groups.len();
                report.groups.push(CountGroup {
                    group: index + 1,
                    document: doc,
                    recipient: recipient_alias,
                    messages: 0,
                    matching_count_rises: 0,
                    other_count_rises: 0,
                    non_increases: 0,
                    zero_matching_previous_total: 0,
                    zero_other: 0,
                    samples_truncated: false,
                    samples: vec![],
                });
                index
            });
        let group = &mut report.groups[index];
        let previous = group.samples.last();
        let receipt_gap_ms = previous
            .and_then(|s| s.receipt_ms.zip(receipt_ms))
            .and_then(|(old, new)| new.checked_sub(old));
        // Numeric pattern only. Even an exact match may be another donation;
        // the group does not establish a stable combo or current room identity.
        let count_pattern = match combo {
            None => "combo_missing",
            Some(0) if previous.is_some_and(|s| s.combo_product_count == Some(count)) => {
                group.zero_matching_previous_total += 1;
                "zero_matching_previous_total"
            }
            Some(0) => {
                group.zero_other += 1;
                "zero_other"
            }
            Some(_) => "positive_combo",
        };
        let reward_change = previous.map(|s| count as i64 - s.reward_product_count as i64);
        let combo_change = previous
            .and_then(|s| s.combo_product_count.zip(combo))
            .map(|(old, new)| new as i64 - old as i64);
        if let Some(change) = combo_change {
            if change > 0 && change as u64 == count {
                group.matching_count_rises += 1;
            } else if change > 0 {
                group.other_count_rises += 1;
            } else {
                group.non_increases += 1;
            }
        }
        group.messages += 1;
        group.samples.push(CountSample {
            message: group.messages,
            source_message,
            reward_product_count: count,
            combo_product_count: combo,
            is_floatmsg,
            float_type,
            reward_count_change: reward_change,
            combo_count_change: combo_change,
            receipt_gap_ms,
            count_pattern,
            receipt_ms,
        });
        if group.samples.len() > 32 {
            group.samples.remove(0);
            group.samples_truncated = true;
        }
    }
    report.group_count = report.groups.len();
    report.omitted_groups = report.group_count.saturating_sub(50);
    report.groups.truncate(50);
    Ok(report)
}

/// Explicit local target ID listing for manual YAML selection. Never identifies
/// a target as the current host. Latest run only, bounded, read-only snapshot.
pub fn inspect_targets(path: &Path) -> Result<serde_json::Value> {
    let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    database.execute_batch("BEGIN")?;
    let run: Option<String> = database
        .query_row(
            "SELECT run_id FROM probe_events ORDER BY rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(run) = run else {
        return Ok(
            serde_json::json!({"room_verified":false,"targets":[],"scanned_rows":0,"input_truncated":false}),
        );
    };
    let total: i64 = database.query_row("SELECT count(*) FROM probe_events WHERE run_id=?1 AND CASE WHEN json_valid(event_json) THEN json_extract(event_json,'$.event_type') END='gift.candidate'", [&run], |r| r.get(0))?;
    let mut stmt = database.prepare("SELECT event_json FROM probe_events WHERE run_id=?1 AND CASE WHEN json_valid(event_json) THEN json_extract(event_json,'$.event_type') END='gift.candidate' ORDER BY rowid DESC LIMIT 10000")?;
    let mut targets = std::collections::BTreeMap::<String, usize>::new();
    let mut seen = std::collections::HashSet::new();
    let mut scanned = 0;
    for row in stmt.query_map([&run], |r| r.get::<_, String>(0))? {
        scanned += 1;
        let text = row?;
        if text.len() > 16384 {
            continue;
        }
        let Ok(event) = serde_json::from_str::<ProbeEvent>(&text) else {
            continue;
        };
        let p = &event.payload;
        if event.validate().is_err()
            || event.source_kind != "network"
            || p["decoder"] != "wechat.app20009.reward.candidate.v1"
            || p["message_type"] != 20009
        {
            continue;
        }
        if let Some(target) = p["recipient_user_id"]
            .as_str()
            .filter(|id| valid_user_id(id) && !id.trim().is_empty())
        {
            if seen.insert((event.user_id, event.native_event_id, target.to_owned())) {
                *targets.entry(target.to_owned()).or_default() += 1;
            }
        }
    }
    Ok(
        serde_json::json!({"room_verified":false,"scanned_rows":scanned,"input_truncated":total > scanned,
        "targets":targets.into_iter().map(|(id,n)|serde_json::json!({"recipient_user_id":id,"candidate_messages":n})).collect::<Vec<_>>()}),
    )
}
