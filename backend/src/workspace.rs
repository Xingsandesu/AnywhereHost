use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
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
/// Images use the same canonical project boundary and authentication as file reads.
pub async fn image(root: &Path, relative: &str) -> Result<Value> {
    const LIMIT: u64 = 12 * 1024 * 1024;
    let target = resolve(root, relative)?;
    let file = tokio::fs::File::open(target).await?;
    let metadata = file.metadata().await?;
    ensure!(
        metadata.is_file() && metadata.len() <= LIMIT,
        "Only images up to 12 MiB are supported"
    );
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes).await?;
    ensure!(bytes.len() <= LIMIT as usize, "image exceeds size limit");
    let mime = image_mime(&bytes).context("Unsupported image format")?;
    Ok(json!({"path":relative,"mimeType":mime,"dataBase64":STANDARD.encode(bytes)}))
}
fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some("image/tiff")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else if bytes.get(4..8) == Some(b"ftyp")
        && matches!(bytes.get(8..12), Some(b"heic" | b"heix" | b"mif1"))
    {
        Some("image/heic")
    } else {
        None
    }
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
    #[tokio::test]
    async fn image_reads_are_bounded_and_confined_to_project() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let png = b"\x89PNG\r\n\x1a\nfixture";
        std::fs::write(root.join("image.png"), png).unwrap();
        let result = image(&root, "image.png").await.unwrap();
        assert_eq!(result["mimeType"], "image/png");
        assert_eq!(
            STANDARD
                .decode(result["dataBase64"].as_str().unwrap())
                .unwrap(),
            png
        );
        std::fs::write(root.join("text.png"), b"secret text").unwrap();
        assert!(image(&root, "text.png").await.is_err());
        std::fs::write(temp.path().join("outside.png"), png).unwrap();
        std::os::unix::fs::symlink(temp.path().join("outside.png"), root.join("escape.png"))
            .unwrap();
        for path in ["../outside.png", "escape.png", "/etc/passwd", ""] {
            assert!(image(&root, path).await.is_err());
        }
        let large = std::fs::File::create(root.join("large.png")).unwrap();
        large.set_len(12 * 1024 * 1024 + 1).unwrap();
        assert!(image(&root, "large.png").await.is_err());
    }
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
