//! Blocking C boundary, called only on background queues. Handles own their endpoint and streams.
use crate::*;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde_json::json;
use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::Mutex as AsyncMutex;
struct Client {
    endpoint: Endpoint,
    connection: iroh::endpoint::Connection,
    expires: Arc<AtomicU64>,
}
struct Stream {
    send: AsyncMutex<iroh::endpoint::SendStream>,
    recv: AsyncMutex<iroh::endpoint::RecvStream>,
    framed: bool,
    client: u64,
    cancelled: tokio_util::sync::CancellationToken,
}
#[derive(Default)]
struct Handles {
    clients: HashMap<u64, Arc<Client>>,
    streams: HashMap<u64, Arc<Stream>>,
}
static HANDLES: OnceLock<Mutex<Handles>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
fn handles() -> &'static Mutex<Handles> {
    HANDLES.get_or_init(Default::default)
}
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("transport runtime"))
}
fn id(v: &Value) -> Result<u64> {
    v["handle"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Missing handle"))
}
fn client(v: &Value) -> Result<Arc<Client>> {
    handles()
        .lock()
        .unwrap()
        .clients
        .get(&id(v)?)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Connection closed"))
}
fn stream(v: &Value) -> Result<Arc<Stream>> {
    handles()
        .lock()
        .unwrap()
        .streams
        .get(&id(v)?)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Stream closed"))
}
async fn operation(v: Value) -> Result<Value> {
    match v["op"].as_str().unwrap_or("") {
        "connect" => {
            let bytes: Vec<u8> = serde_json::from_value(v["secret"].clone())?;
            let secret: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid secret"))?;
            let token = v["admission"].as_str().unwrap_or("");
            let secret = SecretKey::from_bytes(&secret);
            let (endpoint, relays, official) = if token.is_empty() {
                let relays: Vec<String> = serde_json::from_value(v["relays"].clone())?;
                ensure!(
                    cfg!(debug_assertions)
                        && !relays.is_empty()
                        && relays.iter().all(|r| r.starts_with("http://127.0.0.1:")),
                    "Official app certificate required"
                );
                (endpoint(secret, &relays).await?, relays, false)
            } else {
                let c = admission::verify(token, &secret.public().to_string(), Some("client"))?;
                (
                    admission::endpoint(secret, token, "client").await?,
                    c.relays,
                    true,
                )
            };
            let established = async {
                let addr = address(v["hostId"].as_str().unwrap_or(""), &relays)?;
                let connection = tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    endpoint.connect(addr, ALPN),
                )
                .await??;
                let expires = Arc::new(AtomicU64::new(u64::MAX));
                if official {
                    let response = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        call(
                            &connection,
                            &Request {
                                id: "admission".into(),
                                method: "admission.exchange".into(),
                                params: json!({"token":token}),
                            },
                        ),
                    )
                    .await??;
                    let peer = admission::verify(
                        response["token"].as_str().unwrap_or(""),
                        &connection.remote_id().to_string(),
                        Some("host"),
                    )?;
                    let own = admission::verify(token, &endpoint.id().to_string(), Some("client"))?;
                    expires.store(peer.exp.min(own.exp), Ordering::SeqCst);
                }
                if let Some(ticket) = v["ticket"].as_str() {
                    let reply = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        call(
                            &connection,
                            &Request {
                                id: "pair".into(),
                                method: "pairing.accept".into(),
                                params: json!({"ticket":ticket}),
                            },
                        ),
                    )
                    .await??;
                    ensure!(reply.get("error").is_none(), "Pairing expired or rejected");
                }
                Ok::<_, anyhow::Error>((connection, expires))
            }
            .await;
            let (connection, expires) = match established {
                Ok(value) => value,
                Err(error) => {
                    endpoint.close().await;
                    return Err(error);
                }
            };
            if official {
                let deadline = expires.clone();
                let watch = connection.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _=watch.closed()=>break,
                            _=tokio::time::sleep(std::time::Duration::from_secs(1))=>{if admission::now()>=deadline.load(Ordering::SeqCst) {watch.close(1u32.into(),b"admission expired");break;}}
                        }
                    }
                });
            }
            let handle = NEXT.fetch_add(1, Ordering::Relaxed);
            handles().lock().unwrap().clients.insert(
                handle,
                Arc::new(Client {
                    endpoint,
                    connection,
                    expires,
                }),
            );
            Ok(json!({"handle":handle}))
        }
        "admission" => {
            let c = client(&v)?;
            let token = v["token"].as_str().unwrap_or("");
            admission::install(&c.endpoint, token, "client").await?;
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                call(
                    &c.connection,
                    &Request {
                        id: "admission-renew".into(),
                        method: "admission.renew".into(),
                        params: json!({"token":token}),
                    },
                ),
            )
            .await??;
            let peer = admission::verify(
                response["token"].as_str().unwrap_or(""),
                &c.connection.remote_id().to_string(),
                Some("host"),
            )?;
            let own = admission::verify(token, &c.endpoint.id().to_string(), Some("client"))?;
            c.expires.store(peer.exp.min(own.exp), Ordering::SeqCst);
            Ok(json!({}))
        }
        "status" => Ok(connection_status(&client(&v)?.connection)),
        "probe" => {
            let c = client(&v)?;
            // Local QUIC state can remain connected while the peer has already
            // timed out a suspended phone. Bound recovery by an actual round trip.
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                c.endpoint.network_change().await;
                call(
                    &c.connection,
                    &Request {
                        id: "foreground-probe".into(),
                        method: "host.status".into(),
                        params: json!({}),
                    },
                )
                .await
            })
            .await?
        }
        "call" => {
            let c = client(&v)?;
            let request = serde_json::from_value(v["request"].clone())?;
            tokio::time::timeout(
                std::time::Duration::from_secs(75),
                call(&c.connection, &request),
            )
            .await?
        }
        "open" => {
            let c = client(&v)?;
            let request: Request = serde_json::from_value(v["request"].clone())?;
            let (mut send, mut recv) = c.connection.open_bi().await?;
            write_json(&mut send, &request).await?;
            let framed = request.method == "events.subscribe";
            if !framed {
                let response: Value = serde_json::from_slice(&read_frame(&mut recv).await?)?;
                ensure!(response["ready"] == true, "Desktop unavailable");
            }
            let handle = NEXT.fetch_add(1, Ordering::Relaxed);
            handles().lock().unwrap().streams.insert(
                handle,
                Arc::new(Stream {
                    send: AsyncMutex::new(send),
                    recv: AsyncMutex::new(recv),
                    framed,
                    client: id(&v)?,
                    cancelled: tokio_util::sync::CancellationToken::new(),
                }),
            );
            Ok(json!({"handle":handle}))
        }
        "read" => {
            let stream = stream(&v)?;
            let mut recv = stream.recv.lock().await;
            tokio::select! {
                _=stream.cancelled.cancelled()=>anyhow::bail!("Stream cancelled"),
                result=async {
                    if stream.framed {
                        let bytes=read_frame(&mut *recv).await?;
                        Ok(serde_json::from_slice(&bytes)?)
                    } else {
                        let mut bytes=vec![0;65536];
                        let n=recv.read(&mut bytes).await?.ok_or_else(||anyhow::anyhow!("Desktop closed"))?;
                        bytes.truncate(n);Ok(json!({"dataBase64":B64.encode(bytes)}))
                    }
                }=>result
            }
        }

        "write" => {
            let stream = stream(&v)?;
            let bytes = B64.decode(v["dataBase64"].as_str().unwrap_or(""))?;
            ensure!(bytes.len() <= 1048576, "Desktop input too large");
            tokio::time::timeout(std::time::Duration::from_secs(15), async {
                stream.send.lock().await.write_all(&bytes).await
            })
            .await??;
            Ok(json!({}))
        }
        "streamClose" => {
            let stream = handles().lock().unwrap().streams.remove(&id(&v)?);
            if let Some(stream) = stream {
                stream.cancelled.cancel();
                let mut recv = stream.recv.lock().await;
                let _ = recv.stop(0u32.into());
                let mut send = stream.send.lock().await;
                let _ = send.reset(0u32.into());
            }
            Ok(json!({}))
        }
        "close" => {
            let key = id(&v)?;
            let c = {
                let mut h = handles().lock().unwrap();
                h.streams.retain(|_, s| {
                    if s.client == key {
                        s.cancelled.cancel();
                        false
                    } else {
                        true
                    }
                });
                h.clients.remove(&key)
            };
            if let Some(c) = c {
                c.connection.close(0u32.into(), b"closed");
                c.endpoint.close().await;
            }
            Ok(json!({}))
        }
        _ => anyhow::bail!("Unknown transport operation"),
    }
}
/// # Safety
/// `input` must be a valid NUL-terminated UTF-8 string for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anywhere_transport_call(input: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if input.is_null() {
            return Err(anyhow::anyhow!("Null input"));
        }
        let value: Value = serde_json::from_slice(unsafe { CStr::from_ptr(input) }.to_bytes())?;
        runtime().block_on(operation(value))
    });
    let response = match result {
        Ok(Ok(v)) => json!({"value":v}),
        Ok(Err(e)) => json!({"failure":e.to_string()}),
        Err(_) => json!({"failure":"Transport failed"}),
    };
    CString::new(response.to_string())
        .expect("JSON has no NUL")
        .into_raw()
}
/// # Safety
/// `value` must be returned by `anywhere_transport_call` and freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anywhere_transport_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}
