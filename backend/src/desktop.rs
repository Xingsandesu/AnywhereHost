use serde::Deserialize;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{net::TcpStream, sync::Semaphore};

pub struct Desktop {
    path: PathBuf,
    pub slots: Arc<Semaphore>,
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
    #[cfg(test)]
    pub fn fixture(path: PathBuf, target: std::net::SocketAddr) -> Self {
        let mut desktop = Self::new(path);
        desktop.target = Some(target);
        desktop
    }
    pub async fn enabled(&self) -> bool {
        let Ok(data) = tokio::fs::read(&self.path).await else {
            return false;
        };
        serde_json::from_slice::<Settings>(&data).is_ok_and(|v| v.enabled)
    }

    async fn reachable(&self) -> bool {
        self.connect().await.is_ok()
    }

    pub async fn connect(&self) -> std::io::Result<TcpStream> {
        let target = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 5900));
        #[cfg(test)]
        let target = self.target.unwrap_or(target);
        let stream =
            tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(target)).await??;
        stream.set_nodelay(true)?;
        Ok(stream)
    }
}

pub async fn status(s: &Arc<crate::api::AppState>) -> serde_json::Value {
    let enabled = s.desktop.enabled().await;
    let platform = json!({"helper":"macOS Screen Sharing"});
    // Do not create short-lived RFB viewers during an active session.
    let active = 2 - s.desktop.slots.available_permits();
    let reachable = enabled && (active > 0 || s.desktop.reachable().await);
    json!({"supported":true,"platform":platform,"reason":null,"enabled":enabled,"reachable":reachable,"protocol":"rfb","engine":"noVNC","activeSessions":active})
}
