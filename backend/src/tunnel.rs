use crate::config::private_write;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
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
    proxy_name: String,
    child: Mutex<Option<Child>>,
}
impl Tunnel {
    pub fn new(binary: PathBuf, dir: PathBuf, local_port: u16, host_fingerprint: &str) -> Self {
        Self {
            binary,
            dir,
            local_port,
            // The public TLS identity is stable across restarts and distinct per Host.
            // Do not expose usernames, paths or machine identifiers to the frps namespace.
            proxy_name: format!("anywhere-host-{host_fingerprint}"),
            child: Mutex::new(None),
        }
    }
    // Read the generated config so upgrades also recover previously entered settings.
    // The frps credential never leaves the private file through the status API.
    fn saved_config(&self) -> Option<TunnelConfig> {
        let text = std::fs::read_to_string(self.dir.join("frpc.toml")).ok()?;
        let value: toml::Value = toml::from_str(&text).ok()?;
        Some(TunnelConfig {
            server: value.get("serverAddr")?.as_str()?.into(),
            server_port: value.get("serverPort")?.as_integer()?.try_into().ok()?,
            remote_port: value
                .get("proxies")?
                .as_array()?
                .first()?
                .get("remotePort")?
                .as_integer()?
                .try_into()
                .ok()?,
            token: value.get("auth")?.get("token")?.as_str()?.into(),
            trusted_ca_file: value
                .get("transport")
                .and_then(|v| v.get("tls"))
                .and_then(|v| v.get("trustedCaFile"))
                .and_then(toml::Value::as_str)
                .map(PathBuf::from),
        })
    }
    fn log_tail(&self, token: &str) -> String {
        let Ok(mut file) = std::fs::File::open(self.dir.join("frpc.log")) else {
            return String::new();
        };
        let length = file.metadata().map(|m| m.len()).unwrap_or(0);
        // Do not load an unbounded log into memory or return credentials in diagnostics.
        let offset = length.saturating_sub(16_384 + token.len() as u64);
        let _ = file.seek(SeekFrom::Start(offset));
        let mut bytes = Vec::new();
        let _ = file.take(32_768).read_to_end(&mut bytes);
        let output = String::from_utf8_lossy(&bytes);
        let output = if offset > 0 {
            output.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
        } else {
            &output
        };
        diagnostic(output, token)
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
        let config = self.saved_config();
        let log = self.log_tail(config.as_ref().map(|c| c.token.as_str()).unwrap_or(""));
        let settings = config.map(|c| {
            json!({
                "server":c.server,"serverPort":c.server_port,"remotePort":c.remote_port,
                "trustedCaFile":c.trusted_ca_file,"hasToken":!c.token.is_empty()
            })
        });
        json!({"state":state,"exitCode":exit,"bundled":self.binary.exists(),"config":settings,"logTail":log,
            "note":"running 表示 frpc 进程存活；连接结果请查看日志，公网可达性需从远程设备验证。"})
    }
    pub async fn start(&self, mut config: TunnelConfig) -> Result<Value> {
        let mut slot = self.child.lock().await;
        if let Some(child) = slot.as_mut() {
            ensure!(child.try_wait()?.is_some(), "隧道正在运行，请先停止");
        }
        config.server = config.server.trim().to_owned();
        if config.token.is_empty()
            && let Some(saved) = self.saved_config()
            && saved.server == config.server
            && saved.server_port == config.server_port
        {
            config.token = saved.token;
        }
        ensure!(
            !config.server.is_empty() && !config.token.is_empty(),
            "隧道服务器和 token 不能为空；更换 frps 地址或控制端口后需重新输入 token"
        );
        ensure!(
            config.server_port > 0 && config.remote_port > 0,
            "端口必须大于 0"
        );
        ensure!(
            config.server_port != config.remote_port,
            "frps 控制端口（serverPort）和 Host 映射端口（remotePort）必须不同"
        );
        ensure!(self.binary.is_file(), "app 内未找到 frpc，请运行打包脚本");
        let value = render(&config, self.local_port, &self.proxy_name)?;
        let path = self.dir.join("frpc.toml");
        private_write(&path, value.as_bytes())?;
        let log_path = self.dir.join("frpc.log");
        private_write(&log_path, b"")?;
        let verification = tokio::time::timeout(
            Duration::from_secs(10),
            Command::new(&self.binary)
                .args(["verify", "-c"])
                .arg(&path)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("frpc 配置验证超时")?
        .context("无法运行 frpc 配置验证")?;
        if !verification.status.success() {
            let details = diagnostic(
                &format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&verification.stdout),
                    String::from_utf8_lossy(&verification.stderr)
                ),
                &config.token,
            );
            private_write(&log_path, details.as_bytes())?;
            anyhow::bail!("frpc 配置验证失败：{details}");
        }
        let log = std::fs::OpenOptions::new().append(true).open(log_path)?;
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
        // Surface immediate authentication/transport failures in the start response.
        // A living process is not evidence that the public proxy is reachable.
        tokio::time::sleep(Duration::from_millis(800)).await;
        if let Some(code) = slot.as_mut().context("frpc 进程丢失")?.try_wait()? {
            anyhow::bail!("frpc 已退出（{code}）：{}", self.log_tail(&config.token));
        }
        if let Err(error) = self.set_resume(true) {
            if let Some(mut child) = slot.take() {
                let _ = child.kill().await;
            }
            return Err(error);
        }
        Ok(json!({"state":"starting","remotePort":config.remote_port}))
    }
    fn set_resume(&self, enabled: bool) -> Result<()> {
        private_write(
            &self.dir.join("tunnel-resume.json"),
            if enabled { b"true" } else { b"false" },
        )
    }
    fn should_resume(&self) -> bool {
        std::fs::read(self.dir.join("tunnel-resume.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<bool>(&bytes).ok())
            .unwrap_or(false)
    }
    pub async fn restore(&self) {
        if self.should_resume()
            && let Some(config) = self.saved_config()
            && self.start(config).await.is_err()
        {
            eprintln!("Saved tunnel could not resume; check tunnel diagnostics");
        }
    }
    pub async fn stop(&self) -> Result<Value> {
        self.set_resume(false)?;
        self.shutdown().await
    }
    // App quit / OTA restart preserves the user's intent; explicit Stop clears it.
    pub async fn shutdown(&self) -> Result<Value> {
        if let Some(mut child) = self.child.lock().await.take()
            && child.try_wait()?.is_none()
        {
            child.kill().await?;
        }
        Ok(json!({"state":"stopped"}))
    }
}
fn diagnostic(output: &str, token: &str) -> String {
    let text = if token.is_empty() {
        output.to_owned()
    } else {
        output.replace(token, "<redacted>")
    };
    let lines: Vec<_> = text.lines().collect();
    lines[lines.len().saturating_sub(8)..]
        .join("\n")
        .chars()
        .take(8000)
        .collect()
}
fn render(config: &TunnelConfig, local_port: u16, proxy_name: &str) -> Result<String> {
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
    }
    // frpc 0.71.1 has no useSystemCa field. No CA uses standard frp TLS:
    // encryption without frps identity verification. Host HTTPS remains pinned.
    let value = toml::Value::try_from(json!({
      "serverAddr":config.server,"serverPort":config.server_port,"loginFailExit":true,
      "auth":{"method":"token","token":config.token},
      "transport":{"tls":tls},"log":{"disablePrintColor":true},
      "proxies":[{"name":proxy_name,"type":"tcp","localIP":"127.0.0.1","localPort":local_port,"remotePort":config.remote_port}]
    }))?;
    Ok(toml::to_string_pretty(&value)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn restart_preserves_intent_but_explicit_stop_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let tunnel = Tunnel::new(
            dir.path().join("unused"),
            dir.path().to_owned(),
            9443,
            "fixture",
        );
        assert!(!tunnel.should_resume());
        tunnel.set_resume(true).unwrap();
        tunnel.shutdown().await.unwrap();
        assert!(tunnel.should_resume());
        tunnel.stop().await.unwrap();
        assert!(!tunnel.should_resume());
        std::fs::write(dir.path().join("tunnel-resume.json"), "broken").unwrap();
        assert!(!tunnel.should_resume());
    }
    #[test]
    fn configuration_cannot_inject_proxies() {
        let c = TunnelConfig {
            server: "host\n[[proxies]]".into(),
            server_port: 7000,
            remote_port: 9443,
            token: "secret\"\n".into(),
            trusted_ca_file: None,
        };
        let s = render(&c, 9443, "anywhere-host-fixture").unwrap();
        let v: toml::Value = toml::from_str(&s).unwrap();
        assert_eq!(v["proxies"].as_array().unwrap().len(), 1);
        assert_eq!(v["transport"]["tls"]["enable"].as_bool(), Some(true));
        assert!(v["transport"]["tls"].get("useSystemCa").is_none());
        assert_eq!(v["proxies"][0]["localIP"].as_str(), Some("127.0.0.1"));
        assert_eq!(
            v["proxies"][0]["name"].as_str(),
            Some("anywhere-host-fixture")
        );
    }
    #[test]
    fn diagnostic_redacts_before_truncating() {
        let secret = "private-token";
        let input = format!("{}{}", "a".repeat(7998), secret);
        let result = diagnostic(&input, secret);
        assert!(!result.contains("private"));
        assert!(result.chars().count() <= 8000);
    }
}
