use crate::api::AppState;
use anyhow::{Result, ensure};
use anywhere_transport::{Request, read_frame, write_json};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
static ACTIVE_STREAMS: Semaphore = Semaphore::const_new(64);

pub fn error(error: impl std::fmt::Display) -> Value {
    json!({"error":{"code":-32000,"message":error.to_string()}})
}
pub async fn execute(state: &Arc<AppState>, request: Request, owner: &str, local: bool) -> Value {
    if !local && !state.security.active_owner(owner) {
        return json!({"error":{"code":401,"message":"Device authorization required"}});
    }
    let read_only = matches!(
        request.method.as_str(),
        "host.status"
            | "host.capabilities"
            | "requests.list"
            | "thread.access"
            | "resource.files"
            | "resource.file"
            | "resource.image"
            | "resource.git"
            | "desktop.status"
            | "devices.list"
            | "activity.status"
    ) || (request.method == "codex.call"
        && crate::policy::read_only(request.params["method"].as_str().unwrap_or("")));
    if read_only {
        return crate::api::dispatch(state, &request.method, request.params, owner, local)
            .await
            .unwrap_or_else(error);
    }
    // Local service management must remain available even if the business journal
    // is exhausted or unavailable. These operations are idempotent or local-only.
    if local
        && matches!(
            request.method.as_str(),
            "pairing.create" | "devices.revoke" | "admission.install" | "runtime.connect"
        )
    {
        return crate::api::dispatch(state, &request.method, request.params, owner, true)
            .await
            .unwrap_or_else(error);
    }
    if let Err(e) = crate::api::validate_mutation(&request, local) {
        return error(e);
    }
    let state = state.clone();
    let dispatch_owner = owner.to_owned();
    state
        .operations
        .clone()
        .execute(owner, request, move |request| async move {
            crate::api::dispatch(
                &state,
                &request.method,
                request.params,
                &dispatch_owner,
                local,
            )
            .await
            .unwrap_or_else(error)
        })
        .await
}

