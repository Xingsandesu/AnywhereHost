//! Durable dispatch reservations: interrupted operations remain indeterminate and are never dispatched twice.
use anyhow::{Result, ensure};
use anywhere_transport::Request;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

const RESPONSE_RETENTION: i64 = 24 * 60 * 60;
const RESPONSE_COUNT: i64 = 1024;
const RESPONSE_BUDGET: i64 = 32 * 1024 * 1024;

pub struct Operations(Mutex<Connection>);
impl Operations {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS operations (owner TEXT NOT NULL, id TEXT NOT NULL, digest BLOB NOT NULL, response TEXT, PRIMARY KEY(owner,id));")?;
        let columns = db
            .prepare("PRAGMA table_info(operations)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !columns.iter().any(|c| c == "completed_at") {
            db.execute_batch("BEGIN IMMEDIATE; ALTER TABLE operations ADD COLUMN completed_at INTEGER; ALTER TABLE operations ADD COLUMN response_bytes INTEGER NOT NULL DEFAULT 0; UPDATE operations SET completed_at=unixepoch(),response_bytes=length(CAST(response AS BLOB)) WHERE response IS NOT NULL; COMMIT;")?;
        }
        db.execute_batch("CREATE INDEX IF NOT EXISTS operations_completed ON operations(completed_at) WHERE response IS NOT NULL;")?;
        Self::compact(&db)?;
        Ok(Self(Mutex::new(db)))
    }
    pub async fn execute<F, Fut>(
        self: &Arc<Self>,
        owner: &str,
        request: Request,
        dispatch: F,
    ) -> Value
    where
        F: FnOnce(Request) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Value> + Send + 'static,
    {
        // Cached results remain queryable even when all execution slots are busy.
        match self.lookup(owner, &request) {
            Ok(Some(value)) => return value,
            Err(e) => return crate::network::error(e),
            Ok(None) => {}
        }
        static MUTATIONS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(64);
        let Ok(permit) = MUTATIONS.try_acquire() else {
            return crate::network::error("Too many active operations");
        };
        match self.reserve(owner, &request) {
            Ok(Some(value)) => return value,
            Err(e) => return crate::network::error(e),
            Ok(None) => {}
        }
        let db = self.clone();
        let owner = owner.to_owned();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            // Capacity belongs to execution, not to the requesting network stream.
            let _permit = permit;
            let id = request.id.clone();
            let response = dispatch(request).await;
            while db.finish(&owner, &id, &response).is_err() {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let _ = tx.send(response);
        });
        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(response)) => response,
            _ => {
                json!({"error":{"code":-32002,"message":"Operation outcome is not yet known; query again with the same request ID"}})
            }
        }
    }
    fn lookup(&self, owner: &str, request: &Request) -> Result<Option<Value>> {
        Self::saved(&self.0.lock().unwrap(), owner, request)
    }
    fn saved(db: &Connection, owner: &str, request: &Request) -> Result<Option<Value>> {
        let row: Option<(Vec<u8>, Option<String>)> = db
            .query_row(
                "SELECT digest,CASE WHEN completed_at >= unixepoch()-?3 THEN response END FROM operations WHERE owner=?1 AND id=?2",
                params![owner, request.id, RESPONSE_RETENTION],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((saved, response)) = row {
            ensure!(
                saved == Self::digest(request)?,
                "Operation ID reused with different input"
            );
            return Ok(Some(match response {
                Some(text) => serde_json::from_str(&text)?,
                None => {
                    json!({"error":{"code":-32002,"message":"Operation result is pending, unavailable or expired; never redispatch this ID"}})
                }
            }));
        }
        Ok(None)
    }
    fn digest(request: &Request) -> Result<Vec<u8>> {
        Ok(Sha256::digest(serde_json::to_vec(&(&request.method, &request.params))?).to_vec())
    }
    pub fn reserve(
        &self,
        owner: &str,
        request: &anywhere_transport::Request,
    ) -> Result<Option<Value>> {
        ensure!(
            !request.id.is_empty() && request.id.len() <= 128,
            "Invalid operation ID"
        );
        let digest = Self::digest(request)?;
        let mut db = self.0.lock().unwrap();
        let tx = db.transaction()?;
        if let Some(value) = Self::saved(&tx, owner, request)? {
            return Ok(Some(value));
        }
        tx.execute(
            "INSERT INTO operations(owner,id,digest) VALUES(?1,?2,?3)",
            params![owner, request.id, digest.as_slice()],
        )?;
        tx.commit()?;
        Ok(None)
    }
    pub fn finish(&self, owner: &str, id: &str, response: &Value) -> Result<()> {
        let mut db = self.0.lock().unwrap();
        let tx = db.transaction()?;
        let text = serde_json::to_string(response)?;
        tx.execute(
            "UPDATE operations SET response=?3,completed_at=unixepoch(),response_bytes=?4 WHERE owner=?1 AND id=?2",
            params![owner, id, text, text.len() as i64],
        )?;
        Self::compact(&tx)?;
        tx.commit()?;
        Ok(())
    }
    fn compact(db: &Connection) -> Result<()> {
        // Never delete reservations: a forgotten UUID could be dispatched again.
        // Only the replayable result bodies expire. Small identity/digest tombstones
        // remain durable and impose no global lifetime operation-count ceiling.
        db.execute("UPDATE operations SET response=NULL,response_bytes=0 WHERE response IS NOT NULL AND completed_at < unixepoch()-?1", [RESPONSE_RETENTION])?;
        db.execute("UPDATE operations SET response=NULL,response_bytes=0 WHERE rowid IN (SELECT rowid FROM (SELECT rowid,SUM(response_bytes) OVER (ORDER BY completed_at DESC,rowid DESC) AS bytes, ROW_NUMBER() OVER (ORDER BY completed_at DESC,rowid DESC) AS rank FROM operations WHERE response IS NOT NULL) WHERE bytes>?1 OR rank>?2)", params![RESPONSE_BUDGET, RESPONSE_COUNT])?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn caller_timeout_and_cancellation_preserve_late_results() {
        let db = Arc::new(Operations::open(Path::new(":memory:")).unwrap());
        for cancel in [false, true] {
            let request = Request {
                id: format!("delayed-{cancel}"),
                method: "codex.call".into(),
                params: Value::Null,
            };
            let (complete, waiting) = tokio::sync::oneshot::channel();
            let task = tokio::spawn({
                let db = db.clone();
                let request = request.clone();
                async move {
                    db.execute("device", request, |_| async { waiting.await.unwrap() })
                        .await
                }
            });
            tokio::task::yield_now().await;
            if cancel {
                task.abort();
                let _ = task.await;
            } else {
                tokio::time::advance(Duration::from_secs(61)).await;
                assert_eq!(task.await.unwrap()["error"]["code"], -32002);
            }
            // A retry while running never invokes the dispatcher.
            assert_eq!(
                db.execute("device", request.clone(), |_| async {
                    panic!("redispatched")
                })
                .await["error"]["code"],
                -32002
            );
            complete
                .send(json!({"result":{"thread":{"id":"late"}}}))
                .unwrap();
            tokio::task::yield_now().await;
            let value = db
                .execute("device", request, |_| async { panic!("redispatched") })
                .await;
            assert_eq!(value["result"]["thread"]["id"], "late");
        }
    }
    #[tokio::test(start_paused = true)]
    async fn completion_retries_storage_failure_without_reexecuting_native_work() {
        let db = Arc::new(Operations::open(Path::new(":memory:")).unwrap());
        db.0.lock().unwrap().execute_batch("CREATE TRIGGER fail_finish BEFORE UPDATE ON operations BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        let request = Request {
            id: "retry-write".into(),
            method: "codex.call".into(),
            params: Value::Null,
        };
        let task = tokio::spawn({
            let db = db.clone();
            let request = request.clone();
            async move {
                db.execute("device", request, |_| async { json!({"result":42}) })
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        db.0.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_finish")
            .unwrap();
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(task.await.unwrap()["result"], 42);
        assert_eq!(
            db.lookup("device", &request).unwrap().unwrap()["result"],
            42
        );
    }
    #[test]
    fn compacted_results_never_redispatch_and_new_operations_keep_working() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ops.sqlite");
        let db = Operations::open(&path).unwrap();
        let req = |id: String| anywhere_transport::Request {
            id,
            method: "codex.call".into(),
            params: serde_json::json!({"method":"turn/start"}),
        };
        for i in 0..RESPONSE_COUNT + 2 {
            let r = req(i.to_string());
            assert!(db.reserve("device", &r).unwrap().is_none());
            db.finish("device", &r.id, &serde_json::json!({"result":i}))
                .unwrap();
        }
        assert_eq!(
            db.reserve("device", &req("0".into())).unwrap().unwrap()["error"]["code"],
            -32002
        );
        assert!(db.reserve("device", &req("new".into())).unwrap().is_none());
        db.0.lock()
            .unwrap()
            .execute("UPDATE operations SET completed_at=0", [])
            .unwrap();
        drop(db);
        let db = Operations::open(&path).unwrap();
        assert_eq!(
            db.reserve("device", &req(RESPONSE_COUNT.to_string()))
                .unwrap()
                .unwrap()["error"]["code"],
            -32002
        );
        assert!(
            db.reserve("device", &req("after-restart".into()))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn response_byte_budget_keeps_small_durable_reservations() {
        let db = Operations::open(Path::new(":memory:")).unwrap();
        let req = anywhere_transport::Request {
            id: "large".into(),
            method: "codex.call".into(),
            params: Value::Null,
        };
        db.reserve("device", &req).unwrap();
        db.finish(
            "device",
            &req.id,
            &serde_json::json!({"text":"x".repeat(RESPONSE_BUDGET as usize)}),
        )
        .unwrap();
        assert_eq!(
            db.reserve("device", &req).unwrap().unwrap()["error"]["code"],
            -32002
        );
        assert_eq!(
            db.0.lock()
                .unwrap()
                .query_row(
                    "SELECT COALESCE(SUM(response_bytes),0) FROM operations",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
    #[test]
    fn restart_and_lost_reply_do_not_repeat_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ops.sqlite");
        let req = anywhere_transport::Request {
            id: "one".into(),
            method: "codex.call".into(),
            params: serde_json::json!({"method":"turn/start"}),
        };
        let db = Operations::open(&path).unwrap();
        assert!(db.reserve("device", &req).unwrap().is_none());
        drop(db);
        let db = Operations::open(&path).unwrap();
        assert_eq!(
            db.reserve("device", &req).unwrap().unwrap()["error"]["code"],
            -32002
        );
        let result = serde_json::json!({"result":123});
        db.finish("device", "one", &result).unwrap();
        assert_eq!(db.reserve("device", &req).unwrap(), Some(result));
        let mut conflict = req.clone();
        conflict.params = Value::Null;
        assert!(db.reserve("device", &conflict).is_err());
        assert!(db.reserve("other", &req).unwrap().is_none());
    }
}
