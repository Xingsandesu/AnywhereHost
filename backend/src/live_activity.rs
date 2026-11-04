//! Durable per-turn projection and per-device APNs outbox, in the event journal DB.
use crate::{
    events::Events,
    notifications::{self, Delivery, Environment},
};
use anyhow::{Result, ensure};
use futures_util::{StreamExt, stream};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

const APPLE_EPOCH: f64 = 978_307_200.0;
const LIFETIME: i64 = 8 * 60 * 60;
const PHASE_INTERVAL: i64 = 5;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ContentState {
    phase: String,
    started_at: f64,
    ended_at: Option<f64>,
    updated_at: f64,
    completed_tools: usize,
    revision: i64,
}
impl ContentState {
    fn terminal(&self) -> bool {
        matches!(self.phase.as_str(), "completed" | "failed" | "stopped")
    }
    fn urgent(&self) -> bool {
        self.terminal() || self.phase == "approval"
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Turn {
    state: ContentState,
    work_phase: String,
    tools: HashSet<String>,
    // Wrapper request ID -> original native request ID. No command/message text.
    approvals: HashMap<String, String>,
}
impl Turn {
    fn new(started_at: f64, now: i64) -> Self {
        Self {
            state: ContentState {
                phase: "working".into(),
                started_at: started_at - APPLE_EPOCH,
                ended_at: None,
                updated_at: now as f64 - APPLE_EPOCH,
                completed_tools: 0,
                revision: 0,
            },
            work_phase: "working".into(),
            tools: HashSet::new(),
            approvals: HashMap::new(),
        }
    }
    fn finish(&mut self, status: &str, ended_at: f64) {
        self.state.phase = match status {
            "failed" => "failed",
            "interrupted" => "stopped",
            "completed" => "completed",
            _ => "disconnected",
        }
        .into();
        if !self.state.terminal() {
            self.work_phase = "disconnected".into();
            self.state.ended_at = None;
            return;
        }
        self.state.ended_at = Some(ended_at - APPLE_EPOCH);
        self.approvals.clear();
    }
    fn update_phase(&mut self) {
        if !self.state.terminal() {
            self.state.phase = if self.approvals.is_empty() {
                self.work_phase.clone()
            } else {
                "approval".into()
            };
        }
        self.state.completed_tools = self.tools.len();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registration {
    pub activity_id: String,
    pub delivery_token: String,
    pub environment: Environment,
    pub thread_id: String,
    pub turn_id: String,
    pub started_at: f64, // Explicit Unix seconds; NOT Swift's Codable Date epoch.
    pub token_version: i64,
}
impl Registration {
    pub fn validate_context(&self) -> Result<()> {
        ensure!(
            !self.activity_id.is_empty()
                && self.activity_id.len() <= 128
                && self
                    .activity_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
            "Invalid activity ID"
        );
        ensure!(
            [&self.thread_id, &self.turn_id]
                .iter()
                .all(|s| !s.is_empty() && s.len() <= 256),
            "Invalid native task identity"
        );
        ensure!(
            self.started_at.is_finite()
                && self.started_at >= 0.0
                && self.started_at <= notifications::now() as f64 + 60.0,
            "Invalid task start time"
        );
        ensure!(
            self.token_version > 0 && self.token_version <= 9_007_199_254_740_991,
            "Invalid token version"
        );
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_context()?;
        ensure!(
            (100..=4096).contains(&self.delivery_token.len())
                && self.delivery_token.len().is_multiple_of(2)
                && self.delivery_token.bytes().all(|c| c.is_ascii_hexdigit()),
            "Invalid activity token"
        );
        Ok(())
    }
    pub fn binding(&self, owner: &str) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(
            serde_json::to_vec(&json!([
                owner,
                self.activity_id,
                self.environment,
                self.thread_id,
                self.turn_id,
                self.started_at,
                self.token_version
            ]))
            .expect("validated registration"),
        ))
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Removal {
    pub activity_id: String,
    pub token_version: i64,
}

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS activity_turns (
        thread TEXT NOT NULL, turn TEXT NOT NULL, data TEXT NOT NULL, sequence INTEGER NOT NULL,
        version INTEGER NOT NULL DEFAULT 1, updated INTEGER NOT NULL, PRIMARY KEY(thread,turn));
        CREATE TABLE IF NOT EXISTS notification_subscriptions (
        id TEXT PRIMARY KEY, owner TEXT NOT NULL, token TEXT NOT NULL, environment TEXT NOT NULL,
        token_version INTEGER NOT NULL,
        thread TEXT NOT NULL, turn TEXT NOT NULL, expires INTEGER NOT NULL,
        delivered INTEGER NOT NULL DEFAULT -1, next_attempt INTEGER NOT NULL DEFAULT 0,
        attempts INTEGER NOT NULL DEFAULT 0, last_sent INTEGER NOT NULL DEFAULT 0,
        last_timestamp INTEGER NOT NULL DEFAULT 0, last_error TEXT, delivered_phase TEXT);
        CREATE TABLE IF NOT EXISTS notification_retired (id TEXT NOT NULL, owner TEXT NOT NULL, expires INTEGER NOT NULL, PRIMARY KEY(id,owner));
        CREATE TABLE IF NOT EXISTS notification_revocations (token TEXT PRIMARY KEY, expires INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_attempt INTEGER NOT NULL DEFAULT 0);
        CREATE TRIGGER IF NOT EXISTS notification_revoke_on_delete AFTER DELETE ON notification_subscriptions BEGIN INSERT OR IGNORE INTO notification_revocations(token,expires) VALUES(old.token,old.expires); END;
        CREATE TRIGGER IF NOT EXISTS notification_revoke_on_rotate AFTER UPDATE OF token ON notification_subscriptions WHEN old.token != new.token BEGIN INSERT OR IGNORE INTO notification_revocations(token,expires) VALUES(old.token,old.expires); END;
        CREATE INDEX IF NOT EXISTS notification_subscription_turn ON notification_subscriptions(thread,turn);")?;
    Ok(())
}
fn read_turn(db: &Connection, thread: &str, turn: &str) -> Result<Option<(Turn, i64)>> {
    let value: Option<(String, i64)> = db
        .query_row(
            "SELECT data,sequence FROM activity_turns WHERE thread=?1 AND turn=?2",
            params![thread, turn],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    value
        .map(|(data, sequence)| Ok((serde_json::from_str(&data)?, sequence)))
        .transpose()
}
fn save_turn(
    db: &Connection,
    thread: &str,
    turn: &str,
    value: &mut Turn,
    sequence: i64,
    now: i64,
) -> Result<()> {
    value.state.updated_at = now as f64 - APPLE_EPOCH;
    value.state.revision = sequence;
    db.execute("INSERT INTO activity_turns(thread,turn,data,sequence,updated) VALUES(?1,?2,?3,?4,?5)
        ON CONFLICT(thread,turn) DO UPDATE SET data=excluded.data,sequence=excluded.sequence,updated=excluded.updated,version=version+1",
        params![thread, turn, serde_json::to_string(value)?, sequence, now])?;
    // Phase changes should not sit behind the ordinary progress/heartbeat
    // throttle. Preserve APNs error backoff; coalesce rapid work-phase changes.
    db.execute(
        "UPDATE notification_subscriptions SET next_attempt=MIN(next_attempt,
        CASE WHEN ?4 OR delivered_phase='approval' THEN ?3 ELSE MAX(?3,last_sent+?6) END)
        WHERE thread=?1 AND turn=?2 AND attempts=0 AND (?4 OR delivered_phase IS NOT ?5)",
        params![
            thread,
            turn,
            now,
            value.state.urgent(),
            value.state.phase,
            PHASE_INTERVAL
        ],
    )?;
    Ok(())
}
fn native_time(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= notifications::now() as f64 + 60.0)
}

/// Called in the SAME SQLite transaction as journal insertion, before broadcast.
pub fn record_event(db: &Connection, event: &Value, sequence: i64, now: i64) -> Result<()> {
    if sequence % 256 == 0 {
        db.execute(
            "DELETE FROM notification_subscriptions WHERE expires<=?1",
            [now],
        )?;
        db.execute("DELETE FROM notification_retired WHERE expires<=?1", [now])?;
        db.execute("DELETE FROM activity_turns WHERE updated<?1 AND NOT EXISTS(SELECT 1 FROM notification_subscriptions s WHERE s.thread=activity_turns.thread AND s.turn=activity_turns.turn)", [now-LIFETIME])?;
    }
    let method = event["method"].as_str().unwrap_or("");
    if matches!(
        method,
        "host/request/resolved" | "serverRequest/resolved" | "host/runtime/disconnected"
    ) {
        let rows: Vec<(String, String, String)> = db
            .prepare("SELECT thread,turn,data FROM activity_turns WHERE updated>?1")?
            .query_map([now - LIFETIME], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (thread, turn, data) in rows {
            let mut value: Turn = serde_json::from_str(&data)?;
            if value.state.terminal() {
                continue;
            }
            let old = value.state.clone();
            let old_approvals = value.approvals.clone();
            if method == "host/runtime/disconnected" {
                value.state.phase = "disconnected".into();
            } else {
                let id = &event["params"]["requestId"];
                if method == "host/request/resolved" {
                    value
                        .approvals
                        .retain(|wrapper, _| Some(wrapper.as_str()) != id.as_str());
                } else {
                    // Stored native IDs use JSON encoding, preserving numeric
                    // versus string IDs (2 and "2" are distinct requests).
                    let encoded_id = id.to_string();
                    value
                        .approvals
                        .retain(|_, native| native.as_str() != encoded_id.as_str());
                }
                value.update_phase();
            }
            if old != value.state || old_approvals != value.approvals {
                save_turn(db, &thread, &turn, &mut value, sequence, now)?;
            }
        }
        return Ok(());
    }
    let pending = method == "host/request/pending";
    let p = if pending {
        &event["params"]["request"]["params"]
    } else {
        &event["params"]
    };
    let Some(thread) = p["threadId"].as_str() else {
        return Ok(());
    };
    let turn = p["turnId"]
        .as_str()
        .or_else(|| p["turn"]["id"].as_str())
        .map(str::to_owned);
    let turn = if turn.is_none() && pending {
        db.query_row(
            "SELECT turn FROM activity_turns WHERE thread=?1 ORDER BY sequence DESC LIMIT 1",
            [thread],
            |r| r.get(0),
        )
        .optional()?
    } else {
        turn
    };
    let Some(turn) = turn else { return Ok(()) };
    if !pending
        && !matches!(
            method,
            "turn/started"
                | "turn/completed"
                | "item/started"
                | "item/completed"
                | "item/agentMessage/delta"
                | "item/reasoning/summaryTextDelta"
                | "item/reasoning/textDelta"
                | "item/commandExecution/outputDelta"
        )
    {
        return Ok(());
    }
    let existing = read_turn(db, thread, &turn)?;
    if existing.as_ref().is_some_and(|(v, _)| v.state.terminal()) {
        return Ok(());
    }
    let mut value = existing
        .as_ref()
        .map(|(v, _)| v.clone())
        .unwrap_or_else(|| {
            Turn::new(
                native_time(&p["turn"]["startedAt"]).unwrap_or(now as f64),
                now,
            )
        });
    let previous = value.state.clone();
    let previous_work = value.work_phase.clone();
    if pending {
        if let Some(id) = event["params"]["requestId"].as_str() {
            value
                .approvals
                .insert(id.into(), event["params"]["request"]["id"].to_string());
        }
    } else if method == "turn/completed" {
        value.finish(
            p["turn"]["status"].as_str().unwrap_or(""),
            native_time(&p["turn"]["completedAt"]).unwrap_or(now as f64),
        );
    } else {
        let kind = p["item"]["type"].as_str().unwrap_or("");
        value.work_phase = if kind == "agentMessage" || method == "item/agentMessage/delta" {
            "responding"
        } else if kind == "reasoning"
            || matches!(
                method,
                "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta"
            )
        {
            "thinking"
        } else {
            "working"
        }
        .into();
        if method == "item/completed"
            && kind == "commandExecution"
            && p["item"]["status"] == "completed"
            && let Some(id) = p["item"]["id"].as_str()
        {
            value.tools.insert(id.into());
        }
    }
    value.update_phase();
    // Do not write or send for every token of an unchanged message phase.
    if previous != value.state || previous_work != value.work_phase || pending || existing.is_none()
    {
        save_turn(db, thread, &turn, &mut value, sequence, now)?;
    }
    Ok(())
}

impl Events {
    pub fn has_activity(&self, thread: &str, turn: &str) -> bool {
        read_turn(&self.db.lock().unwrap(), thread, turn)
            .ok()
            .flatten()
            .is_some()
    }
    pub fn observe_activity_snapshot(
        &self,
        thread: &str,
        turns: &Value,
        requests: &Value,
        checkpoint: i64,
    ) -> Result<()> {
        let db = self.db.lock().unwrap();
        let now = notifications::now();
        for turn in turns["data"].as_array().into_iter().flatten() {
            let Some(id) = turn["id"].as_str() else {
                continue;
            };
            let previous = read_turn(&db, thread, id)?;
            if previous
                .as_ref()
                .is_some_and(|(v, seq)| *seq > checkpoint || v.state.terminal())
            {
                continue;
            }
            let before = previous.as_ref().map(|(v, _)| v.clone());
            let mut value = previous.map(|(v, _)| v).unwrap_or_else(|| {
                Turn::new(native_time(&turn["startedAt"]).unwrap_or(now as f64), now)
            });
            if let Some(start) = native_time(&turn["startedAt"]) {
                value.state.started_at = start - APPLE_EPOCH;
            }
            // A verified running snapshot can recover even before its first item.
            // Unknown native statuses must remain unverified, never look active.
            value.work_phase = if turn["status"] == "inProgress" {
                "working"
            } else {
                "disconnected"
            }
            .into();
            for item in turn["items"].as_array().into_iter().flatten() {
                if turn["status"] == "inProgress" {
                    value.work_phase = match item["type"].as_str() {
                        Some("agentMessage") => "responding",
                        Some("reasoning") => "thinking",
                        _ => "working",
                    }
                    .into();
                }
                if item["type"] == "commandExecution"
                    && item["status"] == "completed"
                    && let Some(id) = item["id"].as_str()
                {
                    value.tools.insert(id.into());
                }
            }
            value.approvals.clear();
            for request in requests.as_array().into_iter().flatten() {
                let p = &request["request"]["params"];
                if p["threadId"] == thread
                    && (p["turnId"].is_null() || p["turnId"] == id)
                    && let Some(request_id) = request["requestId"].as_str()
                {
                    value
                        .approvals
                        .insert(request_id.into(), request["request"]["id"].to_string());
                }
            }
            if matches!(
                turn["status"].as_str(),
                Some("completed" | "failed" | "interrupted")
            ) {
                value.finish(
                    turn["status"].as_str().unwrap(),
                    native_time(&turn["completedAt"]).unwrap_or(now as f64),
                );
            }
            value.update_phase();
            if before.as_ref() == Some(&value) {
                // Advance the observation checkpoint even when content is unchanged,
                // so an older in-flight snapshot cannot overwrite this observation.
                // The outbox already sends unchanged running activities every minute.
                value.state.updated_at = now as f64 - APPLE_EPOCH;
                db.execute(
                    "UPDATE activity_turns SET data=?3,updated=?4,sequence=?5 WHERE thread=?1 AND turn=?2",
                    params![thread, id, serde_json::to_string(&value)?, now, checkpoint],
                )?;
            } else {
                save_turn(&db, thread, id, &mut value, checkpoint, now)?;
            }
        }
        Ok(())
    }
    pub fn register_activity(&self, registration: &Registration, owner: &str) -> Result<()> {
        registration.validate()?;
        let db = self.db.lock().unwrap();
        let retired: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM notification_retired WHERE id=?1 AND expires>?2 AND owner=?3)",
            params![registration.activity_id, notifications::now(), owner],
            |r| r.get(0),
        )?;
        ensure!(!retired, "Activity has already ended");
        ensure!(
            read_turn(&db, &registration.thread_id, &registration.turn_id)?.is_some(),
            "Task not found in Host history"
        );
        let existing: Option<(String, String, String)> = db
            .query_row(
                "SELECT thread,turn,owner FROM notification_subscriptions WHERE id=?1",
                [&registration.activity_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((thread, turn, current_owner)) = &existing {
            ensure!(
                current_owner == owner,
                "Activity belongs to another enrollment"
            );
            ensure!(
                thread == &registration.thread_id && turn == &registration.turn_id,
                "Activity identity cannot change"
            );
        }
        let count: i64 =
            db.query_row("SELECT count(*) FROM notification_subscriptions", [], |r| {
                r.get(0)
            })?;
        ensure!(
            existing.is_some() || count < 256,
            "Too many Live Activity subscriptions"
        );
        db.execute("INSERT INTO notification_subscriptions(id,token,environment,thread,turn,expires,token_version,owner) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
            ON CONFLICT(id) DO UPDATE SET token=excluded.token,environment=excluded.environment,
            delivered=CASE WHEN token!=excluded.token OR environment!=excluded.environment THEN -1 ELSE delivered END,
            next_attempt=0,attempts=0,last_error=NULL,token_version=excluded.token_version
            WHERE excluded.token_version>token_version OR (excluded.token_version=token_version AND (token!=excluded.token OR environment!=excluded.environment))",
            params![registration.activity_id, registration.delivery_token.to_lowercase(), registration.environment.name(), registration.thread_id, registration.turn_id, notifications::now()+LIFETIME, registration.token_version, owner])?;
        Ok(())
    }
    pub fn remove_activity(&self, removal: &Removal, owner: &str) -> Result<()> {
        ensure!(
            !removal.activity_id.is_empty()
                && removal.activity_id.len() <= 128
                && removal.token_version > 0
                && removal.token_version <= 9_007_199_254_740_991,
            "Invalid activity removal"
        );
        let db = self.db.lock().unwrap();
        let tx = db.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM notification_subscriptions WHERE id=?1 AND token_version<=?2 AND owner=?3",
            params![removal.activity_id, removal.token_version, owner],
        )?;
        tx.execute(
            "DELETE FROM notification_retired WHERE expires<=?1",
            [notifications::now()],
        )?;
        let retired: i64 = tx.query_row("SELECT count(*) FROM notification_retired", [], |r| {
            r.get(0)
        })?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM notification_retired WHERE id=?1 AND owner=?2)",
            params![removal.activity_id, owner],
            |r| r.get(0),
        )?;
        ensure!(exists || retired < 4096, "Too many retired activities");
        tx.execute("INSERT OR REPLACE INTO notification_retired(id,owner,expires) SELECT ?1,?2,?3 WHERE NOT EXISTS(SELECT 1 FROM notification_subscriptions WHERE id=?1)", params![removal.activity_id,owner,notifications::now()+LIFETIME])?;
        tx.commit()?;
        Ok(())
    }
    pub fn activity_status(&self) -> Value {
        let db = self.db.lock().unwrap();
        let counts = db.query_row("SELECT count(*),COALESCE(MAX(last_sent),0),count(last_error) FROM notification_subscriptions", [], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?))).unwrap_or_default();
        let last_error: Option<String> = db.query_row("SELECT last_error FROM notification_subscriptions WHERE last_error IS NOT NULL ORDER BY next_attempt DESC LIMIT 1", [], |r|r.get(0)).optional().unwrap_or_default();
        json!({"subscriptions":counts.0,"lastAcceptedAt":counts.1,"failedSubscriptions":counts.2,"lastError":last_error})
    }
    fn activity_threads(&self) -> Result<Vec<String>> {
        Ok(self.db.lock().unwrap().prepare("SELECT DISTINCT s.thread FROM notification_subscriptions s JOIN activity_turns t ON s.thread=t.thread AND s.turn=t.turn WHERE s.expires>?1 AND json_extract(t.data,'$.state.phase') NOT IN ('completed','stopped','failed')")?
            .query_map([notifications::now()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }
    fn activity_jobs(&self) -> Result<Vec<Job>> {
        let db = self.db.lock().unwrap();
        let now = notifications::now();
        db.execute(
            "DELETE FROM notification_subscriptions WHERE expires<=?1",
            [now],
        )?;
        db.execute("DELETE FROM notification_retired WHERE expires<=?1", [now])?;
        db.execute("DELETE FROM activity_turns WHERE updated<?1 AND NOT EXISTS(SELECT 1 FROM notification_subscriptions s WHERE s.thread=activity_turns.thread AND s.turn=activity_turns.turn)", [now-LIFETIME])?;
        let rows = db.prepare("SELECT s.id,s.token,s.environment,t.data,t.version,s.attempts,s.last_timestamp,s.owner,s.token_version FROM notification_subscriptions s JOIN activity_turns t ON s.thread=t.thread AND s.turn=t.turn
            WHERE s.next_attempt<=?1 AND (s.delivered!=t.version OR (s.last_sent<=?2 AND json_extract(t.data,'$.state.phase') NOT IN ('completed','stopped','failed')))
            ORDER BY CASE WHEN json_extract(t.data,'$.state.phase') IN ('approval','completed','stopped','failed') THEN 0 ELSE 1 END,s.last_sent LIMIT 32")?
            .query_map(params![now,now-60], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,u32>(5)?,r.get::<_,i64>(6)?,r.get::<_,String>(7)?,r.get::<_,i64>(8)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(
                |(
                    id,
                    token,
                    environment,
                    data,
                    version,
                    attempts,
                    last_timestamp,
                    owner,
                    token_version,
                )| {
                    let turn: Turn = serde_json::from_str(&data)?;
                    let timestamp = now.max(last_timestamp + 1);
                    db.execute(
                        "UPDATE notification_subscriptions SET last_timestamp=?2 WHERE id=?1",
                        params![id, timestamp],
                    )?;
                    Ok(Job {
                        owner,
                        token_version,
                        id,
                        token,
                        environment: serde_json::from_value(json!(environment))?,
                        state: turn.state,
                        version,
                        attempts,
                        timestamp,
                    })
                },
            )
            .collect()
    }
    fn activity_job_current(&self, job: &Job) -> bool {
        self.db.lock().unwrap().query_row(
            "SELECT EXISTS(SELECT 1 FROM notification_subscriptions WHERE id=?1 AND owner=?2 AND token=?3 AND token_version=?4 AND expires>?5 AND environment=?6)",
            params![job.id,job.owner,job.token,job.token_version,notifications::now(),job.environment.name()], |r| r.get(0)).unwrap_or(false)
    }
    fn prune_activity_owners(&self, security: &crate::security::Security) -> Result<()> {
        let db = self.db.lock().unwrap();
        let owners = db
            .prepare("SELECT DISTINCT owner FROM notification_subscriptions")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for owner in owners {
            if !security.active_owner(&owner) {
                db.execute(
                    "DELETE FROM notification_subscriptions WHERE owner=?1",
                    [owner],
                )?;
            }
        }
        Ok(())
    }
    fn activity_revocations(&self) -> Result<Vec<String>> {
        let db = self.db.lock().unwrap();
        db.execute(
            "DELETE FROM notification_revocations WHERE expires<=?1",
            [notifications::now()],
        )?;
        Ok(db
            .prepare("SELECT token FROM notification_revocations WHERE next_attempt<=?1 ORDER BY next_attempt LIMIT 32")?
            .query_map([notifications::now()], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    fn finish_activity_revocation(&self, token: &str, result: Delivery) -> Result<()> {
        let db = self.db.lock().unwrap();
        match result {
            Delivery::Accepted | Delivery::InvalidToken => {
                db.execute(
                    "DELETE FROM notification_revocations WHERE token=?1",
                    [token],
                )?;
            }
            Delivery::Retry { delay, .. } => {
                db.execute("UPDATE notification_revocations SET next_attempt=?2+MAX(?3,MIN(300,5*(1 << MIN(attempts,6)))),attempts=attempts+1 WHERE token=?1",
                    params![token,notifications::now(),delay])?;
            }
            Delivery::Rejected(_) => {
                db.execute(
                    "UPDATE notification_revocations SET next_attempt=expires WHERE token=?1",
                    [token],
                )?;
            }
        }
        Ok(())
    }
    fn finish_activity_job(&self, job: &Job, result: Delivery) -> Result<()> {
        let db = self.db.lock().unwrap();
        let now = notifications::now();
        let current: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM notification_subscriptions WHERE id=?1 AND owner=?2 AND token=?3 AND token_version=?4 AND environment=?5)",
            params![job.id,job.owner,job.token,job.token_version,job.environment.name()], |r| r.get(0))?;
        if !current {
            return Ok(());
        }
        match result {
            Delivery::Accepted => {
                db.execute("UPDATE notification_subscriptions SET delivered=?3,last_sent=?4,delivered_phase=?5,
                next_attempt=CASE
                    WHEN EXISTS(SELECT 1 FROM activity_turns t WHERE t.thread=notification_subscriptions.thread AND t.turn=notification_subscriptions.turn AND t.version>?3 AND
                        (json_extract(t.data,'$.state.phase') IN ('approval','completed','stopped','failed') OR (?5='approval' AND json_extract(t.data,'$.state.phase')!=?5))) THEN ?4
                    WHEN EXISTS(SELECT 1 FROM activity_turns t WHERE t.thread=notification_subscriptions.thread AND t.turn=notification_subscriptions.turn AND t.version>?3 AND json_extract(t.data,'$.state.phase')!=?5) THEN ?4+?6
                    ELSE ?4+15 END,
                attempts=0,last_error=NULL WHERE id=?1 AND token=?2", params![job.id,job.token,job.version,now,job.state.phase,PHASE_INTERVAL])?;
            }
            Delivery::InvalidToken => {
                db.execute(
                    "DELETE FROM notification_subscriptions WHERE id=?1 AND token=?2",
                    params![job.id, job.token],
                )?;
            }
            Delivery::Retry { reason, delay } => {
                let backoff = delay.max((5_i64 * 2_i64.pow(job.attempts.min(6))).min(300));
                db.execute("UPDATE notification_subscriptions SET attempts=attempts+1,next_attempt=?3,last_error=?4 WHERE id=?1 AND token=?2", params![job.id,job.token,now+backoff,reason])?;
            }
            Delivery::Rejected(reason) => {
                db.execute("UPDATE notification_subscriptions SET next_attempt=?4,attempts=attempts+1,last_error=?3 WHERE id=?1 AND token=?2", params![job.id,job.token,reason,now+300])?;
            }
        }
        Ok(())
    }
}

struct Job {
    owner: String,
    token_version: i64,
    id: String,
    token: String,
    environment: Environment,
    state: ContentState,
    version: i64,
    attempts: u32,
    timestamp: i64,
}
impl Job {
    fn payload(&self) -> Value {
        json!({"aps":{"timestamp":self.timestamp,"event":if self.state.terminal() {"end"} else {"update"},"content-state":self.state}})
    }
}

pub fn start(state: Arc<crate::api::AppState>) {
    let Some(client) = state.notifications.clone() else {
        return;
    };
    let reconcile = state.clone();
    tokio::spawn(async move {
        let mut events = reconcile.events.sender.subscribe();
        loop {
            let codex = reconcile.codex.read().await.clone();
            let connected = codex
                .as_ref()
                .is_some_and(|c| c.ready.load(Ordering::SeqCst));
            if let Some(codex) = codex
                && connected
            {
                for thread in reconcile.events.activity_threads().unwrap_or_default() {
                    let checkpoint = reconcile.events.last_sequence();
                    if let Ok(turns) = codex.request("thread/turns/list", json!({"threadId":thread,"limit":20,"itemsView":"full","sortDirection":"desc"})).await {
                        let requests = codex.pending_requests().await;
                        if reconcile.events.observe_activity_snapshot(&thread,&turns,&requests,checkpoint).is_err() { eprintln!("APNs snapshot persistence failed"); }
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(if connected {60} else {2})) => {},
                _ = async {
                    loop {
                        match events.recv().await {
                            Ok(sequence) if reconcile.events.get(sequence).is_ok_and(|event| event.payload["method"] == "host/runtime/connected") => break,
                            Ok(_) => {},
                            Err(_) => break,
                        }
                    }
                } => {},
            }
        }
    });
    tokio::spawn(async move {
        loop {
            if state.events.prune_activity_owners(&state.security).is_err() {
                eprintln!("APNs owner reconciliation failed");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            if let Ok(tokens) = state.events.activity_revocations() {
                stream::iter(tokens)
                    .for_each_concurrent(4, |token| {
                        let client = &client;
                        let events = &state.events;
                        async move {
                            let result = client.retire(&token).await;
                            let _ = events.finish_activity_revocation(&token, result);
                        }
                    })
                    .await;
            }
            match state.events.activity_jobs() {
                Ok(jobs) => {
                    stream::iter(jobs)
                        .for_each_concurrent(4, |job| {
                            let client = &client;
                            let events = &state.events;
                            let security = &state.security;
                            async move {
                                if !security.active_owner(&job.owner)
                                    || !events.activity_job_current(&job)
                                {
                                    return;
                                }
                                let result = client.deliver(&job.token, job.payload()).await;
                                if events.finish_activity_job(&job, result).is_err() {
                                    eprintln!("APNs delivery persistence failed");
                                }
                            }
                        })
                        .await
                }
                Err(_) => eprintln!("APNs outbox read failed"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn revocation_failures_back_off_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        let events = Events::open(&path).unwrap();
        running(&events);
        let registration = registration("a", 1);
        events.register_activity(&registration, "admin").unwrap();
        events
            .remove_activity(
                &Removal {
                    activity_id: registration.activity_id.clone(),
                    token_version: 1,
                },
                "admin",
            )
            .unwrap();
        let token = &registration.delivery_token;
        assert_eq!(events.activity_revocations().unwrap(), vec![token.clone()]);
        events
            .finish_activity_revocation(
                token,
                Delivery::Retry {
                    reason: "Unavailable".into(),
                    delay: 60,
                },
            )
            .unwrap();
        assert!(events.activity_revocations().unwrap().is_empty());
        drop(events);
        let events = Events::open(&path).unwrap();
        assert!(events.activity_revocations().unwrap().is_empty());
        let db = events.db.lock().unwrap();
        let (attempts, next): (i64, i64) = db
            .query_row(
                "SELECT attempts,next_attempt FROM notification_revocations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(attempts, 1);
        assert!(next >= notifications::now() + 59);
        drop(db);
        events
            .finish_activity_revocation(token, Delivery::InvalidToken)
            .unwrap();
        assert_eq!(
            events
                .db
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM notification_revocations", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
    }
    #[test]
    fn unchanged_snapshots_and_registration_do_not_trigger_extra_deliveries() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        let snapshot = json!({"data":[{"id":"r","status":"inProgress","startedAt":notifications::now(),"items":[]}]});
        events
            .observe_activity_snapshot("t", &snapshot, &json!([]), 1)
            .unwrap();
        let registration = registration("a", 1);
        events.register_activity(&registration, "admin").unwrap();
        let first = events.activity_jobs().unwrap().remove(0);
        events
            .finish_activity_job(&first, Delivery::Accepted)
            .unwrap();
        events
            .observe_activity_snapshot("t", &snapshot, &json!([]), 2)
            .unwrap();
        events.register_activity(&registration, "admin").unwrap();
        assert!(events.activity_jobs().unwrap().is_empty());
        events
            .db
            .lock()
            .unwrap()
            .execute("UPDATE notification_subscriptions SET next_attempt=0", [])
            .unwrap();
        assert!(events.activity_jobs().unwrap().is_empty());
        events
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE notification_subscriptions SET last_sent=?1",
                [notifications::now() - 60],
            )
            .unwrap();
        let heartbeat = events.activity_jobs().unwrap().remove(0);
        assert_eq!(heartbeat.version, first.version);
    }
    #[test]
    fn unchanged_newer_snapshot_blocks_out_of_order_observations() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        let current = json!({"data":[{"id":"r","status":"inProgress","startedAt":1000,"items":[{"type":"reasoning"}]}]});
        let stale = json!({"data":[{"id":"r","status":"inProgress","startedAt":1000,"items":[]}]});
        events
            .observe_activity_snapshot("t", &current, &json!([]), 1)
            .unwrap();
        events
            .observe_activity_snapshot("t", &current, &json!([]), 10)
            .unwrap();
        events
            .observe_activity_snapshot("t", &stale, &json!([]), 5)
            .unwrap();
        let db = events.db.lock().unwrap();
        let (turn, sequence) = read_turn(&db, "t", "r").unwrap().unwrap();
        assert_eq!(turn.state.phase, "thinking");
        assert_eq!(sequence, 10);
        assert_eq!(turn.state.revision, 1);
        let version: i64 = db
            .query_row(
                "SELECT version FROM activity_turns WHERE thread='t' AND turn='r'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 1);
    }
    #[test]
    fn deletion_and_rotation_persist_cloud_revocations() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        let original = registration("a", 1);
        events.register_activity(&original, "admin").unwrap();
        events
            .register_activity(&registration("b", 2), "admin")
            .unwrap();
        assert_eq!(
            events.activity_revocations().unwrap(),
            vec![original.delivery_token.clone()]
        );
        events
            .finish_activity_revocation(&original.delivery_token, Delivery::Accepted)
            .unwrap();
        events
            .remove_activity(
                &Removal {
                    activity_id: original.activity_id,
                    token_version: 2,
                },
                "admin",
            )
            .unwrap();
        assert_eq!(
            events.activity_revocations().unwrap(),
            vec!["b".repeat(128)]
        );
    }
    #[test]
    fn activity_ownership_and_queued_delivery_follow_enrollment() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        let original = registration("a", 1);
        events.register_activity(&original, "owner-a").unwrap();
        let old = events.activity_jobs().unwrap().remove(0);
        assert!(events.activity_job_current(&old));
        assert!(
            events
                .register_activity(&registration("b", 2), "owner-b")
                .is_err()
        );
        events
            .remove_activity(
                &Removal {
                    activity_id: original.activity_id.clone(),
                    token_version: original.token_version,
                },
                "owner-b",
            )
            .unwrap();
        assert!(events.activity_job_current(&old));
        // Rotating away and back to identical token bytes cannot revive an old job.
        events
            .register_activity(&registration("b", 2), "owner-a")
            .unwrap();
        let mut next = registration("a", 1);
        next.token_version = 3;
        events.register_activity(&next, "owner-a").unwrap();
        assert!(!events.activity_job_current(&old));
        events
            .finish_activity_job(&old, Delivery::InvalidToken)
            .unwrap();
        assert_eq!(events.activity_jobs().unwrap().len(), 1);
        let dir = tempfile::tempdir().unwrap();
        let security = crate::security::Security::open(dir.path().join("devices.json")).unwrap();
        events.prune_activity_owners(&security).unwrap();
        assert!(events.activity_jobs().unwrap().is_empty());
    }
    fn event(events: &Events, method: &str, p: Value) {
        events.publish(json!({"method":method,"params":p})).unwrap();
    }
    fn registration(token: &str, version: i64) -> Registration {
        Registration {
            activity_id: "device-activity".into(),
            delivery_token: token.repeat(128),
            environment: Environment::Sandbox,
            thread_id: "t".into(),
            turn_id: "r".into(),
            started_at: notifications::now() as f64 - 10.0,
            token_version: version,
        }
    }
    fn running(events: &Events) {
        event(
            events,
            "turn/started",
            json!({"threadId":"t","turn":{"id":"r","startedAt":notifications::now()-10}}),
        );
    }
    #[test]
    fn background_events_generate_complete_decodable_payload_without_subscriber_socket() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        event(
            &events,
            "item/completed",
            json!({"threadId":"t","turnId":"r","item":{"id":"tool","type":"commandExecution","status":"completed"}}),
        );
        event(
            &events,
            "item/completed",
            json!({"threadId":"t","turnId":"r","item":{"id":"tool","type":"commandExecution","status":"completed"}}),
        );
        event(
            &events,
            "host/request/pending",
            json!({"requestId":"approval","request":{"id":42,"params":{"threadId":"t","turnId":"r"}}}),
        );
        let job = events.activity_jobs().unwrap().remove(0);
        assert_eq!(job.state.phase, "approval");
        assert_eq!(job.state.completed_tools, 1);
        assert!(job.state.ended_at.is_none());
        assert!(job.state.urgent());
        let payload = job.payload();
        assert_eq!(payload["aps"]["event"], "update");
        assert!(
            payload["aps"]["content-state"]["startedAt"]
                .as_f64()
                .unwrap()
                < notifications::now() as f64 - APPLE_EPOCH
        );
        assert!(payload["aps"]["timestamp"].as_i64().unwrap() > 1_600_000_000);
        events
            .finish_activity_job(&job, Delivery::Accepted)
            .unwrap();
        event(
            &events,
            "turn/completed",
            json!({"threadId":"t","turn":{"id":"r","status":"completed","completedAt":notifications::now()}}),
        );
        let end = events.activity_jobs().unwrap().remove(0);
        assert_eq!(end.payload()["aps"]["event"], "end");
        assert!(end.payload()["aps"]["stale-date"].is_null());
        assert!(end.state.ended_at.is_some());
        assert!(end.timestamp > job.timestamp);
        event(
            &events,
            "item/started",
            json!({"threadId":"t","turnId":"r","item":{"id":"late","type":"reasoning"}}),
        );
        assert_eq!(
            read_turn(&events.db.lock().unwrap(), "t", "r")
                .unwrap()
                .unwrap()
                .0
                .state
                .phase,
            "completed"
        );
    }
    #[test]
    fn rotation_and_late_delivery_cannot_delete_or_replace_new_token() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        let old = events.activity_jobs().unwrap().remove(0);
        events
            .register_activity(&registration("b", 2), "admin")
            .unwrap();
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        events
            .finish_activity_job(&old, Delivery::InvalidToken)
            .unwrap();
        let new = events.activity_jobs().unwrap().remove(0);
        assert_eq!(new.token, "b".repeat(128));
        events
            .remove_activity(
                &Removal {
                    activity_id: old.id,
                    token_version: old.token_version,
                },
                "admin",
            )
            .unwrap();
        assert_eq!(events.activity_jobs().unwrap().len(), 1);
        events
            .remove_activity(
                &Removal {
                    activity_id: new.id,
                    token_version: new.token_version,
                },
                "admin",
            )
            .unwrap();
        assert!(events.activity_jobs().unwrap().is_empty());
        assert!(
            events
                .register_activity(&registration("b", 2), "admin")
                .is_err()
        );
    }
    #[test]
    fn durable_outbox_survives_restart_and_marks_unverified_running_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.sqlite");
        let events = Events::open(&path).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        let start = events.activity_jobs().unwrap().remove(0).state.started_at;
        drop(events);
        let events = Events::open(&path).unwrap();
        let job = events.activity_jobs().unwrap().remove(0);
        assert_eq!(job.state.phase, "disconnected");
        assert_eq!(job.state.started_at, start);
        assert!(job.state.ended_at.is_none());
    }
    #[test]
    fn native_snapshot_cannot_overwrite_later_journaled_completion() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        let checkpoint = events.last_sequence();
        event(
            &events,
            "turn/completed",
            json!({"threadId":"t","turn":{"id":"r","status":"failed"}}),
        );
        events
            .observe_activity_snapshot(
                "t",
                &json!({"data":[{"id":"r","status":"inProgress","items":[]}]}),
                &json!([]),
                checkpoint,
            )
            .unwrap();
        assert_eq!(
            read_turn(&events.db.lock().unwrap(), "t", "r")
                .unwrap()
                .unwrap()
                .0
                .state
                .phase,
            "failed"
        );
    }
    #[test]
    fn empty_running_snapshot_recovers_after_unknown_state_without_resetting_clock() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        let start = read_turn(&events.db.lock().unwrap(), "t", "r")
            .unwrap()
            .unwrap()
            .0
            .state
            .started_at;
        event(
            &events,
            "turn/completed",
            json!({"threadId":"t","turn":{"id":"r","status":"unknown"}}),
        );
        events
            .observe_activity_snapshot(
                "t",
                &json!({"data":[{"id":"r","status":"inProgress","items":[]}]}),
                &json!([]),
                events.last_sequence(),
            )
            .unwrap();
        let turn = read_turn(&events.db.lock().unwrap(), "t", "r")
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(turn.state.phase, "working");
        assert_eq!(turn.state.started_at, start);
        assert!(turn.state.ended_at.is_none());
    }
    #[test]
    fn completion_of_other_turn_does_not_end_registered_activity() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        event(
            &events,
            "turn/completed",
            json!({"threadId":"t","turn":{"id":"another","status":"completed"}}),
        );
        assert_eq!(
            events.activity_jobs().unwrap().remove(0).state.phase,
            "working"
        );
    }
    #[test]
    fn approvals_resume_only_when_last_pending_request_resolves() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        for (id, native) in [("a", 1), ("b", 2)] {
            event(
                &events,
                "host/request/pending",
                json!({"requestId":id,"request":{"id":native,"params":{"threadId":"t","turnId":"r"}}}),
            );
        }
        event(&events, "host/request/resolved", json!({"requestId":"a"}));
        assert_eq!(
            read_turn(&events.db.lock().unwrap(), "t", "r")
                .unwrap()
                .unwrap()
                .0
                .state
                .phase,
            "approval"
        );
        event(&events, "serverRequest/resolved", json!({"requestId":2}));
        assert_eq!(
            read_turn(&events.db.lock().unwrap(), "t", "r")
                .unwrap()
                .unwrap()
                .0
                .state
                .phase,
            "working"
        );
    }
    #[test]
    fn native_approval_resolution_preserves_request_id_types() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        for (wrapper, native) in [
            ("numeric", json!(2)),
            ("text", json!("2")),
            ("quoted", json!("\"2\"")),
        ] {
            event(
                &events,
                "host/request/pending",
                json!({"requestId":wrapper,"request":{"id":native,"params":{"threadId":"t","turnId":"r"}}}),
            );
        }
        event(&events, "serverRequest/resolved", json!({"requestId":2}));
        let turn = read_turn(&events.db.lock().unwrap(), "t", "r")
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(turn.approvals.len(), 2);
        assert!(turn.approvals.contains_key("text") && turn.approvals.contains_key("quoted"));
        assert_eq!(turn.state.phase, "approval");
        event(&events, "serverRequest/resolved", json!({"requestId":"2"}));
        let turn = read_turn(&events.db.lock().unwrap(), "t", "r")
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(turn.approvals.len(), 1);
        assert!(turn.approvals.contains_key("quoted"));
        assert_eq!(turn.state.phase, "approval");
        event(
            &events,
            "serverRequest/resolved",
            json!({"requestId":"\"2\""}),
        );
        let turn = read_turn(&events.db.lock().unwrap(), "t", "r")
            .unwrap()
            .unwrap()
            .0;
        assert!(turn.approvals.is_empty());
        assert_eq!(turn.state.phase, "working");
    }
    #[test]
    fn old_observation_does_not_control_push_freshness() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        let mut job = events.activity_jobs().unwrap().remove(0);
        job.state.phase = "disconnected".into();
        job.state.updated_at -= 600.0;
        let payload = job.payload();
        assert_eq!(payload["aps"].as_object().unwrap().len(), 3);
        assert_eq!(payload["aps"]["timestamp"], job.timestamp);
        assert_eq!(
            payload["aps"]["content-state"]["updatedAt"],
            job.state.updated_at
        );
    }

    #[test]
    fn payload_rejection_backs_off_without_freezing_subscription() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        let job = events.activity_jobs().unwrap().remove(0);
        events
            .finish_activity_job(
                &job,
                Delivery::Retry {
                    reason: "Unavailable".into(),
                    delay: 30,
                },
            )
            .unwrap();
        assert!(events.activity_jobs().unwrap().is_empty());
        events
            .finish_activity_job(&job, Delivery::Rejected("BadPayload".into()))
            .unwrap();
        event(
            &events,
            "turn/completed",
            json!({"threadId":"t","turn":{"id":"r","status":"completed"}}),
        );
        assert!(events.activity_jobs().unwrap().is_empty());
        let (next, expires): (i64, i64) = events
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT next_attempt,expires FROM notification_subscriptions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(next < expires);
        assert!((notifications::now() + 299..=notifications::now() + 300).contains(&next));
        events
            .db
            .lock()
            .unwrap()
            .execute("UPDATE notification_subscriptions SET next_attempt=0", [])
            .unwrap();
        let recovered = events.activity_jobs().unwrap().remove(0);
        assert_eq!(recovered.state.phase, "completed");
        events
            .finish_activity_job(&recovered, Delivery::Accepted)
            .unwrap();
        assert!(events.activity_jobs().unwrap().is_empty());
    }
    #[test]
    fn phase_changes_bypass_progress_throttle_and_use_high_priority() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        let first = events.activity_jobs().unwrap().remove(0);
        events
            .finish_activity_job(&first, Delivery::Accepted)
            .unwrap();
        // A phase change after five seconds must not wait the remaining ten.
        events
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE notification_subscriptions SET last_sent=?1",
                [notifications::now() - PHASE_INTERVAL],
            )
            .unwrap();
        event(
            &events,
            "item/reasoning/textDelta",
            json!({"threadId":"t","turnId":"r","delta":"private content"}),
        );
        let thinking = events.activity_jobs().unwrap().remove(0);
        assert_eq!(thinking.state.phase, "thinking");
        assert!(!thinking.payload().to_string().contains("private content"));
        events
            .finish_activity_job(&thinking, Delivery::Accepted)
            .unwrap();
        // Rapid transitions are coalesced to avoid spending the push budget on every token.
        event(
            &events,
            "item/agentMessage/delta",
            json!({"threadId":"t","turnId":"r","delta":"answer"}),
        );
        assert!(events.activity_jobs().unwrap().is_empty());
        events
            .db
            .lock()
            .unwrap()
            .execute("UPDATE notification_subscriptions SET next_attempt=0", [])
            .unwrap();
        let responding = events.activity_jobs().unwrap().remove(0);
        assert_eq!(responding.state.phase, "responding");
        events
            .finish_activity_job(&responding, Delivery::Accepted)
            .unwrap();
        // A verified heartbeat in the same phase remains low priority.
        events
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE notification_subscriptions SET next_attempt=0,last_sent=?1",
                [notifications::now() - 60],
            )
            .unwrap();
    }
    #[test]
    fn approval_resolution_is_immediate_even_during_inflight_delivery() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        event(
            &events,
            "host/request/pending",
            json!({"requestId":"approval","request":{"id":42,"params":{"threadId":"t","turnId":"r"}}}),
        );
        let approval = events.activity_jobs().unwrap().remove(0);
        event(
            &events,
            "host/request/resolved",
            json!({"requestId":"approval"}),
        );
        events
            .finish_activity_job(&approval, Delivery::Accepted)
            .unwrap();
        let resumed = events.activity_jobs().unwrap().remove(0);
        assert_eq!(resumed.state.phase, "working");
        events
            .finish_activity_job(&resumed, Delivery::Accepted)
            .unwrap();
        event(
            &events,
            "host/request/pending",
            json!({"requestId":"again","request":{"id":43,"params":{"threadId":"t","turnId":"r"}}}),
        );
        let approval = events.activity_jobs().unwrap().remove(0);
        events
            .finish_activity_job(&approval, Delivery::Accepted)
            .unwrap();
        event(
            &events,
            "host/request/resolved",
            json!({"requestId":"again"}),
        );
        assert_eq!(
            events.activity_jobs().unwrap().remove(0).state.phase,
            "working"
        );
    }
    #[test]
    fn phase_changes_preserve_failed_delivery_backoff() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        let job = events.activity_jobs().unwrap().remove(0);
        events
            .finish_activity_job(
                &job,
                Delivery::Retry {
                    reason: "TooManyRequests".into(),
                    delay: 60,
                },
            )
            .unwrap();
        event(
            &events,
            "item/agentMessage/delta",
            json!({"threadId":"t","turnId":"r"}),
        );
        assert!(events.activity_jobs().unwrap().is_empty());
    }
    #[test]
    fn inflight_phase_change_is_scheduled_before_ordinary_progress() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        let job = events.activity_jobs().unwrap().remove(0);
        event(
            &events,
            "item/agentMessage/delta",
            json!({"threadId":"t","turnId":"r"}),
        );
        events
            .finish_activity_job(&job, Delivery::Accepted)
            .unwrap();
        let delay: i64 = events
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT next_attempt-last_sent FROM notification_subscriptions",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(delay, PHASE_INTERVAL);
    }
    #[test]
    fn same_phase_progress_and_approval_heartbeats_stay_low_priority() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events
            .register_activity(&registration("a", 1), "admin")
            .unwrap();
        let job = events.activity_jobs().unwrap().remove(0);
        events
            .finish_activity_job(&job, Delivery::Accepted)
            .unwrap();
        event(
            &events,
            "item/completed",
            json!({"threadId":"t","turnId":"r","item":{"id":"tool","type":"commandExecution","status":"completed"}}),
        );
        let delay: i64 = events
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT next_attempt-last_sent FROM notification_subscriptions",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(delay, 15);
        assert!(events.activity_jobs().unwrap().is_empty());
        events
            .db
            .lock()
            .unwrap()
            .execute("UPDATE notification_subscriptions SET next_attempt=0", [])
            .unwrap();
        event(
            &events,
            "host/request/pending",
            json!({"requestId":"approval","request":{"id":42,"params":{"threadId":"t","turnId":"r"}}}),
        );
        let approval = events.activity_jobs().unwrap().remove(0);
        events
            .finish_activity_job(&approval, Delivery::Accepted)
            .unwrap();
        let requests = json!([{"requestId":"approval","request":{"id":42,"params":{"threadId":"t","turnId":"r"}}}]);
        events
            .observe_activity_snapshot(
                "t",
                &json!({"data":[{"id":"r","status":"inProgress","items":[]}]}),
                &requests,
                events.last_sequence(),
            )
            .unwrap();
    }
}