pub async fn serve(state: Arc<AppState>) {
    let slots = Arc::new(Semaphore::new(64));
    while let Some(incoming) = state.endpoint.accept().await {
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            incoming.refuse();
            continue;
        };
        let state = state.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Ok(Ok(connection)) =
                tokio::time::timeout(Duration::from_secs(10), incoming).await
            {
                let _ = connection_loop(state, connection).await;
            }
        });
    }
}
async fn exchange_admission(
    state: &Arc<AppState>,
    connection: &iroh::endpoint::Connection,
) -> Result<u64> {
    let (mut send, mut recv) = connection.accept_bi().await?;
    let request: Request =
        serde_json::from_slice(&anywhere_transport::read_frame_limit(&mut recv, 8192).await?)?;
    ensure!(
        request.method == "admission.exchange",
        "Official app certificate required"
    );
    let token = request.params["token"].as_str().unwrap_or("");
    let peer = anywhere_transport::admission::verify(
        token,
        &connection.remote_id().to_string(),
        Some("client"),
    )?;
    let guard = state.admission.read().await;
    let own = guard.as_deref().unwrap_or("");
    let host =
        anywhere_transport::admission::verify(own, &state.endpoint.id().to_string(), Some("host"))?;
    write_json(&mut send, &json!({"token":own})).await?;
    send.finish()?;
    Ok(peer.exp.min(host.exp))
}
async fn connection_loop(
    state: Arc<AppState>,
    connection: iroh::endpoint::Connection,
) -> Result<()> {
    let _registration = state.connections.track(&connection);
    let owner = connection.remote_id().to_string();
    let mut admission_expires = u64::MAX;
    if state.config.relay_urls.is_empty() {
        admission_expires = tokio::time::timeout(
            Duration::from_secs(15),
            exchange_admission(&state, &connection),
        )
        .await??;
    }
    let admission_expires = Arc::new(std::sync::atomic::AtomicU64::new(admission_expires));
    let mut revocations = state.security.revocations.subscribe();
    let slots = Arc::new(Semaphore::new(16));
    let mut check = tokio::time::interval(Duration::from_secs(1));
    let mut authorized = state.security.active_owner(&owner);
    let opened = std::time::Instant::now();
    loop {
        tokio::select! {
            result=revocations.recv()=>{if result.as_ref().is_ok_and(|id|id==&owner) || result.is_err() {connection.close(1u32.into(),b"authorization changed");break;}},
            _=check.tick()=>{
                if anywhere_transport::admission::now()>=admission_expires.load(std::sync::atomic::Ordering::SeqCst) {connection.close(1u32.into(),b"admission expired");break;}
                let active=state.security.active_owner(&owner);
                if (authorized && !active)||(!active&&opened.elapsed()>Duration::from_secs(15)) {connection.close(1u32.into(),b"authorization required");break;}
                authorized=active;
            },
            stream=connection.accept_bi()=>{
                let (send,recv)=stream?;
                let Ok(permit)=slots.clone().try_acquire_owned() else {drop(send);drop(recv);continue};
                let state=state.clone();let owner=owner.clone();let admission_expires=admission_expires.clone();
                tokio::spawn(async move {let _permit=permit;let _=stream_request(state,owner,send,recv,admission_expires).await;});
            }
        }
    }
    Ok(())
}
async fn stream_request(
    state: Arc<AppState>,
    owner: String,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    admission_expires: Arc<std::sync::atomic::AtomicU64>,
) -> Result<()> {
    let _global = ACTIVE_STREAMS.try_acquire()?;
    let limit = if state.security.active_owner(&owner) {
        8 * 1024 * 1024
    } else {
        4096
    };
    let bytes = tokio::time::timeout(
        Duration::from_secs(10),
        anywhere_transport::read_frame_limit(&mut recv, limit),
    )
    .await??;
    let request: Request = serde_json::from_slice(&bytes)?;
    if request.method == "admission.renew" && state.config.relay_urls.is_empty() {
        let peer = anywhere_transport::admission::verify(
            request.params["token"].as_str().unwrap_or(""),
            &owner,
            Some("client"),
        )?;
        let guard = state.admission.read().await;
        let own = guard.as_deref().unwrap_or("");
        let host = anywhere_transport::admission::verify(
            own,
            &state.endpoint.id().to_string(),
            Some("host"),
        )?;
        admission_expires.store(peer.exp.min(host.exp), std::sync::atomic::Ordering::SeqCst);
        write_json(&mut send, &json!({"token":own})).await?;
        send.finish()?;
        return Ok(());
    }
    if request.method == "pairing.accept" {
        let result = state
            .security
            .pair(&owner, request.params["ticket"].as_str().unwrap_or(""));
        write_json(&mut send, &result.unwrap_or_else(error)).await?;
        send.finish()?;
        return Ok(());
    }
    if !state.security.active_owner(&owner) {
        write_json(
            &mut send,
            &json!({"error":{"code":401,"message":"Device authorization required"}}),
        )
        .await?;
        send.finish()?;
        return Ok(());
    }
    match request.method.as_str() {
        "events.subscribe" => events(state, &owner, &request.params, &mut send).await?,
        "desktop.open" => {
            ensure!(state.desktop.enabled().await, "Desktop sharing disabled");
            let _permit = state.desktop.slots.clone().try_acquire_owned()?;
            let mut desktop = state.desktop.connect().await?;
            write_json(&mut send, &json!({"ready":true})).await?;
            let (mut read, mut write) = tokio::io::split(&mut desktop);
            tokio::select! {
                result=async {tokio::try_join!(tokio::io::copy(&mut recv,&mut write),tokio::io::copy(&mut read,&mut send))?;Ok::<_,anyhow::Error>(())}=>{result?;},
                _=desktop_disabled(&state,&owner)=>{}
            }
        }
        _ => {
            let response = execute(&state, request, &owner, false).await;
            tokio::time::timeout(Duration::from_secs(30), write_json(&mut send, &response))
                .await??;
        }
    }
    send.finish()?;
    Ok(())
}
async fn desktop_disabled(state: &AppState, owner: &str) {
    while state.security.active_owner(owner) && state.desktop.enabled().await {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
async fn revoked(state: &AppState, owner: &str) {
    while state.security.active_owner(owner) {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
async fn events(
    state: Arc<AppState>,
    owner: &str,
    params: &Value,
    send: &mut iroh::endpoint::SendStream,
) -> Result<()> {
    let mut rx = state.events.sender.subscribe();
    let mut after = params["after"].as_i64().unwrap_or(0);
    let epoch = params["epoch"].as_str();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    loop {
        ensure!(state.security.active_owner(owner), "Device revoked");
        let page = match state.events.replay(after, epoch) {
            Ok(page) => page,
            Err(_) => {
                write_json(
                    send,
                    &json!({"error":{"code":409,"message":"resync_required"}}),
                )
                .await?;
                return Ok(());
            }
        };
        for event in &page {
            let wire = state.events.encode(event);
            if wire.resync {
                write_json(
                    send,
                    &json!({"error":{"code":409,"message":"resync_required"}}),
                )
                .await?;
                return Ok(());
            }
            tokio::time::timeout(
                Duration::from_secs(15),
                anywhere_transport::write_frame(send, wire.text.as_bytes()),
            )
            .await??;
            after = event.sequence;
        }
        if !page.is_empty() {
            continue;
        }
        tokio::select! {
            _=revoked(&state,owner)=>break,
            _=heartbeat.tick()=>{tokio::time::timeout(Duration::from_secs(15),write_json(send,&json!({"heartbeat":true}))).await??;},
            result=rx.recv()=>{if matches!(result,Err(tokio::sync::broadcast::error::RecvError::Closed)){break;}}
        }
    }
    Ok(())
}
pub async fn local(state: Arc<AppState>, path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    let listener = tokio::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    loop {
        let (mut stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let result = async {
                let bytes = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut stream))
                    .await??;
                let request = serde_json::from_slice(&bytes)?;
                let response = execute(&state, request, "local", true).await;
                write_json(&mut stream, &response).await
            }
            .await;
            if let Err(e) = result {
                let _ = write_json(&mut stream, &error(e)).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{Mutex, RwLock};
    #[tokio::test]
    async fn relay_pair_reconnect_replay_revoke_and_deduplicate() -> Result<()> {
        tokio::time::timeout(Duration::from_secs(40), async {
            let mut relay_config=iroh_relay::server::ServerConfig::default();
            relay_config.relay=Some(iroh_relay::server::RelayConfig::new(([127,0,0,1],0)));
            let relay=iroh_relay::server::Server::spawn(relay_config).await?;
            let urls=vec![format!("http://{}", relay.http_addr().unwrap())];
            let dir=tempfile::tempdir()?;
            std::fs::write(dir.path().join("desktop.json"),b"{\"enabled\":true}")?;
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let desktop_addr=listener.local_addr()?;
            let desktop_task=tokio::spawn(async move {
                use tokio::io::{AsyncReadExt,AsyncWriteExt};
                let (mut tcp,_)=listener.accept().await?;
                tcp.write_all(b"RFB 003.008\n").await?;
                let mut bytes=[0;4];tcp.read_exact(&mut bytes).await?;assert_eq!(&bytes,b"test");
                tcp.write_all(b"done").await?;
                Ok::<_,anyhow::Error>(())
            });

            let (mut config,key)=crate::config::prepare(dir.path())?;config.relay_urls=urls.clone();
            let endpoint=anywhere_transport::endpoint(key,&urls).await?;
            let state=Arc::new(AppState{admission:RwLock::new(None),connections:crate::connections::Connections::default(),security:crate::security::Security::open(dir.path().join("devices.json"))?,desktop:crate::desktop::Desktop::fixture(dir.path().join("desktop.json"),desktop_addr),config,codex:RwLock::new(None),runtime_error:RwLock::new(None),connecting:Mutex::new(()),events:Arc::new(crate::events::Events::open(&dir.path().join("events.sqlite"))?),notifications:None,notifications_error:None,endpoint,operations:Arc::new(crate::operations::Operations::open(&dir.path().join("ops.sqlite"))?)});
            let task=tokio::spawn(serve(state.clone()));
            let key=iroh::SecretKey::from_bytes(&anywhere_transport::new_secret());
            // No direct transports: traffic must traverse the real local relay.
            let client=iroh::Endpoint::builder(iroh::endpoint::presets::Minimal).clear_ip_transports().secret_key(key.clone()).relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from_iter([urls[0].parse::<iroh::RelayUrl>()?]))).bind().await?;
            state.endpoint.online().await;
            let addr=anywhere_transport::address(&state.endpoint.id().to_string(),&urls)?;
            let conn=client.connect(addr.clone(),anywhere_transport::ALPN).await?;
            let req=|method:&str,params:Value|Request{id:uuid::Uuid::new_v4().to_string(),method:method.into(),params};
            assert_eq!(anywhere_transport::call(&conn,&req("host.status",json!({}))).await?["error"]["code"],401);
            let ticket=state.security.ticket();
            let paired=anywhere_transport::call(&conn,&req("pairing.accept",json!({"ticket":ticket}))).await?;
            assert_eq!(paired["deviceId"],client.id().to_string());
            let status=anywhere_transport::call(&conn,&req("host.status",json!({}))).await?;assert_eq!(status["protocolVersion"],3);
            let devices=execute(&state,req("devices.list",json!({})),"local",true).await;
            let link=&devices["connections"][client.id().to_string()][0];
            assert_eq!(link["state"],"connected");
            assert_eq!(link["mode"],"relay");
            assert!(link["rttMs"].as_f64().is_some_and(|ms|ms>=0.0));
            assert_eq!(anywhere_transport::connection_status(&conn)["mode"],"relay");
            assert!(anywhere_transport::call(&conn,&req("devices.list",json!({}))).await?.get("error").is_some());
            let mutation=req("codex.call",json!({"method":"turn/start","params":{}}));
            let first=anywhere_transport::call(&conn,&mutation).await?;
            conn.close(0u32.into(),b"reconnect");
            let conn=client.connect(addr.clone(),anywhere_transport::ALPN).await?;
            assert_eq!(anywhere_transport::call(&conn,&mutation).await?,first);
            state.events.publish(json!({"method":"fixture"}))?;
            let (mut send,mut recv)=conn.open_bi().await?;
            write_json(&mut send,&req("events.subscribe",json!({"after":0,"epoch":state.events.epoch()}))).await?;
            let event:Value=serde_json::from_slice(&read_frame(&mut recv).await?)?;assert_eq!(event["sequence"],1);
            state.events.invalidate();
            state.events.recover()?;
            loop {
                let reset:Value=serde_json::from_slice(&read_frame(&mut recv).await?)?;
                if reset["heartbeat"] == true { continue; }
                assert_eq!(reset["error"]["code"],409);
                break;
            }
            let (mut desktop_send,mut desktop_recv)=conn.open_bi().await?;
            write_json(&mut desktop_send,&req("desktop.open",json!({}))).await?;
            let ready:Value=serde_json::from_slice(&read_frame(&mut desktop_recv).await?)?;assert_eq!(ready["ready"],true);
            let mut greeting=[0;12];desktop_recv.read_exact(&mut greeting).await?;assert_eq!(&greeting,b"RFB 003.008\n");
            desktop_send.write_all(b"test").await?;
            let mut reply=[0;4];desktop_recv.read_exact(&mut reply).await?;assert_eq!(&reply,b"done");
            desktop_task.await??;
            // Local security management works even when all new journal reservations fail.
            let journal=rusqlite::Connection::open(dir.path().join("ops.sqlite"))?;
            journal.execute_batch("CREATE TRIGGER fail_reserve BEFORE INSERT ON operations BEGIN SELECT RAISE(ABORT,'fixture storage failure'); END;")?;
            let revoked=execute(&state,req("devices.revoke",json!({"deviceId":client.id().to_string()})),"local",true).await;
            assert_eq!(revoked["revoked"],true);
            tokio::time::timeout(Duration::from_secs(3),conn.closed()).await?;
            assert_eq!(anywhere_transport::connection_status(&conn)["state"],"disconnected");
            assert!(state.security.devices()["devices"].as_array().unwrap().is_empty());
            let conn=client.connect(addr,anywhere_transport::ALPN).await?;
            assert!(anywhere_transport::call(&conn,&req("pairing.accept",json!({"ticket":ticket}))).await?.get("error").is_some());
            client.close().await;state.endpoint.close().await;task.abort();relay.shutdown().await?;
            Ok::<_,anyhow::Error>(())
        }).await??;
        Ok(())
    }
}
