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
    net::TcpStream,
    sync::Semaphore,
};

pub struct Desktop {
    path: PathBuf,
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
            path,
            slots: Arc::new(Semaphore::new(2)),
            #[cfg(test)]
            target: None,
        }
    }
    pub async fn enabled(&self) -> bool {
        let Ok(data) = tokio::fs::read(&self.path).await else {
            return false;
        };
        serde_json::from_slice::<Settings>(&data).is_ok_and(|v| v.enabled)
    }
}
impl Desktop {
    async fn connect(&self) -> std::io::Result<TcpStream> {
        let target = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 5900));
        #[cfg(test)]
        let target = self.target.unwrap_or(target);
        tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(target)).await?
    }
}
pub async fn status(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let enabled = s.desktop.enabled().await;
    let reachable = enabled && s.desktop.connect().await.is_ok();
    Json(
        json!({"supported":true,"enabled":enabled,"reachable":reachable,"protocol":"rfb","engine":"noVNC","activeSessions":2-s.desktop.slots.available_permits()}),
    )
}
pub async fn socket(
    State(s): State<Arc<AppState>>,
    axum::Extension(principal): axum::Extension<crate::security::Principal>,
    ws: WebSocketUpgrade,
) -> Response {
    if !s.desktop.enabled().await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"请先在 Mac 的 AnywhereHost 中启用远程桌面"})),
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
            Json(json!({"error":"请在 Mac 系统设置中开启屏幕共享"})),
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
    tcp: TcpStream,
    state: Arc<AppState>,
    principal: crate::security::Principal,
) {
    let (mut output, mut input) = ws.split();
    let (mut reader, mut writer) = tcp.into_split();
    let upload = async {
        while let Ok(Some(Ok(message))) =
            tokio::time::timeout(Duration::from_secs(90), input.next()).await
        {
            match message {
                Message::Binary(data) => {
                    if !matches!(
                        tokio::time::timeout(Duration::from_secs(15), writer.write_all(&data))
                            .await,
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                }
                Message::Ping(_) | Message::Pong(_) => {}
                _ => break,
            }
        }
    };
    let download = async {
        let mut buffer = vec![0u8; 64 * 1024];
        while let Ok(n) = reader.read(&mut buffer).await {
            if n == 0 {
                break;
            }
            if !matches!(
                tokio::time::timeout(
                    Duration::from_secs(15),
                    output.send(Message::Binary(buffer[..n].to_vec().into()))
                )
                .await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
    };
    let revoked = async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if !state.security.active(&principal) || !state.desktop.enabled().await {
                break;
            }
        }
    };
    tokio::select! { _ = upload => {}, _ = download => {}, _ = revoked => {} }
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
