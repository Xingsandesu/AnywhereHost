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
    http::{HeaderMap, StatusCode},
    middleware::{self},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};

use tokio::sync::{Mutex, RwLock, broadcast};
use tower_http::compression::CompressionLayer;

pub struct AppState {
    pub security: crate::security::Security,
    pub desktop: crate::desktop::Desktop,
    pub token: String,
    pub fingerprint: String,
    pub config: Config,
    pub codex: RwLock<Option<Arc<Codex>>>,
    pub runtime_error: RwLock<Option<String>>,
    pub connecting: Mutex<()>,
    pub events: Arc<Events>,
    pub apns: Option<Arc<crate::apns::Client>>,
    pub apns_error: Option<String>,
    pub tunnel: Tunnel,
}
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/v1/security/challenge",
            get(|| async { StatusCode::UNAUTHORIZED }),
        )
        .route(
            "/v1/security/pair",
            post(|| async { StatusCode::UNAUTHORIZED }),
        )
        .route("/v1/security/ticket", post(crate::security::ticket))
        .route("/v1/security/devices", get(crate::security::devices))
        .route("/v1/security/devices/revoke", post(crate::security::revoke))
        .route("/v1/status", get(status))
        .route("/v1/desktop", get(crate::desktop::status))
        .route("/v1/desktop/socket", get(crate::desktop::socket))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/rpc", post(rpc))
        .route("/v1/events", get(events))
        .route("/v1/requests", get(requests))
        .route("/v1/live-activities", post(register_activity))
        .route("/v1/live-activities/remove", post(remove_activity))
        .route("/v1/live-activities/status", get(activity_status))
        .route("/v1/threads/{id}/access", get(thread_access))
        .route("/v1/requests/{id}/respond", post(respond))
        .route("/v1/projects/{id}/files", get(project_files))
        .route("/v1/projects/{id}/file", get(project_file))
        .route("/v1/projects/{id}/image", get(project_image))
        .route("/v1/projects/{id}/git", get(project_git))
        .route("/v1/runtime/connect", post(connect))
        .route("/v1/tunnel", get(tunnel_status).post(tunnel_start))
        .route("/v1/tunnel/stop", post(tunnel_stop))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::security::guard,
        ))
        .with_state(state)
}
async fn status(State(s): State<Arc<AppState>>) -> Json<Value> {
    let c = s.codex.read().await;
    Json(
        json!({"name":"AnywhereHost","version":env!("CARGO_PKG_VERSION"),"protocolVersion":2,"connected":c.as_ref().is_some_and(|c|c.ready.load(Ordering::SeqCst)),"runtime":c.as_ref().map(|c|&c.runtime),"error":s.runtime_error.read().await.clone(),"eventEpoch":s.events.epoch,"lastEventSequence":s.events.last_sequence(),"certificateSHA256":s.fingerprint,"listen":s.config.listen.to_string()}),
    )
}
async fn capabilities(State(s): State<Arc<AppState>>) -> Json<Value> {
    let c = s.codex.read().await;
    let methods: Vec<_> = policy::METHODS
        .iter()
        .filter(|m| c.as_ref().is_some_and(|c| c.runtime.methods.contains(**m)))
        .collect();
    Json(
        json!({"protocolVersion":2,"images":{"projectRead":true,"maxBytes":12582912},"remoteDesktop":{"supported":true,"protocol":"rfb","transport":"native-wss"},"methods":methods,"events":{"replay":true,"retained":5000,"epoch":s.events.epoch,"textDeltaOffsets":"utf8"},"liveActivityPush":{"enabled":s.apns.is_some(),"environments":s.apns.as_ref().map(|c| c.environments()).unwrap_or_default()},"history":{"itemPagination":true,"defaultItemLimit":60,"maxItemLimit":100},"approvals":true,"threadAccess":true,"tlsPinning":"sha256-leaf-certificate","sharedProcessControl":c.as_ref().is_some_and(|c|c.runtime.shares_live_process),"desktopAppAttachment":"unverified","foreignThreadMutation":"requires_shared_process"}),
    )
}
async fn activity_status(State(s): State<Arc<AppState>>) -> Json<Value> {
    Json(
        json!({"enabled":s.apns.is_some(),"environments":s.apns.as_ref().map(|c|c.environments()).unwrap_or_default(),
        "error":s.apns_error,"delivery":s.events.activity_status()}),
    )
}
async fn register_activity(
    State(s): State<Arc<AppState>>,
    Json(registration): Json<crate::live_activity::Registration>,
) -> Response {
    if let Err(error) = registration.validate() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":error.to_string()})),
        )
            .into_response();
    }
    let Some(client) = &s.apns else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"APNs is not configured on this Host"})),
        )
            .into_response();
    };
    if !client.supports(registration.environment) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"APNs environment is not configured on this Host"})),
        )
            .into_response();
    }
    if !s
        .events
        .has_activity(&registration.thread_id, &registration.turn_id)
    {
        let Some(codex) = s.codex.read().await.clone() else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"Codex is not connected"})),
            )
                .into_response();
        };
        let checkpoint = s.events.last_sequence();
        match codex.request("thread/turns/list", json!({"threadId":registration.thread_id,"limit":20,"itemsView":"full","sortDirection":"desc"})).await {
        Ok(turns) => {
            let pending = codex.pending_requests().await;
            if s.events.observe_activity_snapshot(&registration.thread_id,&turns,&pending,checkpoint).is_err() {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"Could not persist activity snapshot"}))).into_response()
            }
        }
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":"Activity history is not available; retry registration"}))).into_response(),
        }
    }
    match s.events.register_activity(&registration) {
        Ok(()) => Json(json!({"registered":true})).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}
