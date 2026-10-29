use anyhow::{Result, bail};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, sync::Mutex};
use tokio::sync::broadcast;

const RETAIN: i64 = 5000;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub sequence: i64,
    pub epoch: String,
    pub payload: Value,
}
pub struct Events {
    pub(crate) db: Mutex<Connection>,
    pub epoch: String,
    pub sender: broadcast::Sender<Event>,
}
impl Events {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS events (sequence INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS owned_threads (id TEXT PRIMARY KEY);")?;
        crate::live_activity::initialize(&db)?;
        let sequence = db.query_row("SELECT COALESCE(MAX(sequence),0) FROM events", [], |r| r.get::<_,i64>(0))?;
        crate::live_activity::record_event(&db, &serde_json::json!({"method":"host/runtime/disconnected"}), sequence, crate::apns::now())?;
        db.execute(
            "INSERT OR IGNORE INTO metadata VALUES ('epoch', ?1)",
            [uuid::Uuid::new_v4().to_string()],
        )?;
        let epoch = db.query_row("SELECT value FROM metadata WHERE key='epoch'", [], |r| {
            r.get(0)
        })?;
        let (sender, _) = broadcast::channel(1024);
        Ok(Self {
            db: Mutex::new(db),
            epoch,
            sender,
        })
    }
    pub fn publish(&self, payload: Value) -> Result<Event> {
        let db = self.db.lock().unwrap();
        let transaction = db.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO events(payload) VALUES (?1)",
            [payload.to_string()],
        )?;
        let sequence = transaction.last_insert_rowid();
        crate::live_activity::record_event(&transaction, &payload, sequence, crate::apns::now())?;
        transaction.execute(
            "DELETE FROM events WHERE sequence <= ?1",
            [sequence - RETAIN],
        )?;
        transaction.commit()?;
        let event = Event {
            sequence,
            epoch: self.epoch.clone(),
            payload,
        };
        // Keep the DB lock through send, so concurrent publishers cannot reorder sequences.
        let _ = self.sender.send(event.clone());
        Ok(event)
    }
    pub fn replay(&self, after: i64, epoch: Option<&str>) -> Result<Vec<Event>> {
        let db = self.db.lock().unwrap();
        if after < 0 || (after > 0 && epoch != Some(self.epoch.as_str())) {
            bail!("resync_required: invalid event epoch");
        }
        let (min, max): (i64, i64) = db.query_row(
            "SELECT COALESCE(MIN(sequence),0), COALESCE(MAX(sequence),0) FROM events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if after > max || (after > 0 && after < min - 1) {
            bail!("resync_required: event cursor expired");
        }
        let mut query = db
            .prepare("SELECT sequence,payload FROM events WHERE sequence > ?1 ORDER BY sequence")?;
        let rows = query.query_map([after], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (sequence, payload) = row?;
            Ok(Event {
                sequence,
                epoch: self.epoch.clone(),
                payload: serde_json::from_str(&payload)?,
            })
        })
        .collect()
    }
    pub fn last_sequence(&self) -> i64 {
        self.db
            .lock()
            .unwrap()
            .query_row("SELECT COALESCE(MAX(sequence),0) FROM events", [], |r| {
                r.get(0)
            })
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
    fn durable_replay_and_epoch_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.sqlite");
        let e = Events::open(&path).unwrap();
        e.publish(json!({"method":"turn/completed"})).unwrap();
        e.own("thread-a").unwrap();
        let epoch = e.epoch.clone();
        drop(e);
        let e = Events::open(&path).unwrap();
        assert_eq!(e.epoch, epoch);
        assert!(e.owns("thread-a"));
        assert_eq!(e.replay(0, None).unwrap().len(), 1);
        assert!(e.replay(1, Some("wrong")).is_err());
        assert!(e.replay(2, Some(&epoch)).is_err());
    }
    #[test]
    fn retention_requires_resync() {
        let e = Events::open(Path::new(":memory:")).unwrap();
        for _ in 0..RETAIN + 2 {
            e.publish(Value::Null).unwrap();
        }
        assert!(e.replay(1, Some(&e.epoch)).is_err());
        assert_eq!(e.replay(0, None).unwrap().len(), RETAIN as usize);
    }
}
