use crate::config::private_write;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelConfig {
    pub server: String,
    pub server_port: u16,
    pub remote_port: u16,
    pub token: String,
    pub trusted_ca_file: Option<PathBuf>,
}
pub struct Tunnel {
    binary: PathBuf,
    dir: PathBuf,
    local_port: u16,
    child: Mutex<Option<Child>>,
}
impl Tunnel {
    pub fn new(binary: PathBuf, dir: PathBuf, local_port: u16) -> Self {
        Self {
            binary,
            dir,
            local_port,
            child: Mutex::new(None),
        }
    }
    pub async fn status(&self) -> Value {
        let mut slot = self.child.lock().await;
        let mut state = "stopped";
        let mut exit = None;
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(None) => state = "running",
                Ok(Some(code)) => {
                    state = "exited";
                    exit = code.code();
                }
                Err(_) => state = "unknown",
            }
        }
        json!({"state":state,"exitCode":exit,"bundled":self.binary.exists(),"note":"running 表示 frpc 进程存活；公网可达性需从 iPhone 验证。"})
    }
    pub async fn start(&self, config: TunnelConfig) -> Result<Value> {
        ensure!(
            !config.server.trim().is_empty() && !config.token.is_empty(),
            "隧道服务器和 token 不能为空"
        );
        ensure!(
            config.server_port > 0 && config.remote_port > 0,
            "端口必须大于 0"
        );
        let mut slot = self.child.lock().await;
        if let Some(child) = slot.as_mut() {
            ensure!(child.try_wait()?.is_some(), "隧道正在运行，请先停止");
        }
        ensure!(self.binary.is_file(), "app 内未找到 frpc，请运行打包脚本");
        let value = render(&config, self.local_port)?;
        let path = self.dir.join("frpc.toml");
        private_write(&path, value.as_bytes())?;
        let verified = tokio::time::timeout(
            Duration::from_secs(10),
            Command::new(&self.binary)
                .args(["verify", "-c"])
                .arg(&path)
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        ensure!(verified.status.success(), "frpc 配置验证失败");
        let log = self.dir.join("frpc.log");
        private_write(&log, b"")?;
        let log = std::fs::OpenOptions::new().append(true).open(log)?;
        let child = Command::new(&self.binary)
            .arg("-c")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .kill_on_drop(true)
            .spawn()
            .context("启动 frpc 失败")?;
        *slot = Some(child);
        Ok(json!({"state":"starting","remotePort":config.remote_port}))
    }
    pub async fn stop(&self) -> Result<Value> {
        if let Some(mut child) = self.child.lock().await.take()
            && child.try_wait()?.is_none()
        {
            child.kill().await?;
        }
        Ok(json!({"state":"stopped"}))
    }
}
fn render(config: &TunnelConfig, local_port: u16) -> Result<String> {
    let mut tls = toml::map::Map::new();
    tls.insert("enable".into(), true.into());
    if let Some(path) = &config.trusted_ca_file {
        ensure!(
            Path::new(path).is_absolute() && path.is_file(),
            "CA 文件必须是存在的绝对路径"
        );
        tls.insert(
            "trustedCaFile".into(),
            path.to_string_lossy().into_owned().into(),
        );
    } else {
        tls.insert("useSystemCa".into(), true.into());
    }
    let value = toml::Value::try_from(json!({
      "serverAddr":config.server,"serverPort":config.server_port,"loginFailExit":true,
      "auth":{"method":"token","token":config.token},
      "transport":{"tls":tls},
      "proxies":[{"name":"anywhere-host","type":"tcp","localIP":"127.0.0.1","localPort":local_port,"remotePort":config.remote_port}]
    }))?;
    Ok(toml::to_string_pretty(&value)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_cannot_inject_proxies() {
        let c = TunnelConfig {
            server: "host\n[[proxies]]".into(),
            server_port: 7000,
            remote_port: 9443,
            token: "secret\"\n".into(),
            trusted_ca_file: None,
        };
        let s = render(&c, 9443).unwrap();
        let v: toml::Value = toml::from_str(&s).unwrap();
        assert_eq!(v["proxies"].as_array().unwrap().len(), 1);
        assert_eq!(v["transport"]["tls"]["useSystemCa"].as_bool(), Some(true));
        assert_eq!(v["proxies"][0]["localIP"].as_str(), Some("127.0.0.1"));
    }
}