async fn remove_activity(
    State(s): State<Arc<AppState>>,
    Json(removal): Json<crate::live_activity::Removal>,
) -> Response {
    match s.events.remove_activity(&removal) {
        Ok(()) => Json(json!({"removed":true})).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Could not remove activity registration"})),
        )
            .into_response(),
    }
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
    crate::history::bound_page(&call.method, &mut call.params);
    let result = c.request(&call.method, call.params.clone()).await;
    let result = crate::history::normalize(&call.method, &call.params, result);
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
    axum::Extension(principal): axum::Extension<crate::security::Principal>,
    ws: WebSocketUpgrade,
) -> Response {
    let _ = headers;
    let Ok(permit) = s.security.streams.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let rx = s.events.sender.subscribe();
    let replay = match s.events.replay(cursor.after, cursor.epoch.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::CONFLICT, Json(json!({"error":e.to_string()}))).into_response();
        }
    };
    ws.max_message_size(1024)
        .max_frame_size(1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            let revoked = async {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if !s.security.active(&principal) {
                        break;
                    }
                }
            };
            tokio::select! {
                _ = stream(socket, rx, replay, cursor.after, s.clone(), principal.clone()) => {},
                _ = revoked => {},
            }
        })
        .into_response()
}
async fn stream(
    mut socket: WebSocket,
    mut rx: broadcast::Receiver<Event>,
    replay: Vec<Event>,
    mut last: i64,
    state: Arc<AppState>,
    principal: crate::security::Principal,
) {
    for event in replay {
        if !state.security.active(&principal) {
            return;
        }
        last = event.sequence;
        if send_event(
            &mut socket,
            Message::Text(serde_json::to_string(&event).unwrap().into()),
        )
        .await
        .is_err()
        {
            return;
        }
    }
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(20));
    let mut revoked = tokio::time::interval(std::time::Duration::from_secs(1));
    let mut last_received = std::time::Instant::now();
    loop {
        tokio::select! {
          _=revoked.tick()=>{if !state.security.active(&principal) || last_received.elapsed().as_secs() > 90 {break;}},
          message=socket.recv()=>{match message{Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,Some(Ok(Message::Ping(_)|Message::Pong(_)))=>{last_received=std::time::Instant::now();},_=>break}},
          result=rx.recv()=>{match result {
            Ok(event)=>{if event.sequence<=last{continue;}last=event.sequence;if send_event(&mut socket, Message::Text(serde_json::to_string(&event).unwrap().into())).await.is_err(){break;}},
            Err(broadcast::error::RecvError::Lagged(_))=>{let _=send_event(&mut socket, Message::Text(json!({"error":"resync_required"}).to_string().into())).await;break;},
            Err(_)=>break,
          }},
          _=heartbeat.tick()=>{if send_event(&mut socket, Message::Ping(Vec::new().into())).await.is_err(){break;}}
        }
    }
}
async fn send_event(socket: &mut WebSocket, message: Message) -> Result<(), ()> {
    tokio::time::timeout(std::time::Duration::from_secs(10), socket.send(message))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
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
async fn project_image(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> Response {
    workspace_reply(
        async {
            let root = project_root(&s, &id, q.root).await?;
            crate::workspace::image(&root, &q.path).await
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
