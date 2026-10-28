use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

pub fn resolve(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(
        !Path::new(relative).is_absolute(),
        "path must be relative to the project root"
    );
    let root = root.canonicalize()?;
    let target = root.join(relative).canonicalize()?;
    ensure!(target.starts_with(&root), "path escapes the project root");
    Ok(target)
}
pub async fn files(root: &Path, relative: &str) -> Result<Value> {
    let target = resolve(root, relative)?;
    let mut entries = tokio::fs::read_dir(target).await?;
    let mut result = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_name() == ".git" {
            continue;
        }
        ensure!(
            result.len() < 5000,
            "Directory too large; select a subdirectory"
        );
        let kind = entry.file_type().await?;
        result.push(json!({"name":entry.file_name().to_string_lossy(),"kind":if kind.is_symlink(){"symlink"}else if kind.is_dir(){"directory"}else{"file"}}));
    }
    result.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(json!({"path":relative,"entries":result}))
}
pub async fn file(root: &Path, relative: &str) -> Result<Value> {
    let target = resolve(root, relative)?;
    let metadata = tokio::fs::metadata(&target).await?;
    ensure!(
        metadata.is_file() && metadata.len() <= 2 * 1024 * 1024,
        "Only text files up to 2 MiB are supported"
    );
    let mut bytes = Vec::new();
    tokio::fs::File::open(target)
        .await?
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(bytes.len() <= 2 * 1024 * 1024, "file exceeds size limit");
    let text = String::from_utf8(bytes).context("file is not UTF-8 text")?;
    Ok(json!({"path":relative,"text":text}))
}
async fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("/usr/bin/git");
    cmd.arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let mut bytes = Vec::new();
    let limit = 4 * 1024 * 1024;
    tokio::time::timeout(
        Duration::from_secs(10),
        stdout.take(limit + 1).read_to_end(&mut bytes),
    )
    .await??;
    ensure!(bytes.len() <= limit as usize, "git output too large");
    let exit = tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
    ensure!(exit.success(), "git operation failed");
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
pub async fn git_state(root: &Path) -> Result<Value> {
    let status = git(root, &["status", "--porcelain=v1", "--branch"]).await?;
    let unstaged = git(root, &["diff", "--no-ext-diff", "--no-textconv", "--"]).await?;
    let staged = git(
        root,
        &["diff", "--cached", "--no-ext-diff", "--no-textconv", "--"],
    )
    .await?;
    Ok(
        json!({"status":status,"unstagedDiff":unstaged,"stagedDiff":staged,"untrackedContentsIncluded":false}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_traversal_and_symlink_escape() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(t.path().join("secret"), "hidden").unwrap();
        std::os::unix::fs::symlink(t.path().join("secret"), root.join("link")).unwrap();
        assert!(resolve(&root, "../secret").is_err());
        assert!(resolve(&root, "link").is_err());
        assert!(resolve(&root, "/etc/passwd").is_err());
        assert!(resolve(&root, "").is_ok());
    }
}
