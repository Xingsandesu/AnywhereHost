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
        }
    }
    pub async fn enabled(&self) -> bool {
        let Ok(data) = tokio::fs::read(&self.path).await else {
            return false;
        };
        serde_json::from_slice::<Settings>(&data).is_ok_and(|v| v.enabled)
    }
}
async fn connect() -> std::io::Result<TcpStream> {
    tokio::time::timeout(
        Duration::from_secs(3),
        TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, 5900)),
    )
    .await?
}
pub async fn status(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let enabled = s.desktop.enabled().await;
    let reachable = enabled && connect().await.is_ok();
    Json(
        json!({"supported":true,"enabled":enabled,"reachable":reachable,"protocol":"rfb","engine":"noVNC","activeSessions":2-s.desktop.slots.available_permits()}),
    )
}
pub async fn socket(State(s): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
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
    let Ok(tcp) = connect().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"请在 Mac 系统设置中开启屏幕共享"})),
        )
            .into_response();
    };
    // A bounded native WSS stream preserves the existing Bearer/origin/pinning policy.
    ws.max_message_size(1024 * 1024)
        .max_frame_size(1024 * 1024)
        .on_upgrade(move |ws| async move {
            let _permit = permit;
            relay(ws, tcp, s).await;
        })
}
async fn relay(ws: WebSocket, tcp: TcpStream, state: Arc<AppState>) {
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
            if !state.desktop.enabled().await {
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
}
