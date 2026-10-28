use crate::{
    codex::{Codex, RpcError},
    config::Config,
    events::{Event, Events},
    policy,
    tunnel::{Tunnel, TunnelConfig},
};
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, RwLock, broadcast};

pub struct AppState {
    pub token: String,
    pub fingerprint: String,
    pub config: Config,
    pub codex: RwLock<Option<Arc<Codex>>>,
    pub runtime_error: RwLock<Option<String>>,
    pub connecting: Mutex<()>,
    pub events: Arc<Events>,
    pub tunnel: Tunnel,
}
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/status", get(status))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/rpc", post(rpc))
        .route("/v1/events", get(events))
        .route("/v1/requests", get(requests))
        .route("/v1/threads/{id}/access", get(thread_access))
        .route("/v1/requests/{id}/respond", post(respond))
        .route("/v1/projects/{id}/files", get(project_files))
        .route("/v1/projects/{id}/file", get(project_file))
        .route("/v1/projects/{id}/git", get(project_git))
        .route("/v1/runtime/connect", post(connect))
        .route("/v1/tunnel", get(tunnel_status).post(tunnel_start))
        .route("/v1/tunnel/stop", post(tunnel_stop))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}
async fn auth(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // Native API: forbid browser origins and query-string credentials, including WebSocket upgrades.
    if req.headers().contains_key("origin") {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"browser_origin_not_allowed"})),
        )
            .into_response();
    }
    let token = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if !bool::from(token.as_bytes().ct_eq(state.token.as_bytes())) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
