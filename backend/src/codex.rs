use crate::runtime::{discover, login_environment, methods};
use crate::{config::Config, events::Events};
use anyhow::{Context, Result, ensure};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
};
use tokio_util::codec::{FramedRead, LinesCodec};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    pub mode: String,
    pub binary: PathBuf,
    pub codex_home: PathBuf,
    pub shares_live_process: bool,
    pub methods: HashSet<String>,
    pub note: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}
pub struct Codex {
    pub runtime: Runtime,
    pub ready: AtomicBool,
    closed: std::sync::Mutex<bool>,
    writer: mpsc::Sender<Outgoing>,
    outgoing_bytes: Arc<Semaphore>,
    pending: std::sync::Mutex<HashMap<String, PendingRequest>>,
    approvals: Mutex<HashMap<String, Value>>,
    next: AtomicU64,
    events: Arc<Events>,
    text_offsets: std::sync::Mutex<crate::streaming::TextOffsets>,
    child: Mutex<Option<Child>>,
    transport_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
struct PendingRequest {
    reply: oneshot::Sender<Result<Value, RpcError>>,
    creates_thread: bool,
}
struct Outgoing {
    text: String,
    _permit: OwnedSemaphorePermit,
}
impl Codex {
    pub async fn start(config: &Config, events: Arc<Events>) -> Result<Arc<Self>> {
        let default_socket = config
            .codex_home
            .join("app-server-control/app-server-control.sock");
        let socket = config
            .control_socket
            .clone()
            .or_else(|| default_socket.exists().then_some(default_socket));
        if let Some(p) = &socket {
            ensure!(p.exists(), "configured control socket is unavailable");
        }
        let shared = socket.is_some();
        enum Transport {
            Stdio(tokio::process::ChildStdin, tokio::process::ChildStdout),
            Socket(Box<tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>>),
        }
        let (transport, child, binary, app, methods, peer_version) = if let Some(socket) = socket {
            use std::os::unix::fs::{FileTypeExt, MetadataExt};
            let link_metadata = std::fs::symlink_metadata(&socket)?;
            let metadata = std::fs::metadata(&socket)?;
            ensure!(
                metadata.file_type().is_socket(),
                "controlSocket must be a Unix socket"
            );
            let home_metadata = std::fs::metadata(&config.codex_home)?;
            ensure!(
                metadata.uid() == home_metadata.uid() && link_metadata.uid() == home_metadata.uid(),
                "control socket and Codex Home must have the same owner"
            );
            let stream = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::UnixStream::connect(&socket),
            )
            .await??;
            let binary = crate::runtime::peer_binary(&stream, &config.codex_home)?;
            let version = crate::runtime::version(&binary).await?;
            let methods = methods(&binary).await?;
            let (ws, _) = tokio::time::timeout(
                Duration::from_secs(5),
                tokio_tungstenite::client_async("ws://codex-app-server/rpc", stream),
            )
            .await??;
            (
                Transport::Socket(Box::new(ws)),
                None,
                binary,
                false,
                methods,
                Some(version),
            )
        } else {
            let (binary, app) = discover(config)?;
            let methods = methods(&binary).await?;
            let mut cmd = Command::new(&binary);
            cmd.envs(login_environment().await?);
            cmd.args(["app-server", "--listen", "stdio://"]);
            cmd.env("CODEX_HOME", &config.codex_home)
                .current_dir(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
                .env_remove("CODEX_THREAD_ID")
                .env_remove("CODEX_INTERNAL_ORIGINATOR_OVERRIDE")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let mut child = cmd.spawn().context("start Codex app-server")?;
            (
                Transport::Stdio(child.stdin.take().unwrap(), child.stdout.take().unwrap()),
                Some(child),
                binary,
                app,
                methods,
                None,
            )
        };
        let runtime = Runtime {
            mode: if shared { "sharedCodexProcess" } else if app { "appRuntime" } else { "cliRuntime" }.into(),
            binary,
            codex_home: config.codex_home.clone(),
            shares_live_process: shared,
            methods,
            note: if shared { "已接入现有 Codex 控制 socket；共享该进程的运行状态。" } else { "复用主机 Codex Home、登录、配置和历史；独立 app-server 进程。桌面已有会话可读取或分叉，不能直接接管，以免并发写入。" }.into(),
        };
        let (writer, mut outgoing) = mpsc::channel::<Outgoing>(128);
        let this = Arc::new(Self {
            runtime,
            ready: AtomicBool::new(false),
            closed: std::sync::Mutex::new(false),
            writer,
            outgoing_bytes: Arc::new(Semaphore::new(64 * 1024 * 1024)),
            pending: std::sync::Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            events,
            text_offsets: std::sync::Mutex::new(crate::streaming::TextOffsets::default()),
            child: Mutex::new(child),
            transport_tasks: Mutex::new(Vec::new()),
        });
        match transport {
            Transport::Stdio(stdin, stdout) => {
                let weak = Arc::downgrade(&this);
                let writer_task = tokio::spawn(async move {
                    let mut sink = tokio_util::codec::FramedWrite::new(stdin, LinesCodec::new());
                    while let Some(value) = outgoing.recv().await {
                        if !tokio::time::timeout(Duration::from_secs(10), sink.send(value.text))
                            .await
                            .is_ok_and(|r| r.is_ok())
                        {
                            break;
                        }
                    }
                    if let Some(c) = weak.upgrade() {
                        c.disconnected().await;
                    }
                });
                let weak = Arc::downgrade(&this);
                let reader_task = tokio::spawn(async move {
                    let mut stream =
                        FramedRead::new(stdout, LinesCodec::new_with_max_length(32 * 1024 * 1024));
                    while let Some(Ok(line)) = stream.next().await {
                        let Some(c) = weak.upgrade() else { break };
                        let Ok(message) = serde_json::from_str::<Value>(&line) else {
                            c.disconnected().await;
                            break;
                        };
                        c.receive(message).await;
                    }
                    if let Some(c) = weak.upgrade() {
                        c.disconnected().await;
                    }
                });
                this.transport_tasks
                    .lock()
                    .await
                    .extend([writer_task, reader_task]);
            }
            Transport::Socket(mut socket) => {
                let weak = Arc::downgrade(&this);
                let task = tokio::spawn(async move {
                    use tokio_tungstenite::tungstenite::Message;
                    loop {
                        tokio::select! {
                            value=outgoing.recv()=>{
                                let Some(value)=value else{break};
                                if !tokio::time::timeout(Duration::from_secs(10), socket.send(Message::Text(value.text.into()))).await.is_ok_and(|r|r.is_ok()){break;}
                            },
                            message=socket.next()=>{
                                match message {
                                    Some(Ok(Message::Text(text)))=>{
                                        let Some(c)=weak.upgrade() else{break};
                                        let Ok(message)=serde_json::from_str::<Value>(&text) else{break};
                                        c.receive(message).await;
                                    },
                                    Some(Ok(Message::Ping(_)))=>{if !tokio::time::timeout(Duration::from_secs(10), socket.flush()).await.is_ok_and(|r|r.is_ok()){break;}},
                                    Some(Ok(Message::Close(_)))|Some(Err(_))|None=>break,
                                    _=>{},
                                }
                            }
                        }
                    }
                    if let Some(c) = weak.upgrade() {
                        c.disconnected().await;
                    }
                });
                this.transport_tasks.lock().await.push(task);
            }
        }
        let initialized=this.request("initialize",json!({"clientInfo":{"name":"anywhere_host","title":"Anywhere Host","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await.map_err(|e|anyhow::anyhow!(e.message))?;
        if let Some(version) = peer_version {
            crate::runtime::verify_version(&initialized, &version)?;
        }
        if let Some(home) = initialized["codexHome"].as_str() {
            ensure!(
                Path::new(home).canonicalize()? == config.codex_home.canonicalize()?,
                "connected Codex Home differs from the configured original environment"
            );
        }
        this.queue(json!({"method":"initialized","params":{}}))
            .map_err(|e| anyhow::anyhow!(e.message))?;
        this.mark_ready()?;
        Ok(this)
    }
    async fn disconnected(&self) {
        {
            let mut closed = self.closed.lock().unwrap();
            *closed = true;
            if self.ready.swap(false, Ordering::SeqCst) {
                let _ = self
                    .events
                    .publish(json!({"method":"host/runtime/disconnected","params":{}}));
            }
        }
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.reply.send(Err(RpcError::new(
                -32002,
                "Codex disconnected; reconcile state before retrying mutations",
            )));
        }
        self.approvals.lock().await.clear();
    }
    fn mark_ready(&self) -> Result<()> {
        // A transport can die after initialize replies but before startup finishes.
        // Publishing ready must not resurrect that already-disconnected transport.
        let closed = self.closed.lock().unwrap();
        ensure!(!*closed, "Codex disconnected during initialization");
        self.events.invalidate();
        self.events.publish(json!({"method":"host/runtime/connected","params":{"mode":self.runtime.mode,"sharesLiveProcess":self.runtime.shares_live_process}}))?;
        self.ready.store(true, Ordering::SeqCst);
        Ok(())
    }
    async fn receive(&self, mut message: Value) {
        if message.get("method").is_none() {
            let key = message["id"].to_string();
            if let Some(tx) = self.pending.lock().unwrap().remove(&key) {
                let result = if let Some(e) = message.get("error") {
                    Err(RpcError {
                        code: e["code"].as_i64().unwrap_or(-32603),
                        message: e["message"].as_str().unwrap_or("Codex error").into(),
                        data: e.get("data").cloned(),
                    })
                } else {
                    Ok(message["result"].take())
                };
                if tx.creates_thread
                    && let Ok(value) = &result
                    && let Some(id) = value.pointer("/thread/id").and_then(Value::as_str)
                {
                    // Register before reading subsequent notifications/approvals.
                    // api::rpc retains and retries this result if storage is unavailable.
                    let _ = self.events.own(id);
                }
                let _ = tx.reply.send(result);
            }
        } else if message.get("id").is_some() {
            let method = message["method"].as_str().unwrap_or("");
            if !matches!(
                method,
                "item/commandExecution/requestApproval"
                    | "item/fileChange/requestApproval"
                    | "item/tool/requestUserInput"
                    | "mcpServer/elicitation/request"
                    | "item/permissions/requestApproval"
            ) {
                if self.queue(json!({"id":message["id"],"error":{"code":-32601,"message":"AnywhereHost does not implement this client-side capability"}})).is_err() {
                    self.disconnected().await;
                }
                return;
            }
            if !self.runtime.shares_live_process
                && !message["params"]["threadId"]
                    .as_str()
                    .is_some_and(|id| self.events.owns(id))
            {
                if self.queue(json!({"id":message["id"],"error":{"code":-32009,"message":"This runtime cannot approve changes to a foreign thread"}})).is_err() {
                    self.disconnected().await;
                }
                return;
            }
            if self.approvals.lock().await.len() >= 256 {
                if self.queue(json!({"id":message["id"],"error":{"code":-32005,"message":"Too many pending approval requests"}})).is_err() {
                    self.disconnected().await;
                }
                return;
            }
            let id = uuid::Uuid::new_v4().to_string();
            self.approvals
                .lock()
                .await
                .insert(id.clone(), message.clone());
            let _=self.events.publish(json!({"method":"host/request/pending","params":{"requestId":id,"request":message}}));
        } else {
            if message["method"].as_str() == Some("serverRequest/resolved") {
                let request_id = &message["params"]["requestId"];
                self.approvals
                    .lock()
                    .await
                    .retain(|_, pending| pending["id"] != *request_id);
            }
            if message["method"].as_str() == Some("turn/completed")
                && let (Some(thread_id), Some(turn_id)) = (
                    message["params"]["threadId"].as_str(),
                    message["params"]["turn"]["id"].as_str(),
                )
            {
                self.approvals.lock().await.retain(|_, pending| {
                    pending["params"]["turnId"] != turn_id
                        || pending["params"]["threadId"] != thread_id
                });
            }
            self.text_offsets.lock().unwrap().annotate(&mut message);
            let _ = self.events.publish(message);
        }
    }
    fn queue(&self, message: Value) -> Result<(), RpcError> {
        let text = message.to_string();
        let size = u32::try_from(text.len())
            .map_err(|_| RpcError::new(-32005, "Codex request too large"))?;
        let permit = self
            .outgoing_bytes
            .clone()
            .try_acquire_many_owned(size)
            .map_err(|_| {
                RpcError::new(
                    -32005,
                    "Codex outgoing byte budget exhausted; request not dispatched",
                )
            })?;
        self.writer
            .try_send(Outgoing {
                text,
                _permit: permit,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => RpcError::new(
                    -32005,
                    "Codex request queue is full; request was not dispatched",
                ),
                mpsc::error::TrySendError::Closed(_) => {
                    RpcError::new(-32001, "Codex is disconnected")
                }
            })
    }
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        if method != "initialize" && !self.ready.load(Ordering::SeqCst) {
            return Err(RpcError::new(-32001, "Codex is disconnected"));
        }
        let id = format!("host-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let key = json!(id).to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.len() >= 128 {
                return Err(RpcError::new(-32005, "Too many pending Codex requests"));
            }
            pending.insert(
                key.clone(),
                PendingRequest {
                    reply: tx,
                    creates_thread: matches!(method, "thread/start" | "thread/fork"),
                },
            );
        }
        // HTTP cancellation must release response bookkeeping too. A cancelled
        // request may already have been dispatched; it is never retried here.
        struct Pending<'a>(&'a Codex, String);
        impl Drop for Pending<'_> {
            fn drop(&mut self) {
                self.0.pending.lock().unwrap().remove(&self.1);
            }
        }
        let _pending = Pending(self, key);
        let mut message = json!({"id":id,"method":method});
        message["params"] = params;
        self.queue(message)?;
        if method != "initialize" && !crate::policy::read_only(method) {
            return rx.await.unwrap_or_else(|_| {
                Err(RpcError::new(
                    -32002,
                    "Codex disconnected; operation outcome is unknown",
                ))
            });
        }
        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(result)) => result,
            _ => Err(RpcError::new(
                -32002,
                "Codex response timeout; operation outcome is unknown; refresh before retrying",
            )),
        }
    }
    pub async fn pending_requests(&self) -> Value {
        json!(
            self.approvals
                .lock()
                .await
                .iter()
                .map(|(id, request)| json!({"requestId":id,"request":request}))
                .collect::<Vec<_>>()
        )
    }
    pub async fn respond(&self, id: &str, result: Value) -> Result<Value, RpcError> {
        // Removing under one lock makes approval replies single-use across all phones.
        let mut requests = self.approvals.lock().await;
        let request = requests
            .get(id)
            .ok_or_else(|| RpcError::new(-32004, "Request no longer pending"))?;
        validate_reply(request["method"].as_str().unwrap_or(""), &result)?;
        if !self.ready.load(Ordering::SeqCst) {
            return Err(RpcError::new(-32001, "Codex disconnected"));
        }
        self.queue(json!({"id":request["id"],"result":result}))?;
        requests.remove(id);
        let _ = self
            .events
            .publish(json!({"method":"host/request/resolved","params":{"requestId":id}}));
        Ok(json!({"accepted":true}))
    }
    pub async fn stop(&self) {
        for task in self.transport_tasks.lock().await.drain(..) {
            task.abort();
        }
        if let Some(child) = self.child.lock().await.as_mut() {
            let _ = child.kill().await;
        }
        self.disconnected().await;
    }
}

