use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

static FILE_READERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(16);

// Pin each component to a directory descriptor. Never validate a pathname and
// reopen it later: a repository can replace it with a symlink between the two.
// Symlinks are listed, but deliberately never traversed (including internal ones).
fn open_at(root: &std::fs::File, relative: &str, directory: bool) -> Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, openat};
    use std::path::Component;
    let mut parts = Vec::new();
    for part in Path::new(relative).components() {
        match part {
            Component::Normal(name) => parts.push(name),
            Component::CurDir => {}
            _ => anyhow::bail!("path must stay relative to the project root"),
        }
    }
    let mut current = root.try_clone()?;
    for (index, part) in parts.iter().enumerate() {
        let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
        if directory || index + 1 < parts.len() {
            flags |= OFlags::DIRECTORY;
        }
        current = openat(&current, *part, flags, Mode::empty())?.into();
    }
    Ok(current)
}
fn project_directory(root: &Path) -> Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};
    ensure!(root.is_absolute(), "project root must be absolute");
    Ok(rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY,
        Mode::empty(),
    )?
    .into())
}
pub async fn files(root: &Path, relative: &str) -> Result<Value> {
    let permit = FILE_READERS
        .try_acquire()
        .context("File reader limit reached")?;
    let root = root.to_owned();
    let relative = relative.to_owned();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        use rustix::fs::{AtFlags, Dir, FileType, statat};
        let directory = open_at(&project_directory(&root)?, &relative, true)?;
        let entries = Dir::read_from(&directory)?;
        let mut result = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            if matches!(name.to_bytes(), b"." | b".." | b".git") { continue; }
            ensure!(result.len() < 5000, "Directory too large; select a subdirectory");
            let kind = FileType::from_raw_mode(statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)?.st_mode);
            result.push(json!({"name":name.to_string_lossy(),"kind":match kind {
                FileType::Symlink => "symlink", FileType::Directory => "directory", FileType::RegularFile => "file", _ => "special"
            }}));
        }
        result.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Ok(json!({"path":relative,"entries":result}))
    }).await?
}
async fn read_file(root: &Path, relative: &str, limit: u64) -> Result<Vec<u8>> {
    let permit = FILE_READERS
        .try_acquire()
        .context("File reader limit reached")?;
    let root = root.to_owned();
    let relative = relative.to_owned();
    tokio::task::spawn_blocking(move || {
        // Blocking reads can outlive HTTP cancellation; keep the permit inside.
        let _permit = permit;
        use std::io::Read;
        let file = open_at(&project_directory(&root)?, &relative, false)?;
        // NONBLOCK makes opening FIFOs safe; metadata is from this exact handle.
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() <= limit,
            "Only regular files within the size limit are supported"
        );
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= limit as usize, "file exceeds size limit");
        Ok(bytes)
    })
    .await?
}
pub async fn file(root: &Path, relative: &str) -> Result<Value> {
    let bytes = read_file(root, relative, 2 * 1024 * 1024).await?;
    let text = String::from_utf8(bytes).context("file is not UTF-8 text")?;
    Ok(json!({"path":relative,"text":text}))
}
/// Images and text share the same descriptor-based project boundary.
pub async fn image(root: &Path, relative: &str) -> Result<Value> {
    let bytes = read_file(root, relative, 12 * 1024 * 1024).await?;
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

pub async fn git_state(root: &Path) -> Result<Value> {
    static READERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let _permit = READERS
        .try_acquire()
        .context("Repository reader limit reached")?;
    // Run the built-in read-only reader out of process so an expensive repository
    // cannot occupy a Host worker forever. No repository-supplied programs run.
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--read-git-state")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().context("missing Git reader output")?;
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        stdout
            .take(crate::repository::MAX_OUTPUT as u64 + 1)
            .read_to_end(&mut bytes)
            .await?;
        ensure!(
            bytes.len() <= crate::repository::MAX_OUTPUT,
            "Git output too large"
        );
        ensure!(
            child.wait().await?.success(),
            "Repository cannot be read safely or exceeds limits"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(serde_json::from_slice(&bytes)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn special_files_and_directory_symlinks_are_rejected_without_blocking() {
        let temp = tempfile::tempdir().unwrap();
        // rustix does not expose mkfifoat on macOS. The POSIX utility creates
        // this temporary fixture without a shell.
        assert!(
            std::process::Command::new("mkfifo")
                .args(["-m", "600"])
                .arg(temp.path().join("pipe"))
                .status()
                .unwrap()
                .success()
        );
        let socket = std::os::unix::net::UnixListener::bind(temp.path().join("socket")).unwrap();
        for name in ["pipe", "socket"] {
            assert!(
                tokio::time::timeout(Duration::from_secs(1), file(temp.path(), name))
                    .await
                    .unwrap()
                    .is_err()
            );
            assert!(
                tokio::time::timeout(Duration::from_secs(1), image(temp.path(), name))
                    .await
                    .unwrap()
                    .is_err()
            );
        }
        std::fs::create_dir(temp.path().join("directory")).unwrap();
        std::os::unix::fs::symlink("directory", temp.path().join("link")).unwrap();
        assert!(files(temp.path(), "link").await.is_err());
        drop(socket);
    }
    #[test]
    fn pinned_root_cannot_be_redirected_by_path_replacement() {
        use std::io::Read;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(root.join("file"), "inside").unwrap();
        std::fs::write(outside.join("file"), "outside secret").unwrap();
        let pinned = project_directory(&root).unwrap();
        std::fs::rename(&root, temp.path().join("original")).unwrap();
        std::os::unix::fs::symlink(&outside, &root).unwrap();
        let mut text = String::new();
        open_at(&pinned, "file", false)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "inside");
        std::fs::remove_file(temp.path().join("original/file")).unwrap();
        std::os::unix::fs::symlink(outside.join("file"), temp.path().join("original/file"))
            .unwrap();
        assert!(open_at(&pinned, "file", false).is_err());
    }
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
        assert!(open_at(&project_directory(&root).unwrap(), "../secret", false).is_err());
        assert!(open_at(&project_directory(&root).unwrap(), "link", false).is_err());
        assert!(open_at(&project_directory(&root).unwrap(), "/etc/passwd", false).is_err());
        assert!(open_at(&project_directory(&root).unwrap(), "", true).is_ok());
    }
}
