//! Durable per-turn projection and per-device APNs outbox, in the event journal DB.
use crate::{apns::{self, Delivery, Environment}, events::Events};
use anyhow::{Result, ensure};
use futures_util::{StreamExt, stream};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::{HashMap, HashSet}, sync::{Arc, atomic::Ordering}, time::Duration};

const APPLE_EPOCH: f64 = 978_307_200.0;
const LIFETIME: i64 = 8 * 60 * 60;

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
    fn terminal(&self) -> bool { matches!(self.phase.as_str(), "completed" | "failed" | "stopped") }
    fn urgent(&self) -> bool { self.terminal() || self.phase == "approval" }
}
#[derive(Clone, Serialize, Deserialize)]
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
        Self { state: ContentState { phase:"working".into(), started_at:started_at - APPLE_EPOCH,
            ended_at:None, updated_at:now as f64 - APPLE_EPOCH, completed_tools:0, revision:0 },
            work_phase:"working".into(), tools:HashSet::new(), approvals:HashMap::new() }
    }
    fn finish(&mut self, status: &str, ended_at: f64) {
        self.state.phase = match status { "failed" => "failed", "interrupted" => "stopped", "completed" => "completed", _ => "disconnected" }.into();
        if !self.state.terminal() { self.work_phase = "disconnected".into(); self.state.ended_at = None; return }
        self.state.ended_at = Some(ended_at - APPLE_EPOCH);
        self.approvals.clear();
    }
    fn update_phase(&mut self) {
        if !self.state.terminal() {
            self.state.phase = if self.approvals.is_empty() { self.work_phase.clone() } else { "approval".into() };
        }
        self.state.completed_tools = self.tools.len();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registration {
    pub activity_id: String,
    pub push_token: String,
    pub environment: Environment,
    pub thread_id: String,
    pub turn_id: String,
    pub started_at: f64, // Explicit Unix seconds; NOT Swift's Codable Date epoch.
    pub token_version: i64,
}
impl Registration {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.activity_id.is_empty() && self.activity_id.len() <= 128 && self.activity_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'), "Invalid activity ID");
        ensure!((64..=512).contains(&self.push_token.len()) && self.push_token.len().is_multiple_of(2) && self.push_token.bytes().all(|c| c.is_ascii_hexdigit()), "Invalid activity token");
        ensure!([&self.thread_id, &self.turn_id].iter().all(|s| !s.is_empty() && s.len() <= 256), "Invalid native task identity");
        ensure!(self.started_at.is_finite() && self.started_at >= 0.0 && self.started_at <= apns::now() as f64 + 60.0, "Invalid task start time");
        ensure!(self.token_version > 0 && self.token_version <= 9_007_199_254_740_991, "Invalid token version");
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Removal { pub activity_id: String, pub push_token: String }

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS activity_turns (
        thread TEXT NOT NULL, turn TEXT NOT NULL, data TEXT NOT NULL, sequence INTEGER NOT NULL,
        version INTEGER NOT NULL DEFAULT 1, updated INTEGER NOT NULL, PRIMARY KEY(thread,turn));
        CREATE TABLE IF NOT EXISTS activity_subscriptions (
        id TEXT PRIMARY KEY, token TEXT NOT NULL, environment TEXT NOT NULL,
        token_version INTEGER NOT NULL,
        thread TEXT NOT NULL, turn TEXT NOT NULL, expires INTEGER NOT NULL,
        delivered INTEGER NOT NULL DEFAULT -1, next_attempt INTEGER NOT NULL DEFAULT 0,
        attempts INTEGER NOT NULL DEFAULT 0, last_sent INTEGER NOT NULL DEFAULT 0,
        last_timestamp INTEGER NOT NULL DEFAULT 0, last_error TEXT);
        CREATE TABLE IF NOT EXISTS activity_retired (id TEXT PRIMARY KEY, expires INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS activity_subscription_turn ON activity_subscriptions(thread,turn);")?;
    Ok(())
}
fn read_turn(db: &Connection, thread: &str, turn: &str) -> Result<Option<(Turn, i64)>> {
    let value: Option<(String, i64)> = db.query_row("SELECT data,sequence FROM activity_turns WHERE thread=?1 AND turn=?2", params![thread, turn], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    value.map(|(data, sequence)| Ok((serde_json::from_str(&data)?, sequence))).transpose()
}
fn save_turn(db: &Connection, thread: &str, turn: &str, value: &mut Turn, sequence: i64, now: i64) -> Result<()> {
    value.state.updated_at = now as f64 - APPLE_EPOCH;
    value.state.revision = sequence;
    db.execute("INSERT INTO activity_turns(thread,turn,data,sequence,updated) VALUES(?1,?2,?3,?4,?5)
        ON CONFLICT(thread,turn) DO UPDATE SET data=excluded.data,sequence=excluded.sequence,updated=excluded.updated,version=version+1",
        params![thread, turn, serde_json::to_string(value)?, sequence, now])?;
    if value.state.urgent() {
        db.execute("UPDATE activity_subscriptions SET next_attempt=MIN(next_attempt,?3) WHERE thread=?1 AND turn=?2 AND attempts=0", params![thread, turn, now])?;
    }
    Ok(())
}
fn native_time(value: &Value) -> Option<f64> { value.as_f64().filter(|n| n.is_finite() && *n >= 0.0 && *n <= apns::now() as f64 + 60.0) }

/// Called in the SAME SQLite transaction as journal insertion, before broadcast.
pub fn record_event(db: &Connection, event: &Value, sequence: i64, now: i64) -> Result<()> {
    if sequence % 256 == 0 {
        db.execute("DELETE FROM activity_subscriptions WHERE expires<=?1", [now])?;
        db.execute("DELETE FROM activity_retired WHERE expires<=?1", [now])?;
        db.execute("DELETE FROM activity_turns WHERE updated<?1 AND NOT EXISTS(SELECT 1 FROM activity_subscriptions s WHERE s.thread=activity_turns.thread AND s.turn=activity_turns.turn)", [now-LIFETIME])?;
    }
    let method = event["method"].as_str().unwrap_or("");
    if matches!(method, "host/request/resolved" | "serverRequest/resolved" | "host/runtime/disconnected") {
        let rows: Vec<(String,String,String)> = db.prepare("SELECT thread,turn,data FROM activity_turns WHERE updated>?1")?
            .query_map([now-LIFETIME], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
        for (thread, turn, data) in rows {
            let mut value: Turn = serde_json::from_str(&data)?;
            if value.state.terminal() { continue }
            let old = value.state.clone();
            let old_approvals = value.approvals.clone();
            if method == "host/runtime/disconnected" { value.state.phase = "disconnected".into(); }
            else {
                let id = &event["params"]["requestId"];
                if method == "host/request/resolved" {
                    value.approvals.retain(|wrapper, _| Some(wrapper.as_str()) != id.as_str());
                } else {
                    // Stored native IDs use JSON encoding, preserving numeric
                    // versus string IDs (2 and "2" are distinct requests).
                    let encoded_id = id.to_string();
                    value.approvals.retain(|_, native| native.as_str() != encoded_id.as_str());
                }
                value.update_phase();
            }
            if old != value.state || old_approvals != value.approvals { save_turn(db, &thread, &turn, &mut value, sequence, now)?; }
        }
        return Ok(())
    }
    let pending = method == "host/request/pending";
    let p = if pending { &event["params"]["request"]["params"] } else { &event["params"] };
    let Some(thread) = p["threadId"].as_str() else { return Ok(()) };
    let turn = p["turnId"].as_str().or_else(|| p["turn"]["id"].as_str()).map(str::to_owned);
    let turn = if turn.is_none() && pending {
        db.query_row("SELECT turn FROM activity_turns WHERE thread=?1 ORDER BY sequence DESC LIMIT 1", [thread], |r| r.get(0)).optional()?
    } else { turn };
    let Some(turn) = turn else { return Ok(()) };
    if !pending && !matches!(method, "turn/started" | "turn/completed" | "item/started" | "item/completed" | "item/agentMessage/delta" | "item/reasoning/summaryTextDelta" | "item/commandExecution/outputDelta") { return Ok(()) }
    let existing = read_turn(db, thread, &turn)?;
    if existing.as_ref().is_some_and(|(v,_)| v.state.terminal()) { return Ok(()) }
    let mut value = existing.as_ref().map(|(v,_)| v.clone()).unwrap_or_else(|| Turn::new(native_time(&p["turn"]["startedAt"]).unwrap_or(now as f64), now));
    let previous = value.state.clone();
    let previous_work = value.work_phase.clone();
    if pending {
        if let Some(id) = event["params"]["requestId"].as_str() { value.approvals.insert(id.into(), event["params"]["request"]["id"].to_string()); }
    } else if method == "turn/completed" {
        value.finish(p["turn"]["status"].as_str().unwrap_or(""), native_time(&p["turn"]["completedAt"]).unwrap_or(now as f64));
    } else {
        let kind = p["item"]["type"].as_str().unwrap_or("");
        value.work_phase = if kind == "agentMessage" || method == "item/agentMessage/delta" { "responding" }
            else if kind == "reasoning" || method == "item/reasoning/summaryTextDelta" { "thinking" } else { "working" }.into();
        if method == "item/completed" && kind == "commandExecution" && p["item"]["status"] == "completed"
            && let Some(id) = p["item"]["id"].as_str() { value.tools.insert(id.into()); }
    }
    value.update_phase();
    // Do not write or send for every token of an unchanged message phase.
    if previous != value.state || previous_work != value.work_phase || pending || existing.is_none() { save_turn(db, thread, &turn, &mut value, sequence, now)?; }
    Ok(())
}

impl Events {
    pub fn has_activity(&self, thread: &str, turn: &str) -> bool {
        read_turn(&self.db.lock().unwrap(),thread,turn).ok().flatten().is_some()
    }
    pub fn observe_activity_snapshot(&self, thread: &str, turns: &Value, requests: &Value, checkpoint: i64) -> Result<()> {
        let db = self.db.lock().unwrap();
        let now = apns::now();
        for turn in turns["data"].as_array().into_iter().flatten() {
            let Some(id) = turn["id"].as_str() else { continue };
            let previous = read_turn(&db, thread, id)?;
            if previous.as_ref().is_some_and(|(v, seq)| *seq > checkpoint || v.state.terminal()) { continue }
            let mut value = previous.map(|(v,_)| v).unwrap_or_else(|| Turn::new(native_time(&turn["startedAt"]).unwrap_or(now as f64), now));
            if let Some(start) = native_time(&turn["startedAt"]) { value.state.started_at = start - APPLE_EPOCH; }
            // A verified running snapshot can recover even before its first item.
            // Unknown native statuses must remain unverified, never look active.
            value.work_phase = if turn["status"] == "inProgress" { "working" } else { "disconnected" }.into();
            for item in turn["items"].as_array().into_iter().flatten() {
                if turn["status"] == "inProgress" {
                    value.work_phase = match item["type"].as_str() { Some("agentMessage") => "responding", Some("reasoning") => "thinking", _ => "working" }.into();
                }
                if item["type"] == "commandExecution" && item["status"] == "completed" && let Some(id) = item["id"].as_str() { value.tools.insert(id.into()); }
            }
            value.approvals.clear();
            for request in requests.as_array().into_iter().flatten() {
                let p = &request["request"]["params"];
                if p["threadId"] == thread && (p["turnId"].is_null() || p["turnId"] == id)
                    && let Some(request_id) = request["requestId"].as_str() {
                    value.approvals.insert(request_id.into(), request["request"]["id"].to_string());
                }
            }
            if matches!(turn["status"].as_str(), Some("completed" | "failed" | "interrupted")) {
                value.finish(turn["status"].as_str().unwrap(), native_time(&turn["completedAt"]).unwrap_or(now as f64));
            }
            value.update_phase();
            save_turn(&db, thread, id, &mut value, checkpoint, now)?;
        }
        Ok(())
    }
    pub fn register_activity(&self, registration: &Registration) -> Result<()> {
        registration.validate()?;
        let db = self.db.lock().unwrap();
        let retired: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM activity_retired WHERE id=?1 AND expires>?2)", params![registration.activity_id,apns::now()], |r| r.get(0))?;
        ensure!(!retired, "Activity has already ended");
        ensure!(read_turn(&db, &registration.thread_id, &registration.turn_id)?.is_some(), "Task not found in Host history");
        if let Some((mut turn, sequence)) = read_turn(&db, &registration.thread_id, &registration.turn_id)? &&
            registration.started_at - APPLE_EPOCH < turn.state.started_at {
            turn.state.started_at = registration.started_at - APPLE_EPOCH;
            save_turn(&db,&registration.thread_id,&registration.turn_id,&mut turn,sequence,apns::now())?;
        }
        let existing: Option<(String,String)> = db.query_row("SELECT thread,turn FROM activity_subscriptions WHERE id=?1", [&registration.activity_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((thread, turn)) = &existing { ensure!(thread == &registration.thread_id && turn == &registration.turn_id, "Activity identity cannot change"); }
        let count: i64 = db.query_row("SELECT count(*) FROM activity_subscriptions", [], |r| r.get(0))?;
        ensure!(existing.is_some() || count < 256, "Too many Live Activity subscriptions");
        db.execute("INSERT INTO activity_subscriptions(id,token,environment,thread,turn,expires,token_version) VALUES(?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(id) DO UPDATE SET token=excluded.token,environment=excluded.environment,
            delivered=CASE WHEN token!=excluded.token OR environment!=excluded.environment THEN -1 ELSE delivered END,
            next_attempt=0,attempts=0,last_error=NULL,token_version=excluded.token_version
            WHERE excluded.token_version>token_version OR (excluded.token_version=token_version AND excluded.token=token)",
            params![registration.activity_id, registration.push_token.to_lowercase(), registration.environment.name(), registration.thread_id, registration.turn_id, apns::now()+LIFETIME, registration.token_version])?;
        Ok(())
    }
    pub fn remove_activity(&self, removal: &Removal) -> Result<()> {
        ensure!(!removal.activity_id.is_empty() && removal.activity_id.len()<=128 && (64..=512).contains(&removal.push_token.len()) && removal.push_token.bytes().all(|c|c.is_ascii_hexdigit()), "Invalid activity removal");
        let db = self.db.lock().unwrap();
        let tx = db.unchecked_transaction()?;
        tx.execute("DELETE FROM activity_subscriptions WHERE id=?1 AND token=?2", params![removal.activity_id, removal.push_token.to_lowercase()])?;
        tx.execute("INSERT OR REPLACE INTO activity_retired(id,expires) SELECT ?1,?2 WHERE NOT EXISTS(SELECT 1 FROM activity_subscriptions WHERE id=?1)", params![removal.activity_id,apns::now()+LIFETIME])?;
        tx.commit()?;
        Ok(())
    }
    pub fn activity_status(&self) -> Value {
        let db = self.db.lock().unwrap();
        let counts = db.query_row("SELECT count(*),COALESCE(MAX(last_sent),0),count(last_error) FROM activity_subscriptions", [], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?))).unwrap_or_default();
        let last_error: Option<String> = db.query_row("SELECT last_error FROM activity_subscriptions WHERE last_error IS NOT NULL ORDER BY next_attempt DESC LIMIT 1", [], |r|r.get(0)).optional().unwrap_or_default();
        json!({"subscriptions":counts.0,"lastAcceptedAt":counts.1,"failedSubscriptions":counts.2,"lastError":last_error})
    }
    fn activity_threads(&self) -> Result<Vec<String>> {
        Ok(self.db.lock().unwrap().prepare("SELECT DISTINCT s.thread FROM activity_subscriptions s JOIN activity_turns t ON s.thread=t.thread AND s.turn=t.turn WHERE s.expires>?1 AND json_extract(t.data,'$.state.phase') NOT IN ('completed','stopped','failed')")?
            .query_map([apns::now()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }
    fn activity_jobs(&self) -> Result<Vec<Job>> {
        let db = self.db.lock().unwrap();
        let now = apns::now();
        db.execute("DELETE FROM activity_subscriptions WHERE expires<=?1", [now])?;
        db.execute("DELETE FROM activity_retired WHERE expires<=?1", [now])?;
        db.execute("DELETE FROM activity_turns WHERE updated<?1 AND NOT EXISTS(SELECT 1 FROM activity_subscriptions s WHERE s.thread=activity_turns.thread AND s.turn=activity_turns.turn)", [now-LIFETIME])?;
        let rows = db.prepare("SELECT s.id,s.token,s.environment,t.data,t.version,s.attempts,s.last_timestamp,s.expires,s.delivered FROM activity_subscriptions s JOIN activity_turns t ON s.thread=t.thread AND s.turn=t.turn
            WHERE s.next_attempt<=?1 AND (s.delivered!=t.version OR (s.last_sent<=?2 AND json_extract(t.data,'$.state.phase') NOT IN ('completed','stopped','failed')))
            ORDER BY CASE WHEN json_extract(t.data,'$.state.phase') IN ('approval','completed','stopped','failed') THEN 0 ELSE 1 END,s.last_sent LIMIT 32")?
            .query_map(params![now,now-60], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,u32>(5)?,r.get::<_,i64>(6)?,r.get::<_,i64>(7)?,r.get::<_,i64>(8)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|(id,token,environment,data,version,attempts,last_timestamp,expires,delivered)| {
            let turn: Turn = serde_json::from_str(&data)?;
            let timestamp = now.max(last_timestamp+1);
            db.execute("UPDATE activity_subscriptions SET last_timestamp=?2 WHERE id=?1", params![id,timestamp])?;
            let urgent = delivered != version && turn.state.urgent();
            Ok(Job { id, token, environment:serde_json::from_value(json!(environment))?, state:turn.state, version, attempts, timestamp, expires, urgent })
        }).collect()
    }
    fn finish_activity_job(&self, job: &Job, result: Delivery) -> Result<()> {
        let db = self.db.lock().unwrap();
        let now = apns::now();
        match result {
            Delivery::Accepted => { db.execute("UPDATE activity_subscriptions SET delivered=?3,last_sent=?4,
                next_attempt=CASE WHEN EXISTS(SELECT 1 FROM activity_turns t WHERE t.thread=activity_subscriptions.thread AND t.turn=activity_subscriptions.turn AND t.version>?3 AND json_extract(t.data,'$.state.phase') IN ('approval','completed','stopped','failed')) THEN ?4 ELSE ?4+15 END,
                attempts=0,last_error=NULL WHERE id=?1 AND token=?2", params![job.id,job.token,job.version,now])?; }
            Delivery::InvalidToken => { db.execute("DELETE FROM activity_subscriptions WHERE id=?1 AND token=?2", params![job.id,job.token])?; }
            Delivery::Retry { reason, delay } => {
                let backoff = delay.max((5_i64 * 2_i64.pow(job.attempts.min(6))).min(300));
                db.execute("UPDATE activity_subscriptions SET attempts=attempts+1,next_attempt=?3,last_error=?4 WHERE id=?1 AND token=?2", params![job.id,job.token,now+backoff,reason])?;
            }
            Delivery::Rejected(reason) => { db.execute("UPDATE activity_subscriptions SET next_attempt=expires,attempts=attempts+1,last_error=?3 WHERE id=?1 AND token=?2", params![job.id,job.token,reason])?; }
        }
        Ok(())
    }
}

struct Job { id:String, token:String, environment:Environment, state:ContentState, version:i64, attempts:u32, timestamp:i64, expires:i64, urgent:bool }
impl Job {
    fn payload(&self) -> Value {
        let mut aps = json!({"timestamp":self.timestamp,"event":if self.state.terminal() {"end"} else {"update"},"content-state":self.state});
        if self.state.terminal() { aps["dismissal-date"] = json!(self.timestamp+90); }
        else { aps["stale-date"] = json!((self.state.updated_at+APPLE_EPOCH) as i64+180); }
        if self.state.phase == "approval" { aps["relevance-score"] = json!(100); }
        json!({"aps":aps})
    }
}

pub fn start(state: Arc<crate::api::AppState>) {
    let Some(client) = state.apns.clone() else { return };
    let reconcile = state.clone();
    tokio::spawn(async move {
        let mut events = reconcile.events.sender.subscribe();
        loop {
            let codex = reconcile.codex.read().await.clone();
            let connected = codex.as_ref().is_some_and(|c|c.ready.load(Ordering::SeqCst));
            if let Some(codex) = codex && connected {
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
                            Ok(event) if event.payload["method"] == "host/runtime/connected" => break,
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
            match state.events.activity_jobs() {
                Ok(jobs) => stream::iter(jobs).for_each_concurrent(4, |job| {
                    let client = &client;
                    let events = &state.events;
                    async move {
                        let result = client.send(job.environment,&job.token,job.payload(),job.urgent,job.expires).await;
                        if events.finish_activity_job(&job,result).is_err() { eprintln!("APNs delivery persistence failed"); }
                    }
                }).await,
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
    fn event(events: &Events, method: &str, p: Value) { events.publish(json!({"method":method,"params":p})).unwrap(); }
    fn registration(token: &str, version: i64) -> Registration {
        Registration { activity_id:"device-activity".into(),push_token:token.repeat(64),environment:Environment::Sandbox,
            thread_id:"t".into(),turn_id:"r".into(),started_at:apns::now() as f64-10.0,token_version:version }
    }
    fn running(events: &Events) {
        event(events,"turn/started",json!({"threadId":"t","turn":{"id":"r","startedAt":apns::now()-10}}));
    }
    #[test] fn background_events_generate_complete_decodable_payload_without_subscriber_socket() {
        let events=Events::open(Path::new(":memory:")).unwrap();
        running(&events);
        events.register_activity(&registration("a",1)).unwrap();
        event(&events,"item/completed",json!({"threadId":"t","turnId":"r","item":{"id":"tool","type":"commandExecution","status":"completed"}}));
        event(&events,"item/completed",json!({"threadId":"t","turnId":"r","item":{"id":"tool","type":"commandExecution","status":"completed"}}));
        event(&events,"host/request/pending",json!({"requestId":"approval","request":{"id":42,"params":{"threadId":"t","turnId":"r"}}}));
        let job=events.activity_jobs().unwrap().remove(0);
        assert_eq!(job.state.phase,"approval"); assert_eq!(job.state.completed_tools,1);
        assert!(job.state.ended_at.is_none()); assert!(job.state.urgent());
        let payload=job.payload();
        assert_eq!(payload["aps"]["event"],"update");
        assert!(payload["aps"]["content-state"]["startedAt"].as_f64().unwrap()<apns::now() as f64-APPLE_EPOCH);
        assert!(payload["aps"]["timestamp"].as_i64().unwrap()>1_600_000_000);
        events.finish_activity_job(&job,Delivery::Accepted).unwrap();
        event(&events,"turn/completed",json!({"threadId":"t","turn":{"id":"r","status":"completed","completedAt":apns::now()}}));
        let end=events.activity_jobs().unwrap().remove(0);
        assert_eq!(end.payload()["aps"]["event"],"end");
        assert!(end.payload()["aps"]["stale-date"].is_null());
        assert!(end.state.ended_at.is_some()); assert!(end.timestamp>job.timestamp);
        event(&events,"item/started",json!({"threadId":"t","turnId":"r","item":{"id":"late","type":"reasoning"}}));
        assert_eq!(read_turn(&events.db.lock().unwrap(),"t","r").unwrap().unwrap().0.state.phase,"completed");
    }
    #[test] fn rotation_and_late_delivery_cannot_delete_or_replace_new_token() {
        let events=Events::open(Path::new(":memory:")).unwrap(); running(&events);
        events.register_activity(&registration("a",1)).unwrap();
        let old=events.activity_jobs().unwrap().remove(0);
        events.register_activity(&registration("b",2)).unwrap();
        events.register_activity(&registration("a",1)).unwrap();
        events.finish_activity_job(&old,Delivery::InvalidToken).unwrap();
        let new=events.activity_jobs().unwrap().remove(0);
        assert_eq!(new.token,"b".repeat(64));
        events.remove_activity(&Removal{activity_id:old.id,push_token:old.token}).unwrap();
        assert_eq!(events.activity_jobs().unwrap().len(),1);
        events.remove_activity(&Removal{activity_id:new.id,push_token:new.token}).unwrap();
        assert!(events.activity_jobs().unwrap().is_empty());
        assert!(events.register_activity(&registration("b",2)).is_err());
    }
    #[test] fn durable_outbox_survives_restart_and_marks_unverified_running_state() {
        let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("events.sqlite");
        let events=Events::open(&path).unwrap(); running(&events);
        events.register_activity(&registration("a",1)).unwrap();
        let start=events.activity_jobs().unwrap().remove(0).state.started_at;
        drop(events);
        let events=Events::open(&path).unwrap();
        let job=events.activity_jobs().unwrap().remove(0);
        assert_eq!(job.state.phase,"disconnected"); assert_eq!(job.state.started_at,start);
        assert!(job.state.ended_at.is_none());
    }
    #[test] fn native_snapshot_cannot_overwrite_later_journaled_completion() {
        let events=Events::open(Path::new(":memory:")).unwrap(); running(&events);
        let checkpoint=events.last_sequence();
        event(&events,"turn/completed",json!({"threadId":"t","turn":{"id":"r","status":"failed"}}));
        events.observe_activity_snapshot("t",&json!({"data":[{"id":"r","status":"inProgress","items":[]}]}),&json!([]),checkpoint).unwrap();
        assert_eq!(read_turn(&events.db.lock().unwrap(),"t","r").unwrap().unwrap().0.state.phase,"failed");
    }
    #[test] fn empty_running_snapshot_recovers_after_unknown_state_without_resetting_clock() {
        let events=Events::open(Path::new(":memory:")).unwrap(); running(&events);
        let start=read_turn(&events.db.lock().unwrap(),"t","r").unwrap().unwrap().0.state.started_at;
        event(&events,"turn/completed",json!({"threadId":"t","turn":{"id":"r","status":"unknown"}}));
        events.observe_activity_snapshot("t",&json!({"data":[{"id":"r","status":"inProgress","items":[]}]}),&json!([]),events.last_sequence()).unwrap();
        let turn=read_turn(&events.db.lock().unwrap(),"t","r").unwrap().unwrap().0;
        assert_eq!(turn.state.phase,"working"); assert_eq!(turn.state.started_at,start);
        assert!(turn.state.ended_at.is_none());
    }
    #[test] fn completion_of_other_turn_does_not_end_registered_activity() {
        let events=Events::open(Path::new(":memory:")).unwrap(); running(&events);
        events.register_activity(&registration("a",1)).unwrap();
        event(&events,"turn/completed",json!({"threadId":"t","turn":{"id":"another","status":"completed"}}));
        assert_eq!(events.activity_jobs().unwrap().remove(0).state.phase,"working");
    }
    #[test] fn approvals_resume_only_when_last_pending_request_resolves() {
        let events=Events::open(Path::new(":memory:")).unwrap(); running(&events);
        for (id,native) in [("a",1),("b",2)] { event(&events,"host/request/pending",json!({"requestId":id,"request":{"id":native,"params":{"threadId":"t","turnId":"r"}}})); }
        event(&events,"host/request/resolved",json!({"requestId":"a"}));
        assert_eq!(read_turn(&events.db.lock().unwrap(),"t","r").unwrap().unwrap().0.state.phase,"approval");
        event(&events,"serverRequest/resolved",json!({"requestId":2}));
        assert_eq!(read_turn(&events.db.lock().unwrap(),"t","r").unwrap().unwrap().0.state.phase,"working");
    }
    #[test] fn native_approval_resolution_preserves_request_id_types() {
        let events=Events::open(Path::new(":memory:")).unwrap(); running(&events);
        for (wrapper,native) in [("numeric",json!(2)),("text",json!("2")),("quoted",json!("\"2\""))] {
            event(&events,"host/request/pending",json!({"requestId":wrapper,"request":{"id":native,"params":{"threadId":"t","turnId":"r"}}}));
        }
        event(&events,"serverRequest/resolved",json!({"requestId":2}));
        let turn=read_turn(&events.db.lock().unwrap(),"t","r").unwrap().unwrap().0;
        assert_eq!(turn.approvals.len(),2);
        assert!(turn.approvals.contains_key("text") && turn.approvals.contains_key("quoted"));
        assert_eq!(turn.state.phase,"approval");
        event(&events,"serverRequest/resolved",json!({"requestId":"2"}));
        let turn=read_turn(&events.db.lock().unwrap(),"t","r").unwrap().unwrap().0;
        assert_eq!(turn.approvals.len(),1);
        assert!(turn.approvals.contains_key("quoted"));
        assert_eq!(turn.state.phase,"approval");
        event(&events,"serverRequest/resolved",json!({"requestId":"\"2\""}));
        let turn=read_turn(&events.db.lock().unwrap(),"t","r").unwrap().unwrap().0;
        assert!(turn.approvals.is_empty());
        assert_eq!(turn.state.phase,"working");
    }
    #[test] fn retry_is_delayed_and_permanent_failure_does_not_spin() {
        let events=Events::open(Path::new(":memory:")).unwrap(); running(&events);
        events.register_activity(&registration("a",1)).unwrap();
        let job=events.activity_jobs().unwrap().remove(0);
        events.finish_activity_job(&job,Delivery::Retry{reason:"Unavailable".into(),delay:30}).unwrap();
        assert!(events.activity_jobs().unwrap().is_empty());
        events.finish_activity_job(&job,Delivery::Rejected("BadPayload".into())).unwrap();
        event(&events,"turn/completed",json!({"threadId":"t","turn":{"id":"r","status":"completed"}}));
        assert!(events.activity_jobs().unwrap().is_empty());
    }
}