fn validate_reply(method: &str, result: &Value) -> Result<(), RpcError> {
    let valid = match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => matches!(
            result["decision"].as_str(),
            Some("accept" | "acceptForSession" | "decline" | "cancel")
        ),
        "item/tool/requestUserInput" => result["answers"].as_object().is_some_and(|answers| {
            answers.values().all(|v| {
                v["answers"]
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_string))
            })
        }),
        "mcpServer/elicitation/request" => matches!(
            result["action"].as_str(),
            Some("accept" | "decline" | "cancel")
        ),
        "item/permissions/requestApproval" => {
            result["permissions"].is_object()
                && matches!(result["scope"].as_str(), Some("turn" | "session"))
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(RpcError::new(
            -32602,
            "Invalid response for this pending request type",
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(capacity: usize) -> (Arc<Codex>, mpsc::Receiver<Outgoing>) {
        let (writer, receiver) = mpsc::channel(capacity);
        let codex = Arc::new(Codex {
            runtime: Runtime {
                mode: "fixture".into(),
                binary: PathBuf::new(),
                codex_home: PathBuf::new(),
                shares_live_process: false,
                methods: crate::policy::METHODS
                    .iter()
                    .map(|m| m.to_string())
                    .collect(),
                note: String::new(),
            },
            ready: AtomicBool::new(true),
            closed: std::sync::Mutex::new(false),
            writer,
            outgoing_bytes: Arc::new(Semaphore::new(64 * 1024 * 1024)),
            pending: std::sync::Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            events: Arc::new(Events::open(Path::new(":memory:")).unwrap()),
            text_offsets: std::sync::Mutex::new(crate::streaming::TextOffsets::default()),
            child: Mutex::new(None),
            transport_tasks: Mutex::new(Vec::new()),
        });
        (codex, receiver)
    }
    #[tokio::test(start_paused = true)]
    async fn late_creation_reply_registers_ownership_before_following_approval() {
        let (codex, mut receiver) = fixture(8);
        let task = tokio::spawn({
            let c = codex.clone();
            async move { c.request("thread/start", json!({"cwd":"/tmp"})).await }
        });
        let request: Value = serde_json::from_str(&receiver.recv().await.unwrap().text).unwrap();
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(!task.is_finished());
        codex
            .receive(json!({"id":request["id"],"result":{"thread":{"id":"late"}}}))
            .await;
        assert!(codex.events.owns("late"));
        codex.receive(json!({"id":1,"method":"item/commandExecution/requestApproval","params":{"threadId":"late"}})).await;
        assert_eq!(codex.pending_requests().await.as_array().unwrap().len(), 1);
        assert_eq!(task.await.unwrap().unwrap()["thread"]["id"], "late");
    }
    #[tokio::test]
    async fn outgoing_bytes_remain_reserved_until_transport_finishes() {
        let (mut codex, mut receiver) = fixture(8);
        Arc::get_mut(&mut codex).unwrap().outgoing_bytes = Arc::new(Semaphore::new(100));
        codex.queue(json!("x".repeat(70))).unwrap();
        assert_eq!(codex.queue(json!("y".repeat(70))).unwrap_err().code, -32005);
        let sending = receiver.recv().await.unwrap();
        assert_eq!(codex.outgoing_bytes.available_permits(), 28);
        assert_eq!(codex.queue(json!("y".repeat(70))).unwrap_err().code, -32005);
        drop(sending);
        assert_eq!(codex.outgoing_bytes.available_permits(), 100);
        codex.queue(json!("y".repeat(70))).unwrap();
    }
    #[tokio::test]
    async fn cancelled_requests_and_full_queues_release_pending_state() {
        let (codex, mut receiver) = fixture(1);
        let task = tokio::spawn({
            let codex = codex.clone();
            async move { codex.request("model/list", json!({})).await }
        });
        receiver.recv().await.unwrap();
        assert_eq!(codex.pending.lock().unwrap().len(), 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(codex.pending.lock().unwrap().is_empty());
        codex.queue(json!({})).unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            codex.request("model/list", json!({})),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, -32005);
        assert!(codex.pending.lock().unwrap().is_empty());
        codex.events.own("owned").unwrap();
        codex.approvals.lock().await.insert(
            "approval".into(),
            json!({"id":"native","method":"item/commandExecution/requestApproval"}),
        );
        assert_eq!(
            codex
                .respond("approval", json!({"decision":"accept"}))
                .await
                .unwrap_err()
                .code,
            -32005
        );
        assert!(codex.approvals.lock().await.contains_key("approval"));
        receiver.recv().await.unwrap();
        codex
            .respond("approval", json!({"decision":"accept"}))
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&receiver.recv().await.unwrap().text).unwrap()["id"],
            "native"
        );
        assert_eq!(
            codex
                .respond("approval", json!({"decision":"accept"}))
                .await
                .unwrap_err()
                .code,
            -32004
        );
    }
    #[tokio::test]
    async fn foreign_thread_approvals_are_not_exposed_by_independent_runtime() {
        let (codex, mut receiver) = fixture(1);
        codex.receive(json!({"id":1,"method":"item/commandExecution/requestApproval","params":{"threadId":"foreign"}})).await;
        assert!(codex.approvals.lock().await.is_empty());
        assert_eq!(
            serde_json::from_str::<Value>(&receiver.recv().await.unwrap().text).unwrap()["error"]["code"],
            -32009
        );
    }
    #[tokio::test]
    async fn initialization_cannot_resurrect_a_closed_transport() {
        let (codex, _) = fixture(1);
        codex.ready.store(false, Ordering::SeqCst);
        codex.disconnected().await;
        assert!(codex.mark_ready().is_err());
        assert!(!codex.ready.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn completing_a_turn_only_removes_approvals_for_that_thread_and_turn() {
        let (codex, _) = fixture(1);
        for thread in ["one", "two"] {
            codex.approvals.lock().await.insert(
                thread.into(),
                json!({"params":{"threadId":thread,"turnId":"same-turn"}}),
            );
        }
        codex.receive(json!({"method":"turn/completed","params":{"threadId":"one","turn":{"id":"same-turn"}}})).await;
        let requests = codex.approvals.lock().await;
        assert!(!requests.contains_key("one"));
        assert!(requests.contains_key("two"));
    }
    #[test]
    fn mutations_cannot_skip_thread_identity_or_substitute_fork_history() {
        let (codex, _) = fixture(1);
        codex.events.own("owned").unwrap();
        for method in crate::policy::METHODS.iter().copied().filter(|m| {
            !crate::policy::read_only(m)
                && *m != "thread/start"
                && (m.starts_with("thread/") || m.starts_with("turn/") || *m == "review/start")
        }) {
            for params in [
                json!({}),
                json!({"threadId":null}),
                json!({"threadId":4}),
                json!({"threadId":""}),
            ] {
                assert_eq!(
                    crate::policy::validate(method, &params, &codex, &codex.events)
                        .unwrap_err()
                        .code,
                    -32602,
                    "{method}"
                );
            }
            assert!(
                crate::policy::validate(
                    method,
                    &json!({"threadId":"owned"}),
                    &codex,
                    &codex.events
                )
                .is_ok(),
                "{method}"
            );
            if method != "thread/fork" {
                assert_eq!(
                    crate::policy::validate(
                        method,
                        &json!({"threadId":"foreign"}),
                        &codex,
                        &codex.events
                    )
                    .unwrap_err()
                    .code,
                    -32009,
                    "{method}"
                );
            }
        }
        for method in ["thread/resume", "thread/fork"] {
            for params in [
                json!({"threadId":"owned","path":"/tmp/other"}),
                json!({"threadId":"owned","history":[]}),
            ] {
                assert_eq!(
                    crate::policy::validate(method, &params, &codex, &codex.events)
                        .unwrap_err()
                        .code,
                    -32602
                );
            }
        }
    }
    #[test]
    fn approvals_require_structured_decisions() {
        assert!(
            validate_reply(
                "item/commandExecution/requestApproval",
                &json!({"decision":"accept"})
            )
            .is_ok()
        );
        assert!(
            validate_reply(
                "item/commandExecution/requestApproval",
                &json!({"decision":"yes"})
            )
            .is_err()
        );
        assert!(
            validate_reply(
                "item/tool/requestUserInput",
                &json!({"answers":{"q":{"answers":["a"]}}})
            )
            .is_ok()
        );
        assert!(
            validate_reply(
                "item/tool/requestUserInput",
                &json!({"answers":{"q":"oops"}})
            )
            .is_err()
        );
    }
}
