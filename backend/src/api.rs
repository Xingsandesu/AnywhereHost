use crate::{
    codex::{Codex, RpcError},
    config::Config,
    events::Events,
    policy,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use tokio::sync::{Mutex, RwLock};
pub struct AppState {
    pub admission: RwLock<Option<String>>,
    pub connections: crate::connections::Connections,
    pub security: crate::security::Security,
    pub desktop: crate::desktop::Desktop,
    pub config: Config,
    pub codex: RwLock<Option<Arc<Codex>>>,
    pub runtime_error: RwLock<Option<String>>,
    pub connecting: Mutex<()>,
    pub events: Arc<Events>,
    pub notifications: Option<Arc<crate::notifications::Client>>,
    pub notifications_error: Option<String>,
    pub endpoint: iroh::Endpoint,
    pub operations: Arc<crate::operations::Operations>,
}
pub async fn connect_runtime(s: &Arc<AppState>) -> std::result::Result<(), String> {
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcCall {
    method: String,
    #[serde(default)]
    params: Value,
}
fn rpc_reply(result: std::result::Result<Value, RpcError>) -> Value {
    match result {
        Ok(value) => json!({"result":value}),
        Err(error) => json!({"error":error}),
    }
}
async fn rpc(s: &Arc<AppState>, mut call: RpcCall) -> Value {
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
    let history_context = if call.method == "thread/turns/list" {
        call.params.clone()
    } else {
        Value::Null
    };
    let result = c.request(&call.method, call.params).await;
    let result = crate::history::normalize(&call.method, &history_context, result);
    if matches!(call.method.as_str(), "thread/start" | "thread/fork")
        && let Ok(value) = &result
        && let Some(id) = value.pointer("/thread/id").and_then(Value::as_str)
    {
        // The native side has committed creation. Retain its result until ownership
        // is durable; a transient storage error must not orphan a real thread.
        while s.events.own(id).is_err() {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    rpc_reply(result)
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

pub fn validate_mutation(request: &anywhere_transport::Request, local: bool) -> Result<()> {
    match request.method.as_str() {
        "codex.call" => {
            let call: RpcCall = serde_json::from_value(request.params.clone())?;
            anyhow::ensure!(
                policy::METHODS.contains(&call.method.as_str()) && call.params.is_object(),
                "Unsupported Codex method or invalid params"
            );
        }
        "requests.respond" | "runtime.connect" | "activity.remove" | "activity.grant"
        | "activity.register" => {}
        "pairing.create" | "devices.revoke" | "admission.install" if local => {}
        method => bail!("Unsupported method: {method}"),
    }
    Ok(())
}

pub async fn dispatch(
    s: &Arc<AppState>,
    method: &str,
    p: Value,
    owner: &str,
    local: bool,
) -> Result<Value> {
    let result = match method {
        "host.status" => {
            let c = s.codex.read().await;
            json!({"name":"AnywhereHost","version":env!("CARGO_PKG_VERSION"),"build":env!("ANYWHERE_BUILD").parse::<u64>().expect("valid build number"),"protocolVersion":3,"connected":c.as_ref().is_some_and(|c|c.ready.load(Ordering::SeqCst)),"runtime":c.as_ref().map(|c|&c.runtime),"error":s.runtime_error.read().await.clone(),"eventEpoch":s.events.epoch(),"lastEventSequence":s.events.last_sequence(),"hostId":s.endpoint.id().to_string(),"relayUrls":s.config.relay_urls})
        }
        "admission.install" if local => {
            let token = p["token"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing admission certificate"))?;
            let claims = anywhere_transport::admission::install(&s.endpoint, token, "host").await?;
            tokio::time::timeout(std::time::Duration::from_secs(20), s.endpoint.online())
                .await
                .map_err(|_| anyhow::anyhow!("官方网络接入超时，请检查网络后重试"))?;
            *s.admission.write().await = Some(token.to_owned());
            json!({"expiresAt":claims.exp})
        }
        "host.capabilities" => {
            let c = s.codex.read().await;
            let methods: Vec<_> = policy::METHODS
                .iter()
                .filter(|m| c.as_ref().is_some_and(|c| c.runtime.methods.contains(**m)))
                .collect();
            json!({"protocolVersion":3,"methods":methods,"images":{"projectRead":true,"maxBytes":12582912},"remoteDesktop":{"supported":true,"protocol":"rfb","transport":"iroh"},"events":{"replay":true,"epoch":s.events.epoch(),"textDeltaOffsets":"utf8"},"history":{"itemPagination":true},"approvals":true,"threadAccess":true,"liveActivityPush":notification_status(s).await})
        }
        "codex.call" => rpc(s, serde_json::from_value(p)?).await,
        "requests.list" => {
            if let Some(c) = s.codex.read().await.clone() {
                c.pending_requests().await
            } else {
                json!([])
            }
        }
        "requests.respond" => {
            let c = s
                .codex
                .read()
                .await
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Codex unavailable"))?;
            rpc_reply(
                c.respond(
                    p["requestId"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Missing requestId"))?,
                    p["result"].clone(),
                )
                .await,
            )
        }
        "thread.access" => {
            let c = s.codex.read().await;
            let shared = c.as_ref().is_some_and(|c| c.runtime.shares_live_process);
            let id = p["threadId"].as_str().unwrap_or("");
            json!({"threadId":id,"canMutate":c.as_ref().is_some_and(|c|c.ready.load(Ordering::SeqCst))&&(shared||s.events.owns(id)),"hostCreated":s.events.owns(id),"sharedProcess":shared})
        }
        "resource.files" | "resource.file" | "resource.image" | "resource.git" => {
            let root = project_root(
                s,
                p["projectId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing projectId"))?,
                p["root"].as_u64().unwrap_or(0) as usize,
            )
            .await?;
            let path = p["path"].as_str().unwrap_or("");
            match method {
                "resource.files" => crate::workspace::files(&root, path).await?,
                "resource.file" => crate::workspace::file(&root, path).await?,
                "resource.image" => crate::workspace::image(&root, path).await?,
                _ => crate::workspace::git_state(&root).await?,
            }
        }
        "desktop.status" => crate::desktop::status(s).await,
        "runtime.connect" => {
            connect_runtime(s).await.map_err(anyhow::Error::msg)?;
            json!({"connected":true})
        }
        "devices.list" if local => {
            let mut devices = s.security.devices();
            let connections: serde_json::Map<String, Value> = devices["devices"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .map(|id| (id.to_owned(), s.connections.for_device(id)))
                .collect();
            devices["connections"] = json!(connections);
            devices
        }
        "devices.revoke" if local => s.security.revoke(
            p["deviceId"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing deviceId"))?,
        )?,
        "pairing.create" if local => {
            if s.config.relay_urls.is_empty() {
                let guard = s.admission.read().await;
                anywhere_transport::admission::verify(
                    guard.as_deref().unwrap_or(""),
                    &s.endpoint.id().to_string(),
                    Some("host"),
                )?;
            }
            let ticket = s.security.ticket();
            json!({"hostId":s.endpoint.id().to_string(),"relayUrls":s.config.relay_urls,"ticket":ticket,"expiresIn":120})
        }
        "activity.status" => {
            let mut status = notification_status(s).await;
            status["delivery"] = s.events.activity_status();
            status
        }
        "activity.remove" => {
            s.events
                .remove_activity(&serde_json::from_value(p)?, owner)?;
            json!({"removed":true})
        }
        "activity.grant" => {
            let registration: crate::live_activity::Registration =
                serde_json::from_value(p["registration"].clone())?;
            registration.validate_context()?;
            anyhow::ensure!(
                registration.delivery_token.is_empty(),
                "Unexpected delivery token"
            );
            let client = s
                .notifications
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Notification service is unavailable"))?;
            client.grant(
                registration.binding(owner),
                registration.environment,
                p["keyId"].as_str().unwrap_or(""),
                &s.endpoint.id().to_string(),
                &registration,
                p["tokenDigest"].as_str().unwrap_or(""),
            )?
        }
        "activity.register" => {
            let registration: crate::live_activity::Registration = serde_json::from_value(p)?;
            registration.validate()?;
            let client = s
                .notifications
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Notification service is unavailable"))?;
            if !s
                .events
                .has_activity(&registration.thread_id, &registration.turn_id)
            {
                let c = s
                    .codex
                    .read()
                    .await
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("Codex unavailable"))?;
                let checkpoint = s.events.last_sequence();
                let turns=c.request("thread/turns/list",json!({"threadId":registration.thread_id,"limit":20,"itemsView":"full","sortDirection":"desc"})).await.map_err(|e|anyhow::anyhow!(e.message))?;
                s.events.observe_activity_snapshot(
                    &registration.thread_id,
                    &turns,
                    &c.pending_requests().await,
                    checkpoint,
                )?;
            }
            client
                .validate(
                    &registration.delivery_token,
                    registration.binding(owner),
                    registration.environment,
                )
                .await?;
            anyhow::ensure!(s.security.active_owner(owner), "Device revoked");
            s.events.register_activity(&registration, owner)?;
            json!({"registered":true})
        }
        _ => bail!("Unsupported method: {method}"),
    };
    Ok(result)
}
async fn notification_status(s: &AppState) -> Value {
    match &s.notifications {
        Some(client) => client.status().await,
        None => json!({"enabled":false,"environments":[],"error":s.notifications_error}),
    }
}
