//! Authenticated RFB tunnel to this Mac only. Local console owns the enable switch.
use crate::api::AppState;
use axum::{
    Json,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Semaphore,
};

#[cfg(target_os = "linux")]
mod linux;

#[cfg(any(not(target_os = "linux"), test))]
use tokio::net::TcpStream;

pub trait DesktopIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> DesktopIo for T {}

pub struct Desktop {
    path: PathBuf,
    #[cfg(target_os = "linux")]
    sharing: linux::Sharing,
    slots: Arc<Semaphore>,
    #[cfg(test)]
    target: Option<std::net::SocketAddr>,
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    enabled: bool,
}
impl Desktop {
    pub fn new(path: PathBuf) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            sharing: linux::Sharing::new(
                path.parent()
                    .expect("desktop state directory")
                    .to_path_buf(),
            ),
            path,
            slots: Arc::new(Semaphore::new(2)),
            #[cfg(test)]
            target: None,
        }
    }
    pub fn supported(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.sharing.supported()
        }
        #[cfg(not(target_os = "linux"))]
        {
            cfg!(target_os = "macos")
        }
    }
    pub async fn maintain(&self) {
        #[cfg(target_os = "linux")]
        self.sharing.maintain(self.enabled().await).await;
    }
    pub async fn shutdown(&self) {
        #[cfg(target_os = "linux")]
        self.sharing.shutdown().await;
    }
    pub async fn enabled(&self) -> bool {
        let Ok(data) = tokio::fs::read(&self.path).await else {
            return false;
        };
        serde_json::from_slice::<Settings>(&data).is_ok_and(|v| v.enabled)
    }
}
impl Desktop {
    async fn connect(&self) -> std::io::Result<Box<dyn DesktopIo>> {
        #[cfg(test)]
        if let Some(target) = self.target {
            let stream = TcpStream::connect(target).await?;
            stream.set_nodelay(true)?;
            return Ok(Box::new(stream));
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Box::new(
                tokio::time::timeout(
                    Duration::from_secs(3),
                    tokio::net::UnixStream::connect(self.sharing.socket()),
                )
                .await??,
            ))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let target = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 5900));
            let stream =
                tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(target)).await??;
            stream.set_nodelay(true)?;
            Ok(Box::new(stream))
        }
    }
}

