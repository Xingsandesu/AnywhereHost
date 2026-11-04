use anyhow::{Result, bail};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::broadcast;

const RETAIN: i64 = 5000;
pub const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_JOURNAL_BYTES: i64 = 32 * 1024 * 1024;
const REPLAY_PAGE: usize = 64;
const REPLAY_BYTES: usize = 64 * 1024;
const WIRE_CACHE_BYTES: usize = 4 * 1024 * 1024 + 1024;
#[derive(Clone)]
pub struct WireEvent {
    pub sequence: i64,
    pub resync: bool,
    pub text: std::sync::Arc<str>,
}
#[derive(Default)]
struct Journal {
    retained: VecDeque<(i64, i64)>,
    bytes: i64,
    wire: VecDeque<WireEvent>,
    wire_bytes: usize,
}
impl Journal {
    fn cache(&mut self, event: &Event, serialized_payload: Option<&str>) -> WireEvent {
        if let Some(wire) = self
            .wire
            .iter()
            .find(|wire| wire.sequence == event.sequence)
        {
            return wire.clone();
        }
        let text = if let Some(payload) = serialized_payload {
            // Reuse the journal's JSON encoding; never serialize a large Value twice.
            format!(
                "{{\"sequence\":{},\"epoch\":{},\"payload\":{}}}",
                event.sequence,
                serde_json::to_string(&event.epoch).expect("event epoch"),
                payload
            )
        } else {
            serde_json::to_string(event).expect("event JSON")
        };
        let wire = WireEvent {
            sequence: event.sequence,
            resync: event.payload["method"] == "host/resync_required",
            text: text.into(),
        };
        while self.wire.len() >= REPLAY_PAGE || self.wire_bytes + wire.text.len() > WIRE_CACHE_BYTES
        {
            let Some(old) = self.wire.pop_front() else {
                break;
            };
            self.wire_bytes -= old.text.len();
        }
        if wire.text.len() <= WIRE_CACHE_BYTES {
            self.wire_bytes += wire.text.len();
            self.wire.push_back(wire.clone());
        }
        wire
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub sequence: i64,
    pub epoch: String,
    pub payload: Value,
}
pub struct Events {
    pub(crate) db: Mutex<Connection>,
    journal: Mutex<Journal>,
    epoch: RwLock<String>,
    invalid: AtomicBool,
    pub sender: broadcast::Sender<i64>,
}
impl Events {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS events (sequence INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS owned_threads (id TEXT PRIMARY KEY);")?;
        let columns = db
            .prepare("PRAGMA table_info(events)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !columns.iter().any(|name| name == "bytes") {
            db.execute_batch(
                "BEGIN IMMEDIATE;
                ALTER TABLE events ADD COLUMN bytes INTEGER NOT NULL DEFAULT 0;
                UPDATE events SET bytes=length(CAST(payload AS BLOB));
                COMMIT;",
            )?;
        }
        // Events emitted while Host was stopped cannot be replayed. Keep ownership,
        // but require a fresh snapshot across every process restart. AUTOINCREMENT
        // remains monotonic for the live-activity projection in this database.
        let sequence: i64 = db.query_row(
            "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='events'),0)",
            [],
            |r| r.get(0),
        )?;
        db.execute("DELETE FROM events", [])?;
        crate::live_activity::initialize(&db)?;
        crate::live_activity::record_event(
            &db,
            &serde_json::json!({"method":"host/runtime/disconnected"}),
            sequence,
            crate::notifications::now(),
        )?;
        db.execute(
            "INSERT OR REPLACE INTO metadata VALUES ('epoch', ?1)",
            [uuid::Uuid::new_v4().to_string()],
        )?;
        let epoch = db.query_row("SELECT value FROM metadata WHERE key='epoch'", [], |r| {
            r.get(0)
        })?;
        let (sender, _) = broadcast::channel(1024);
        Ok(Self {
            db: Mutex::new(db),
            journal: Mutex::new(Journal::default()),
            epoch: RwLock::new(epoch),
            invalid: AtomicBool::new(false),
            sender,
        })
    }
    pub fn epoch(&self) -> String {
        self.epoch.read().unwrap().clone()
    }
    pub fn invalidate(&self) {
        self.invalid.store(true, Ordering::SeqCst);
        let _ = self.sender.send(0);
    }
    pub fn recover(&self) -> Result<()> {
        let db = self.db.lock().unwrap();
        self.recover_locked(&db)
    }
    fn recover_locked(&self, db: &Connection) -> Result<()> {
        if !self.invalid.load(Ordering::SeqCst) {
            return Ok(());
        }
        let epoch = uuid::Uuid::new_v4().to_string();
        let tx = db.unchecked_transaction()?;
        tx.execute("DELETE FROM events", [])?;
        tx.execute("UPDATE metadata SET value=?1 WHERE key='epoch'", [&epoch])?;
        tx.commit()?;
        *self.journal.lock().unwrap() = Journal::default();
        *self.epoch.write().unwrap() = epoch;
        self.invalid.store(false, Ordering::SeqCst);
        let _ = self.sender.send(0);
        Ok(())
    }
    pub fn publish(&self, payload: Value) -> Result<Event> {
        // The DB lock also serializes invalidation: no subsequent publisher may
        // hide a failed write behind a seemingly continuous cursor.
        let db = self.db.lock().unwrap();
        let result = self
            .recover_locked(&db)
            .and_then(|()| self.publish_locked(&db, payload));
        if result.is_err() {
            self.invalidate();
        }
        result
    }
    fn publish_locked(&self, db: &Connection, payload: Value) -> Result<Event> {
        let text = payload.to_string();
        let (payload, text) = if text.len() > MAX_EVENT_BYTES {
            let marker = serde_json::json!({"method":"host/resync_required","params":{"reason":"event_too_large"}});
            let text = marker.to_string();
            (marker, text)
        } else {
            (payload, text)
        };
        let mut journal = self.journal.lock().unwrap();
        let transaction = db.unchecked_transaction()?;
        transaction
            .prepare_cached("INSERT INTO events(payload,bytes) VALUES (?1,?2)")?
            .execute(params![text, text.len() as i64])?;
        let sequence = transaction.last_insert_rowid();
        crate::live_activity::record_event(
            &transaction,
            &payload,
            sequence,
            crate::notifications::now(),
        )?;
        // Compute only the prefix that must be evicted; do not scan 5,000 rows
        // on every delta. Update memory only after the durable commit succeeds.
        let mut bytes = journal.bytes + text.len() as i64;
        let mut count = journal.retained.len() + 1;
        let mut evicted = 0;
        let mut cutoff = None;
        for &(old_sequence, size) in &journal.retained {
            if count <= RETAIN as usize && bytes <= MAX_JOURNAL_BYTES {
                break;
            }
            bytes -= size;
            count -= 1;
            evicted += 1;
            cutoff = Some(old_sequence);
        }
        if let Some(cutoff) = cutoff {
            transaction
                .prepare_cached("DELETE FROM events WHERE sequence<=?1")?
                .execute([cutoff])?;
        }
        transaction.commit()?;
        journal.retained.drain(..evicted);
        journal.retained.push_back((sequence, text.len() as i64));
        journal.bytes = bytes;
        let event = Event {
            sequence,
            epoch: self.epoch(),
            payload,
        };
        // One UTF-8 serialization shared by all live clients. Cache is byte-bound;
        // broadcasts remain sequence-only, so lagging receivers retain no bodies.
        journal.cache(&event, Some(&text));
        // Keep the DB lock through send, so concurrent publishers cannot reorder sequences.
        let _ = self.sender.send(sequence);
        Ok(event)
    }
    pub fn replay(&self, after: i64, epoch: Option<&str>) -> Result<Vec<Event>> {
        let db = self.db.lock().unwrap();
        if self.invalid.load(Ordering::SeqCst)
            || after < 0
            || epoch.is_some_and(|e| e != self.epoch())
            || (after > 0 && epoch.is_none())
        {
            bail!("resync_required: invalid event epoch");
        }
        let (min, max): (i64, i64) = db.query_row(
            "SELECT COALESCE((SELECT MIN(sequence) FROM events),0), COALESCE((SELECT seq FROM sqlite_sequence WHERE name='events'),0)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if after > max || (after > 0 && after < min - 1) {
            bail!("resync_required: event cursor expired");
        }
        let mut query = db
            .prepare("SELECT sequence,payload,bytes FROM events WHERE sequence > ?1 ORDER BY sequence LIMIT 64")?;
        let rows = query.query_map([after], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u32>(2)? as usize,
            ))
        })?;
        let mut result = Vec::with_capacity(REPLAY_PAGE);
        let mut bytes = 0;
        for row in rows {
            let (sequence, payload, size) = row?;
            if !result.is_empty() && bytes + size > REPLAY_BYTES {
                break;
            }
            bytes += size;
            result.push(Event {
                sequence,
                epoch: self.epoch(),
                payload: serde_json::from_str(&payload)?,
            });
        }
        Ok(result)
    }
    pub fn get(&self, sequence: i64) -> Result<Event> {
        let db = self.db.lock().unwrap();
        let payload: String = db
            .query_row(
                "SELECT payload FROM events WHERE sequence=?1",
                [sequence],
                |r| r.get(0),
            )
            .map_err(|_| anyhow::anyhow!("resync_required: event cursor expired"))?;
        Ok(Event {
            sequence,
            epoch: self.epoch(),
            payload: serde_json::from_str(&payload)?,
        })
    }
    #[cfg(test)]
    pub fn wire(&self, sequence: i64) -> Result<WireEvent> {
        {
            let journal = self.journal.lock().unwrap();
            if let Some(wire) = journal.wire.iter().find(|wire| wire.sequence == sequence)
                && journal
                    .retained
                    .front()
                    .is_some_and(|(first, _)| sequence >= *first)
            {
                return Ok(wire.clone());
            }
        }
        let event = self.get(sequence)?;
        Ok(self.journal.lock().unwrap().cache(&event, None))
    }
    pub fn encode(&self, event: &Event) -> WireEvent {
        self.journal.lock().unwrap().cache(event, None)
    }
    pub fn last_sequence(&self) -> i64 {
        self.db
            .lock()
            .unwrap()
            .query_row(
                "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='events'),0)",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }
    pub fn own(&self, id: &str) -> Result<()> {
        self.db
            .lock()
            .unwrap()
            .execute("INSERT OR IGNORE INTO owned_threads VALUES (?1)", [id])?;
        Ok(())
    }
    pub fn owns(&self, id: &str) -> bool {
        self.db
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM owned_threads WHERE id=?1",
                params![id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn wire_cache_is_shared_bounded_and_cannot_replay_expired_events() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        let first = events
            .publish(json!({"method":"fixture","text":"x".repeat(1024)}))
            .unwrap();
        let a = events.wire(first.sequence).unwrap();
        let b = events.wire(first.sequence).unwrap();
        assert_eq!(a.text.as_ptr(), b.text.as_ptr());
        assert_eq!(
            serde_json::from_str::<Event>(&a.text).unwrap().payload,
            first.payload
        );
        for _ in 0..20 {
            events
                .publish(json!({"method":"fixture","text":"x".repeat(2 * 1024 * 1024)}))
                .unwrap();
        }
        assert!(events.wire(first.sequence).is_err());
        let journal = events.journal.lock().unwrap();
        assert!(journal.wire_bytes <= WIRE_CACHE_BYTES);
        assert!(journal.wire.len() <= REPLAY_PAGE);
    }
    #[test]
    fn failed_transaction_invalidates_replay_and_recovery_rotates_epoch() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        let mut receiver = events.sender.subscribe();
        events.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(events.publish(json!({"method":"fixture"})).is_err());
        assert!(events.journal.lock().unwrap().retained.is_empty());
        assert_eq!(receiver.try_recv().unwrap(), 0);
        assert!(events.replay(0, None).is_err());
        let old_epoch = events.epoch();
        events
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_event")
            .unwrap();
        let event = events.publish(json!({"method":"fixture"})).unwrap();
        assert_eq!(event.sequence, 1);
        assert_ne!(events.epoch(), old_epoch);
        assert!(events.replay(0, Some(&old_epoch)).is_err());
        assert_eq!(receiver.try_recv().unwrap(), 0);
        assert_eq!(receiver.try_recv().unwrap(), 1);
    }
    #[test]
    #[ignore = "release benchmark; run alone with --test-threads=1 --nocapture"]
    fn benchmark_event_paths() {
        let dir = tempfile::tempdir().unwrap();
        for (name, path) in [
            ("journal_memory_5k", std::path::PathBuf::from(":memory:")),
            ("journal_disk_5k", dir.path().join("events.sqlite")),
        ] {
            let events = Events::open(&path).unwrap();
            let payload = json!({"method":"item/agentMessage/delta","params":{"threadId":"fixture","turnId":"turn","itemId":"item","delta":"x".repeat(128)}});
            for _ in 0..5000 {
                events.publish(payload.clone()).unwrap();
            }
            crate::benchmark::measure(name, 1000, |_| {
                std::hint::black_box(events.publish(payload.clone()).unwrap());
            });
        }
        let events = Events::open(Path::new(":memory:")).unwrap();
        let event = events
            .publish(json!({"method":"fixture","text":"x".repeat(128 * 1024)}))
            .unwrap();
        crate::benchmark::measure("event_fanout_128k", 3200, |_| {
            std::hint::black_box(events.wire(event.sequence).unwrap());
        });
        let frames: Vec<_> = (0..32)
            .map(|_| events.wire(event.sequence).unwrap())
            .collect();
        let buffers: std::collections::HashMap<_, _> = frames
            .iter()
            .map(|frame| (frame.text.as_ptr() as usize, frame.text.len()))
            .collect();
        println!(
            "MEMORY {}",
            json!({"eventWireBytes32Clients": buffers.values().sum::<usize>()})
        );
        std::hint::black_box(frames);
    }
    #[test]
    fn event_byte_budget_paged_replay_and_oversize_resync() {
        let events = Events::open(Path::new(":memory:")).unwrap();
        let payload = json!({"method":"fixture","text":"x".repeat(MAX_EVENT_BYTES / 2)});
        for _ in 0..18 {
            events.publish(payload.clone()).unwrap();
        }
        let bytes: i64 = events
            .db
            .lock()
            .unwrap()
            .query_row("SELECT SUM(bytes) FROM events", [], |r| r.get(0))
            .unwrap();
        assert!(bytes <= MAX_JOURNAL_BYTES);
        assert!(events.replay(1, Some(&events.epoch())).is_err());
        let mut page = events.replay(0, None).unwrap();
        assert_eq!(page.len(), 1);
        let mut last = page[0].sequence;
        while last < events.last_sequence() {
            page = events.replay(last, Some(&events.epoch())).unwrap();
            assert_eq!(page[0].sequence, last + 1);
            last = page.last().unwrap().sequence;
        }
        let marker = events
            .publish(json!({"text":"x".repeat(MAX_EVENT_BYTES)}))
            .unwrap();
        assert_eq!(marker.payload["method"], "host/resync_required");
        assert_eq!(events.get(marker.sequence).unwrap().payload, marker.payload);
    }
    #[test]
    fn restart_preserves_ownership_but_requires_a_new_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.sqlite");
        let e = Events::open(&path).unwrap();
        e.publish(json!({"method":"turn/completed"})).unwrap();
        e.own("thread-a").unwrap();
        let epoch = e.epoch();
        drop(e);
        let e = Events::open(&path).unwrap();
        assert_ne!(e.epoch(), epoch);
        assert!(e.owns("thread-a"));
        assert!(e.replay(0, None).unwrap().is_empty());
        assert!(e.replay(1, Some("wrong")).is_err());
        assert!(e.replay(2, Some(&epoch)).is_err());
        assert_eq!(e.last_sequence(), 1);
        assert!(e.replay(1, Some(&e.epoch())).unwrap().is_empty());
    }
    #[test]
    fn retention_requires_resync() {
        let e = Events::open(Path::new(":memory:")).unwrap();
        for _ in 0..RETAIN + 2 {
            e.publish(Value::Null).unwrap();
        }
        assert!(e.replay(1, Some(&e.epoch())).is_err());
        let first = e.replay(0, None).unwrap();
        assert_eq!(first.len(), REPLAY_PAGE);
        assert_eq!(first[0].sequence, 3);
        assert_eq!(
            e.replay(first.last().unwrap().sequence, Some(&e.epoch()))
                .unwrap()[0]
                .sequence,
            first.last().unwrap().sequence + 1
        );
    }
}
