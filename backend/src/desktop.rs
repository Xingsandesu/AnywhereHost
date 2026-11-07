use serde::Deserialize;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
#[cfg(test)]
use tokio::net::TcpStream;
use tokio::sync::Semaphore;

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
}

pub async fn status(s: &Arc<crate::api::AppState>) -> serde_json::Value {
    let enabled = s.desktop.enabled().await;
    let active = 2 - s.desktop.slots.available_permits();
    let available = native_executable().is_some();
    json!({"supported":true,"enabled":enabled,"reachable":enabled && available,"protocol":"avc-v1","engine":"VideoToolbox","activeSessions":active,"nativeProtocol":"avc-v1","nativeAvailable":available})
}

/// The GUI executable is selected locally by HostProcess, never supplied by a peer.
pub fn native_executable() -> Option<PathBuf> {
    std::env::var_os("ANYWHERE_DESKTOP_EXECUTABLE")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_file())
}

pub async fn native_session(
    state: &Arc<crate::api::AppState>,
    owner: &str,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
) -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    ensure!(state.desktop.enabled().await, "Desktop sharing disabled");
    let _permit = state.desktop.slots.clone().try_acquire_owned()?;
    #[cfg(test)]
    if let Some(target) = state.desktop.target {
        let mut fixture = TcpStream::connect(target).await?;
        anywhere_transport::write_json(send, &json!({"ready":true})).await?;
        let (mut read, mut write) = tokio::io::split(&mut fixture);
        tokio::select! {
            result = tokio::io::copy(recv, &mut write) => { result?; },
            result = tokio::io::copy(&mut read, send) => { result?; },
        }
        return Ok(());
    }
    let executable = native_executable().context("Native desktop helper unavailable")?;
    let mut child = tokio::process::Command::new(executable)
        .arg("--desktop-session")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut input = child.stdin.take().context("Desktop input unavailable")?;
    let mut output = child.stdout.take().context("Desktop output unavailable")?;
    anywhere_transport::write_json(send, &json!({"ready":true})).await?;
    // Either half closing ends the session. EOF gives the helper a chance to release keys.
    let result = tokio::select! {
        result = tokio::io::copy(recv, &mut input) => result.map(|_| ()),
        result = tokio::io::copy(&mut output, send) => result.map(|_| ()),
        _ = async {
            while state.security.active_owner(owner) && state.desktop.enabled().await {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        } => Ok(()),
    };
    drop(output);
    let _ = input.shutdown().await;
    drop(input);
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
    result?;
    Ok(())
}
