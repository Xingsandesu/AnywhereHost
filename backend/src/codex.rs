use crate::{config::Config, events::Events};
use anyhow::{Context, Result, bail, ensure};
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
    sync::{Mutex, mpsc, oneshot},
};
use tokio_util::codec::{FramedRead, LinesCodec};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    pub mode: String,
    pub binary: PathBuf,
    pub codex_home: PathBuf,
    pub shares_live_process: bool,
    pub execution_sandbox: String,
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
    writer: mpsc::Sender<Value>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, RpcError>>>>,
    approvals: Mutex<HashMap<String, Value>>,
    next: AtomicU64,
    events: Arc<Events>,
    child: Mutex<Option<Child>>,
    transport_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
pub fn discover(config: &Config) -> Result<(PathBuf, bool)> {
    if let Some(path) = &config.codex_binary {
        ensure!(
            executable(path),
            "configured Codex binary is not executable"
        );
        return Ok((path.clone(), path.to_string_lossy().contains(".app/")));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let apps = [
        "/Applications/Codex.app/Contents/Resources/codex".to_string(),
        "/Applications/Codex.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
            .to_string(),
        "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
            .to_string(),
        format!("{home}/Applications/Codex.app/Contents/Resources/codex"),
        format!(
            "{home}/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
        ),
    ];
    for path in apps {
        let path = PathBuf::from(path);
        if executable(&path) {
            return Ok((path, true));
        }
    }
    let mut dirs: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    dirs.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    for dir in dirs {
        let p = dir.join("codex");
        if executable(&p) {
            return Ok((p, false));
        }
    }
    bail!("未找到主机 Codex。请先安装或指定 Codex App / CLI。")
}
async fn methods(binary: &Path) -> Result<HashSet<String>> {
    let dir = tempfile::tempdir()?;
    let mut cmd = Command::new(binary);
    cmd.args([
        "app-server",
        "generate-json-schema",
        "--experimental",
        "--out",
    ])
    .arg(dir.path())
    .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(20), cmd.output()).await??;
    ensure!(
        output.status.success(),
        "Codex protocol schema generation failed"
    );
    let doc: Value =
        serde_json::from_slice(&tokio::fs::read(dir.path().join("ClientRequest.json")).await?)?;
    Ok(doc["oneOf"]
        .as_array()
        .context("unknown Codex protocol schema")?
        .iter()
        .filter_map(|x| {
            x.pointer("/properties/method/enum/0")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect())
}
async fn login_environment() -> Result<HashMap<String, String>> {
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    ensure!(
        shell.is_absolute() && executable(&shell),
        "login shell is unavailable"
    );
    // This is a fixed environment probe, never built from remote input. Do not log its output.
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(shell)
            .args(["-l", "-c", "/usr/bin/env -0"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("login environment timed out")??;
    ensure!(
        output.status.success(),
        "could not load the user's login environment"
    );
    let mut env = HashMap::new();
    for entry in output.stdout.split(|b| *b == 0) {
        if let Ok(entry) = std::str::from_utf8(entry)
            && let Some((key, value)) = entry.split_once('=')
            && !key.is_empty()
            && key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            env.insert(key.to_owned(), value.to_owned());
        }
    }
    Ok(env)
}
impl Codex {
    pub async fn start(config: &Config, events: Arc<Events>) -> Result<Arc<Self>> {
        let (binary, app) = discover(config)?;
        let methods = methods(&binary).await?;
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
        let mode = if shared {
            "sharedCodexProcess"
        } else if app {
            "appRuntime"
        } else {
            "cliRuntime"
        };
        let note = if shared {
            "已接入现有 Codex 控制 socket；共享该进程的运行状态。"
        } else {
            "复用主机 Codex Home、登录、配置和历史；独立 app-server 进程。桌面已有会话可读取或分叉，不能直接接管，以免并发写入。"
        };
        let app_sandbox = crate::app_sandbox::enabled() && !shared;
        if app_sandbox {
            ensure!(
                methods.contains("thread/settings/update"),
                "Installed Codex does not support App Sandbox execution policy; update Codex."
            );
        }
        let runtime = Runtime {
            mode: mode.into(),
            binary: binary.clone(),
            codex_home: config.codex_home.clone(),
            shares_live_process: shared,
            execution_sandbox: if app_sandbox {
                "macOSAppSandbox"
            } else {
                "codex"
            }
            .into(),
            methods,
            note: note.into(),
        };
        enum Transport {
            Stdio(tokio::process::ChildStdin, tokio::process::ChildStdout),
            Socket(Box<tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>>),
        }
        let (transport, child) = if let Some(socket) = socket {
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
            let (ws, _) = tokio::time::timeout(
                Duration::from_secs(5),
                tokio_tungstenite::client_async("ws://codex-app-server/rpc", stream),
            )
            .await??;
            (Transport::Socket(Box::new(ws)), None)
        } else {
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
            )
        };
        let (writer, mut outgoing) = mpsc::channel::<Value>(128);
        let this = Arc::new(Self {
            runtime,
            ready: AtomicBool::new(false),
            writer,
            pending: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            events,
            child: Mutex::new(child),
            transport_tasks: Mutex::new(Vec::new()),
        });
        match transport {
            Transport::Stdio(stdin, stdout) => {
                let weak = Arc::downgrade(&this);
                let writer_task = tokio::spawn(async move {
                    let mut sink = tokio_util::codec::FramedWrite::new(stdin, LinesCodec::new());
                    while let Some(value) = outgoing.recv().await {
                        if sink.send(value.to_string()).await.is_err() {
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
                                if socket.send(Message::Text(value.to_string().into())).await.is_err(){break;}
                            },
                            message=socket.next()=>{
                                match message {
                                    Some(Ok(Message::Text(text)))=>{
                                        let Some(c)=weak.upgrade() else{break};
                                        let Ok(message)=serde_json::from_str::<Value>(&text) else{break};
                                        c.receive(message).await;
                                    },
                                    Some(Ok(Message::Ping(_)))=>{if socket.flush().await.is_err(){break;}},
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
        if let Some(home) = initialized["codexHome"].as_str() {
            ensure!(
                Path::new(home).canonicalize()? == config.codex_home.canonicalize()?,
                "connected Codex Home differs from the configured original environment"
            );
        }
        this.writer
            .send(json!({"method":"initialized","params":{}}))
            .await?;
        this.ready.store(true, Ordering::SeqCst);
        this.events.publish(json!({"method":"host/runtime/connected","params":{"mode":mode,"sharesLiveProcess":shared}}))?;
        Ok(this)
    }
    async fn disconnected(&self) {
        if self.ready.swap(false, Ordering::SeqCst) {
            let _ = self
                .events
                .publish(json!({"method":"host/runtime/disconnected","params":{}}));
        }
        for (_, tx) in self.pending.lock().await.drain() {
            let _ = tx.send(Err(RpcError::new(
                -32001,
                "Codex disconnected; reconcile state before retrying mutations",
            )));
        }
        self.approvals.lock().await.clear();
    }
    async fn receive(&self, message: Value) {
        if message.get("method").is_none() {
            let key = message["id"].to_string();
            if let Some(tx) = self.pending.lock().await.remove(&key) {
                let result = if let Some(e) = message.get("error") {
                    Err(RpcError {
                        code: e["code"].as_i64().unwrap_or(-32603),
                        message: e["message"].as_str().unwrap_or("Codex error").into(),
                        data: e.get("data").cloned(),
                    })
                } else {
                    Ok(message["result"].clone())
                };
                let _ = tx.send(result);
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
                let _=self.writer.send(json!({"id":message["id"],"error":{"code":-32601,"message":"AnywhereHost does not implement this client-side capability"}})).await;
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
            if message["method"].as_str() == Some("turn/completed") {
                let turn_id = &message["params"]["turn"]["id"];
                self.approvals
                    .lock()
                    .await
                    .retain(|_, pending| pending["params"]["turnId"] != *turn_id);
            }
            let _ = self.events.publish(message);
        }
    }
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = format!("host-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(json!(id).to_string(), tx);
        if self
            .writer
            .send(json!({"id":id,"method":method,"params":params}))
            .await
            .is_err()
        {
            self.pending.lock().await.remove(&json!(id).to_string());
            return Err(RpcError::new(-32001, "Codex is disconnected"));
        }
        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(result)) => result,
            _ => {
                self.pending.lock().await.remove(&json!(id).to_string());
                Err(RpcError::new(
                    -32002,
                    "Codex response timeout; operation outcome is unknown; refresh before retrying",
                ))
            }
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
        self.writer
            .send(json!({"id":request["id"],"result":result}))
            .await
            .map_err(|_| RpcError::new(-32001, "Codex disconnected"))?;
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