pub async fn status(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let supported = s.desktop.supported();
    let enabled = s.desktop.enabled().await;
    #[cfg(target_os = "linux")]
    let platform = s.desktop.sharing.status().await;
    #[cfg(not(target_os = "linux"))]
    let platform = json!({"helper":"macOS Screen Sharing"});
    // Do not create short-lived RFB viewers during an active session.
    let active = 2 - s.desktop.slots.available_permits();
    let reachable = supported && enabled && (active > 0 || s.desktop.connect().await.is_ok());
    Json(
        json!({"supported":supported,"platform":platform,"reason":if supported { None } else { Some("Desktop session or sharing helper is unavailable") },"enabled":enabled,"reachable":reachable,"protocol":"rfb","engine":"noVNC","activeSessions":active}),
    )
}
pub async fn socket(
    State(s): State<Arc<AppState>>,
    axum::Extension(principal): axum::Extension<crate::security::Principal>,
    ws: WebSocketUpgrade,
) -> Response {
    // Transport tests use an isolated loopback peer without requiring a desktop session.
    #[cfg(test)]
    let supported = s.desktop.supported() || s.desktop.target.is_some();
    #[cfg(not(test))]
    let supported = s.desktop.supported();
    if !supported {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({"error":"Desktop session or sharing helper is unavailable"})),
        )
            .into_response();
    }
    if !s.desktop.enabled().await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"请先在本机 AnywhereHost 中启用远程桌面"})),
        )
            .into_response();
    }
    let Ok(permit) = s.desktop.slots.clone().try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"远程桌面连接数已达上限"})),
        )
            .into_response();
    };
    let Ok(tcp) = s.desktop.connect().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"本机屏幕共享不可连接，请检查共享服务和权限"})),
        )
            .into_response();
    };
    // A bounded native WSS stream preserves the device proof/origin/pinning policy.
    ws.max_message_size(1024 * 1024)
        .max_frame_size(1024 * 1024)
        .on_upgrade(move |ws| async move {
            let _permit = permit;
            relay(ws, tcp, s, principal).await;
        })
}
async fn relay(
    ws: WebSocket,
    tcp: Box<dyn DesktopIo>,
    state: Arc<AppState>,
    principal: crate::security::Principal,
) {
    let session = uuid::Uuid::new_v4();
    let started = std::time::Instant::now();
    let (mut output, mut input) = ws.split();
    let (mut reader, mut writer) = tokio::io::split(tcp);
    let mut uploaded = 0u64;
    let mut downloaded = 0u64;
    // Keep these futures alive across polls: cancelling a partially completed
    // write and then retrying can duplicate bytes in the RFB stream.
    let upload = async {
        loop {
            let message = match tokio::time::timeout(Duration::from_secs(90), input.next()).await {
                Err(_) => return "client_idle_timeout",
                Ok(None) => return "client_eof",
                Ok(Some(Err(_))) => return "client_read_error",
                Ok(Some(Ok(message))) => message,
            };
            match message {
                Message::Binary(data) => {
                    match tokio::time::timeout(Duration::from_secs(30), writer.write_all(&data))
                        .await
                    {
                        Err(_) => return "screen_write_timeout",
                        Ok(Err(_)) => return "screen_write_error",
                        Ok(Ok(())) => uploaded += data.len() as u64,
                    }
                }
                Message::Ping(_) | Message::Pong(_) => {}
                Message::Close(_) => return "client_close",
                _ => return "invalid_client_frame",
            }
        }
    };
    let download = async {
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            let n = match reader.read(&mut buffer).await {
                Ok(0) => return "screen_eof",
                Ok(n) => n,
                Err(_) => return "screen_read_error",
            };
            match tokio::time::timeout(
                Duration::from_secs(30),
                output.send(Message::Binary(buffer[..n].to_vec().into())),
            )
            .await
            {
                Err(_) => return "client_write_timeout",
                Ok(Err(_)) => return "client_write_error",
                Ok(Ok(())) => downloaded += n as u64,
            }
        }
    };
    let revoked = async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if !state.security.active(&principal) {
                return "device_revoked";
            }
            if !state.desktop.enabled().await {
                return "desktop_disabled";
            }
        }
    };
    eprintln!("desktop session={session} opened");
    let reason = tokio::select! { reason = upload => reason, reason = download => reason, reason = revoked => reason };
    // Metadata only: never log RFB data, keys, passwords or device identity.
    eprintln!(
        "desktop session={session} closed reason={reason} duration_ms={} uploaded={uploaded} downloaded={downloaded}",
        started.elapsed().as_millis()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn local_switch_fails_closed_and_can_be_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop.json");
        let desktop = Desktop::new(path.clone());
        assert!(!desktop.enabled().await);
        for data in [
            r#"{"enabled":true}"#,
            r#"{"enabled":false}"#,
            "broken",
            r#"{"enabled":true,"target":"remote:22"}"#,
        ] {
            std::fs::write(&path, data).unwrap();
            assert_eq!(desktop.enabled().await, data == r#"{"enabled":true}"#);
        }
    }
    #[tokio::test]
    async fn signed_device_relay_preserves_bytes_and_revocation_closes_it() {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        use ring::signature::KeyPair;
        use sha2::{Digest, Sha256};
        use tokio_tungstenite::tungstenite::{Message as Frame, client::IntoClientRequest};
        let dir = tempfile::tempdir().unwrap();
        let random = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &random,
        )
        .unwrap();
        let key = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8.as_ref(),
            &random,
        )
        .unwrap();
        let id = hex::encode(Sha256::digest(key.public_key().as_ref()));
        let registry = dir.path().join("devices.json");
        std::fs::write(&registry,json!({"devices":{&id:{"publicKey":B64.encode(key.public_key().as_ref())}},"ticket":null}).to_string()).unwrap();
        let settings = dir.path().join("desktop.json");
        std::fs::write(&settings, r#"{"enabled":true}"#).unwrap();
        let peer = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut desktop = Desktop::new(settings);
        desktop.target = Some(peer.local_addr().unwrap());
        let peer_task = tokio::spawn(async move {
            let (mut tcp, _) = peer.accept().await.unwrap();
            tcp.write_all(b"RFB 003.008\n").await.unwrap();
            let mut bytes = vec![0; 1024];
            while let Ok(n) = tcp.read(&mut bytes).await {
                if n == 0 {
                    break;
                }
                if tcp.write_all(&bytes[..n]).await.is_err() {
                    break;
                }
            }
        });
        let state = Arc::new(AppState {
            desktop,
            security: crate::security::Security::open(registry).unwrap(),
            token: "admin".into(),
            fingerprint: "host-pin".into(),
            config: crate::config::Config::default(),
            codex: tokio::sync::RwLock::new(None),
            runtime_error: tokio::sync::RwLock::new(None),
            connecting: tokio::sync::Mutex::new(()),
            events: Arc::new(
                crate::events::Events::open(&dir.path().join("events.sqlite")).unwrap(),
            ),
            apns: None,
            apns_error: None,
            tunnel: crate::tunnel::Tunnel::new(
                dir.path().join("unused"),
                dir.path().to_path_buf(),
                1,
            ),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = crate::api::router(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut request = format!("ws://{address}/v1/desktop/socket")
            .into_client_request()
            .unwrap();
        let identity = format!("Anywhere-Device {id}");
        let timestamp = crate::security::now().to_string();
        let nonce = "a".repeat(32);
        let digest = hex::encode(Sha256::digest([]));
        let canonical = format!(
            "Anywhere-v2\nhost-pin\n{identity}\nGET\n/v1/desktop/socket\n{digest}\n{timestamp}\n{nonce}\n{}",
            state.security.challenge
        );
        for (name, value) in [
            ("authorization", identity),
            ("x-anywhere-time", timestamp),
            ("x-anywhere-nonce", nonce),
            ("x-anywhere-content-sha256", digest),
            ("x-anywhere-challenge", state.security.challenge.clone()),
            (
                "x-anywhere-signature",
                B64.encode(key.sign(&random, canonical.as_bytes()).unwrap().as_ref()),
            ),
        ] {
            request.headers_mut().insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        let tcp = TcpStream::connect(address).await.unwrap();
        let (mut socket, _) = tokio_tungstenite::client_async(request, tcp).await.unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap().into_data(),
            b"RFB 003.008\n".as_slice()
        );
        socket
            .send(Frame::Binary(vec![0, 1, 2, 255].into()))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap().into_data(),
            [0, 1, 2, 255].as_slice()
        );
        // A quiet desktop must reply without waiting for the next screen update.
        socket
            .send(Frame::Ping(b"idle-heartbeat".to_vec().into()))
            .await
            .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("idle desktop heartbeat stalled")
            .unwrap()
            .unwrap();
        assert_eq!(reply, Frame::Pong(b"idle-heartbeat".to_vec().into()));
        // High-volume screen updates and drag input must preserve every byte.
        let payload: Vec<u8> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let (mut sender, mut receiver) = socket.split();
        let send = async {
            for chunk in payload.chunks(32 * 1024) {
                sender
                    .send(Frame::Binary(chunk.to_vec().into()))
                    .await
                    .unwrap();
            }
            sender
                .send(Frame::Ping(b"busy-heartbeat".to_vec().into()))
                .await
                .unwrap();
        };
        let receive = async {
            let mut received = Vec::new();
            let mut pong = false;
            while received.len() < payload.len() || !pong {
                match receiver.next().await.unwrap().unwrap() {
                    Frame::Binary(data) => received.extend_from_slice(&data),
                    Frame::Pong(data) => {
                        assert_eq!(data.as_ref(), b"busy-heartbeat");
                        pong = true;
                    }
                    other => panic!("unexpected frame {other:?}"),
                }
            }
            assert_eq!(received, payload);
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(send, receive);
        })
        .await
        .unwrap();
        let mut socket = sender.reunite(receiver).unwrap();
        let revocation = serde_json::from_value(json!({"deviceID":id})).unwrap();
        assert_eq!(
            crate::security::revoke(State(state), Json(revocation))
                .await
                .status(),
            StatusCode::OK
        );
        let closed = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap();
        assert!(matches!(
            closed,
            None | Some(Err(_)) | Some(Ok(Frame::Close(_)))
        ));
        server.abort();
        peer_task.abort();
    }
}
