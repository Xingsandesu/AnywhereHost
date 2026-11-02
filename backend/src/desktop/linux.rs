//! Desktop sharing helpers are local children, reachable only through a private Unix socket.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Stdio};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

pub struct Sharing {
    directory: PathBuf,
    helper: Option<PathBuf>,
    state: Mutex<Process>,
    stopping: std::sync::atomic::AtomicBool,
}
#[derive(Default)]
struct Process {
    child: Option<Child>,
    attempted: bool,
    error: Option<String>,
}
impl Sharing {
    pub fn new(directory: PathBuf) -> Self {
        let helper_name = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            Some("w0vncserver")
        } else if std::env::var_os("DISPLAY").is_some() {
            Some("x0vncserver")
        } else {
            None
        };
        let helper = helper_name.and_then(|name| {
            let path = std::env::current_exe().ok()?.parent()?.join(name);
            path.metadata()
                .ok()
                .filter(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)?;
            Some(path)
        });
        Self {
            directory,
            helper,
            state: Mutex::new(Process::default()),
            stopping: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn supported(&self) -> bool {
        self.helper.is_some()
    }
    pub fn socket(&self) -> PathBuf {
        self.directory.join("desktop.sock")
    }
    pub async fn shutdown(&self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.maintain(false).await;
    }
    pub async fn maintain(&self, enabled: bool) {
        let mut state = self.state.lock().await;
        if enabled && self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if !enabled {
            if let Some(mut child) = state.child.take() {
                let _ = child.kill().await;
                let _ = tokio::fs::remove_file(self.socket()).await;
            }
            state.attempted = false;
            state.error = None;
            return;
        }
        if let Some(child) = state.child.as_mut() {
            match child.try_wait() {
                Ok(Some(code)) => {
                    state.error = Some(format!(
                        "Desktop sharing exited ({code}); check desktop.log and re-enable sharing"
                    ))
                }
                Err(error) => state.error = Some(error.to_string()),
                Ok(None) => {}
            }
            return;
        }
        if state.attempted {
            return;
        }
        state.attempted = true;
        match self.start().await {
            Ok(child) => state.child = Some(child),
            Err(error) => state.error = Some(error.to_string()),
        }
    }
    async fn start(&self) -> Result<Child> {
        let helper = self
            .helper
            .as_ref()
            .context("No desktop session or bundled sharing helper")?;
        let password = self.directory.join("desktop.passwd");
        let metadata = std::fs::symlink_metadata(&password)
            .context("Set a desktop password in the local Host UI first")?;
        ensure!(
            metadata.is_file() && metadata.len() == 8 && metadata.permissions().mode() & 0o077 == 0,
            "Desktop password file must be private (0600), with exactly one VNC password"
        );
        // The state directory is private and the backend holds its exclusive instance lock.
        if self.socket().exists() {
            ensure!(
                tokio::net::UnixStream::connect(self.socket())
                    .await
                    .is_err(),
                "Desktop socket is already in use"
            );
            std::fs::remove_file(self.socket())?;
        }
        let config = self.directory.join("desktop-config");
        std::fs::create_dir_all(&config)?;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o700))?;
        let log_path = self.directory.join("desktop.log");
        crate::config::private_write(&log_path, b"")?;
        let log = std::fs::OpenOptions::new().append(true).open(log_path)?;
        let mut command = Command::new(helper);
        command
            .args(["-rfbport", "-1", "-rfbunixpath"])
            .arg(self.socket())
            .args([
                "-rfbunixmode",
                "0600",
                "-SecurityTypes",
                "VncAuth",
                "-PasswordFile",
            ])
            .arg(password)
            .args([
                "-AlwaysShared",
                "-DisconnectClients=0",
                "-desktop",
                "AnywhereHost",
            ])
            .env("XDG_CONFIG_HOME", config)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .kill_on_drop(true);
        if helper.file_name().is_some_and(|name| name == "w0vncserver") {
            command.args(["-RememberDisplayChoice", "Always"]);
        } else {
            command.args(["-AcceptCutText=0", "-SendCutText=0"]);
        }
        command.spawn().context("Could not start desktop sharing")
    }
    pub async fn status(&self) -> Value {
        let state = self.state.lock().await;
        json!({"helper":self.helper.as_ref().and_then(|p|p.file_name()).map(|p|p.to_string_lossy()),
            "error":state.error,"started":state.child.is_some(),
            "authorization":"Wayland screen and input access require the local desktop portal's consent"})
    }
}
