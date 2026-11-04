//! Locate the runtime and derive capabilities from the executable that owns it.
use crate::config::Config;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
pub fn discover(config: &Config) -> Result<(PathBuf, bool)> {
    if let Some(path) = &config.codex_binary {
        ensure!(
            executable(path),
            "configured Codex binary is not executable"
        );
        return Ok((path.clone(), path.to_string_lossy().contains(".app/")));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let apps = [
        "/Applications/Codex.app/Contents/Resources/codex".to_string(),
        "/Applications/Codex.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
            .to_string(),
        "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
            .to_string(),
        format!("{home}/Applications/Codex.app/Contents/Resources/codex"),
        format!(
            "{home}/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
        ),
    ];
    for path in apps {
        let path = PathBuf::from(path);
        if executable(&path) {
            return Ok((path, true));
        }
    }
    let mut dirs: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    dirs.extend([
        PathBuf::from(&home).join(".local/bin"),
        PathBuf::from(&home).join(".cargo/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ]);
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    for dir in dirs {
        let p = dir.join("codex");
        if executable(&p) {
            return Ok((p, false));
        }
    }
    bail!("未找到主机 Codex。请先安装或指定 Codex App / CLI。")
}
pub async fn methods(binary: &Path) -> Result<HashSet<String>> {
    let dir = tempfile::tempdir()?;
    let mut cmd = Command::new(binary);
    cmd.args([
        "app-server",
        "generate-json-schema",
        "--experimental",
        "--out",
    ])
    .arg(dir.path())
    .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(20), cmd.output()).await??;
    ensure!(
        output.status.success(),
        "Codex protocol schema generation failed"
    );
    let doc: Value =
        serde_json::from_slice(&tokio::fs::read(dir.path().join("ClientRequest.json")).await?)?;
    Ok(doc["oneOf"]
        .as_array()
        .context("unknown Codex protocol schema")?
        .iter()
        .filter_map(|x| {
            x.pointer("/properties/method/enum/0")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect())
}
pub async fn login_environment() -> Result<HashMap<String, String>> {
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    ensure!(
        shell.is_absolute() && executable(&shell),
        "login shell is unavailable"
    );
    // This is a fixed environment probe, never built from remote input. Do not log its output.
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(shell)
            .args(["-l", "-c", "/usr/bin/env -0"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("login environment timed out")??;
    ensure!(
        output.status.success(),
        "could not load the user's login environment"
    );
    let mut env = HashMap::new();
    for entry in output.stdout.split(|b| *b == 0) {
        if let Ok(entry) = std::str::from_utf8(entry)
            && let Some((key, value)) = entry.split_once('=')
            && !key.is_empty()
            && key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            env.insert(key.to_owned(), value.to_owned());
        }
    }
    Ok(env)
}

pub fn peer_binary(stream: &tokio::net::UnixStream, home: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let peer = stream.peer_cred()?;
    ensure!(
        peer.uid() == std::fs::metadata(home)?.uid(),
        "control socket peer and Codex Home must have the same owner"
    );
    let pid = peer
        .pid()
        .context("Cannot identify the control socket process")?;
    let path = {
        use std::os::unix::ffi::OsStringExt;
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // proc_pidpath writes at most buffer.len() bytes and includes a trailing NUL.
        let length =
            unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        ensure!(
            length > 0,
            "Cannot resolve the control socket executable: {}",
            std::io::Error::last_os_error()
        );
        let end = buffer
            .iter()
            .position(|b| *b == 0)
            .context("Invalid executable path")?;
        buffer.truncate(end);
        PathBuf::from(std::ffi::OsString::from_vec(buffer))
    };
    ensure!(
        executable(&path),
        "The control socket executable is unavailable"
    );
    Ok(path)
}
pub async fn version(binary: &Path) -> Result<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(binary)
            .arg("--version")
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    ensure!(
        output.status.success(),
        "Cannot identify the control socket runtime version"
    );
    let text = std::str::from_utf8(&output.stdout)?.trim();
    Ok(text
        .strip_prefix("codex-cli ")
        .context("The socket peer is not a Codex runtime")?
        .to_owned())
}
pub fn verify_version(initialized: &Value, expected: &str) -> Result<()> {
    let actual = initialized["userAgent"]
        .as_str()
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.split_once('/'))
        .map(|(_, v)| v);
    ensure!(
        actual == Some(expected),
        "Control socket runtime differs from its executable; restart Codex before connecting"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_version_must_match_the_connected_process() {
        assert!(
            verify_version(
                &serde_json::json!({"userAgent":"anywhere_host/1.2.3 (macOS)"}),
                "1.2.3"
            )
            .is_ok()
        );
        assert!(
            verify_version(
                &serde_json::json!({"userAgent":"anywhere_host/1.2.4 (macOS)"}),
                "1.2.3"
            )
            .is_err()
        );
        assert!(verify_version(&serde_json::json!({}), "1.2.3").is_err());
    }
    #[tokio::test]
    async fn peer_executable_comes_from_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::UnixListener::bind(dir.path().join("rpc.sock")).unwrap();
        let stream = tokio::net::UnixStream::connect(dir.path().join("rpc.sock"))
            .await
            .unwrap();
        let (_peer, _) = listener.accept().await.unwrap();
        assert_eq!(
            peer_binary(&stream, dir.path())
                .unwrap()
                .canonicalize()
                .unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
    }
}