async fn status(State(s): State<Arc<AppState>>) -> Json<Value> {
    let c = s.codex.read().await;
    Json(
        json!({"name":"AnywhereHost","version":env!("CARGO_PKG_VERSION"),"protocolVersion":1,"connected":c.as_ref().is_some_and(|c|c.ready.load(Ordering::SeqCst)),"runtime":c.as_ref().map(|c|&c.runtime),"error":s.runtime_error.read().await.clone(),"eventEpoch":s.events.epoch,"lastEventSequence":s.events.last_sequence(),"certificateSHA256":s.fingerprint,"listen":s.config.listen.to_string()}),
    )
}
async fn capabilities(State(s): State<Arc<AppState>>) -> Json<Value> {
    let c = s.codex.read().await;
    let methods: Vec<_> = policy::METHODS
        .iter()
        .filter(|m| c.as_ref().is_some_and(|c| c.runtime.methods.contains(**m)))
        .collect();
    Json(
        json!({"protocolVersion":1,"methods":methods,"events":{"replay":true,"retained":5000,"epoch":s.events.epoch},"approvals":true,"threadAccess":true,"tlsPinning":"sha256-leaf-certificate","sharedProcessControl":c.as_ref().is_some_and(|c|c.runtime.shares_live_process),"desktopAppAttachment":"unverified","foreignThreadMutation":"requires_shared_process","ccConnect":false}),
    )
}
// Exposes the same mutation gate used by RPC policy, so clients can offer a native fork action.
async fn thread_access(State(s): State<Arc<AppState>>, Path(id): Path<String>) -> Json<Value> {
    let c = s.codex.read().await;
    let shared = c.as_ref().is_some_and(|c| c.runtime.shares_live_process);
    let connected = c.as_ref().is_some_and(|c| c.ready.load(Ordering::SeqCst));
    let owned = s.events.owns(&id);
    Json(
        json!({"threadId":id,"canMutate":connected && (shared || owned),"hostCreated":owned,"sharedProcess":shared}),
    )
}
pub async fn connect_runtime(s: &Arc<AppState>) -> Result<(), String> {
    let _guard = s.connecting.lock().await;
    {
        let c = s.codex.read().await;
        if c.as_ref().is_some_and(|c| c.ready.load(Ordering::SeqCst)) {
            return Ok(());
        }
    }
    if let Some(old) = s.codex.write().await.take() {
        old.stop().await;
    }
    match Codex::start(&s.config, s.events.clone()).await {
        Ok(c) => {
            *s.codex.write().await = Some(c);
            *s.runtime_error.write().await = None;
            Ok(())
        }
        Err(e) => {
            let message = e.to_string();
            *s.runtime_error.write().await = Some(message.clone());
            Err(message)
        }
    }
}
async fn connect(State(s): State<Arc<AppState>>) -> Response {
    match connect_runtime(&s).await {
        Ok(()) => Json(json!({"connected":true})).into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":e}))).into_response(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcCall {
    pub method: String,
    #[serde(default = "empty_object")]
    pub params: Value,
}
fn empty_object() -> Value {
    json!({})
}
fn rpc_reply(result: Result<Value, RpcError>) -> Json<Value> {
    match result {
        Ok(v) => Json(json!({"result":v})),
        Err(e) => Json(json!({"error":e})),
    }
}
async fn rpc(State(s): State<Arc<AppState>>, Json(mut call): Json<RpcCall>) -> Json<Value> {
    let c = s.codex.read().await.clone();
    let Some(c) = c else {
        return rpc_reply(Err(RpcError::new(-32001, "Codex is unavailable")));
    };
    if !c.ready.load(Ordering::SeqCst) {
        return rpc_reply(Err(RpcError::new(
            -32001,
            "Codex disconnected; reconnect from host settings",
        )));
    }
    if let Err(e) = policy::validate(&call.method, &call.params, &c, &s.events) {
        return rpc_reply(Err(e));
    }
    if call.method == "thread/list" && call.params.get("sourceKinds").is_none() {
        call.params["sourceKinds"] = json!(["cli", "vscode", "appServer", "exec"]);
    }
    // Inherited host policies are kept intact. No sandbox/approval overrides are injected.
    let result = c.request(&call.method, call.params).await;
    if matches!(call.method.as_str(), "thread/start" | "thread/fork")
        && let Ok(value) = &result
        && let Some(id) = value.pointer("/thread/id").and_then(Value::as_str)
        && let Err(e) = s.events.own(id)
    {
        return rpc_reply(Err(RpcError::new(
            -32603,
            format!("Thread created but ownership persistence failed: {e}"),
        )));
    }
    rpc_reply(result)
}
async fn requests(State(s): State<Arc<AppState>>) -> Json<Value> {
    let c = s.codex.read().await.clone();
    Json(if let Some(c) = c {
        c.pending_requests().await
    } else {
        json!([])
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    result: Value,
}
async fn respond(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(reply): Json<Reply>,
) -> Json<Value> {
    let c = s.codex.read().await.clone();
    rpc_reply(if let Some(c) = c {
        c.respond(&id, reply.result).await
    } else {
        Err(RpcError::new(-32001, "Codex unavailable"))
    })
}
#[derive(Deserialize)]
struct Cursor {
    #[serde(default)]
    after: i64,
    epoch: Option<String>,
}
async fn events(
    State(s): State<Arc<AppState>>,
    Query(cursor): Query<Cursor>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let _ = headers;
    let rx = s.events.sender.subscribe();
    let replay = match s.events.replay(cursor.after, cursor.epoch.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::CONFLICT, Json(json!({"error":e.to_string()}))).into_response();
        }
    };
    ws.max_message_size(1024)
        .on_upgrade(move |socket| stream(socket, rx, replay, cursor.after))
        .into_response()
}
async fn stream(
    mut socket: WebSocket,
    mut rx: broadcast::Receiver<Event>,
    replay: Vec<Event>,
    mut last: i64,
) {
    for event in replay {
        last = event.sequence;
        if socket
            .send(Message::Text(serde_json::to_string(&event).unwrap().into()))
            .await
            .is_err()
        {
            return;
        }
    }
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(20));
    loop {
        tokio::select! {
          message=socket.recv()=>{match message{Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,_=>{}}},
          result=rx.recv()=>{match result {
            Ok(event)=>{if event.sequence<=last{continue;}last=event.sequence;if socket.send(Message::Text(serde_json::to_string(&event).unwrap().into())).await.is_err(){break;}},
            Err(broadcast::error::RecvError::Lagged(_))=>{let _=socket.send(Message::Text(json!({"error":"resync_required"}).to_string().into())).await;break;},
            Err(_)=>break,
          }},
          _=heartbeat.tick()=>{if socket.send(Message::Ping(Vec::new().into())).await.is_err(){break;}}
        }
    }
}
async fn tunnel_status(State(s): State<Arc<AppState>>) -> Json<Value> {
    Json(s.tunnel.status().await)
}
async fn tunnel_start(State(s): State<Arc<AppState>>, Json(c): Json<TunnelConfig>) -> Response {
    match s.tunnel.start(c).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":e.to_string()})),
        )
            .into_response(),
    }
}
async fn tunnel_stop(State(s): State<Arc<AppState>>) -> Response {
    match s.tunnel.stop().await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct FileQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    root: usize,
}
async fn project_root(s: &AppState, id: &str, index: usize) -> anyhow::Result<std::path::PathBuf> {
    let c = s
        .codex
        .read()
        .await
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Codex unavailable"))?;
    let project = c
        .request("project/read", json!({"projectId":id}))
        .await
        .map_err(|e| anyhow::anyhow!(e.message))?;
    let path = project["project"]["roots"]
        .get(index)
        .and_then(|r| r["path"].as_str())
        .ok_or_else(|| anyhow::anyhow!("Project root not found"))?;
    Ok(std::path::PathBuf::from(path))
}
fn workspace_reply(result: anyhow::Result<Value>) -> Response {
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":e.to_string()})),
        )
            .into_response(),
    }
}
async fn project_files(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> Response {
    workspace_reply(
        async {
            let root = project_root(&s, &id, q.root).await?;
            crate::workspace::files(&root, &q.path).await
        }
        .await,
    )
}
async fn project_file(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> Response {
    workspace_reply(
        async {
            let root = project_root(&s, &id, q.root).await?;
            crate::workspace::file(&root, &q.path).await
        }
        .await,
    )
}
async fn project_git(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> Response {
    workspace_reply(
        async {
            let root = project_root(&s, &id, q.root).await?;
            crate::workspace::git_state(&root).await
        }
        .await,
    )
}
