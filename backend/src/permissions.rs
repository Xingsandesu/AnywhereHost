//! Local access checks only: no server, configuration or Codex session is started.
use serde::Serialize;
use std::{io, io::Write, path::Path};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryAccess {
    readable: bool,
    writable: bool,
    issue: Option<&'static str>,
}

fn issue(error: io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::PermissionDenied => "permissionDenied",
        io::ErrorKind::NotFound => "missing",
        _ => "unavailable",
    }
}

pub fn check(path: &Path) -> DirectoryAccess {
    let mut result = DirectoryAccess {
        readable: false,
        writable: false,
        issue: None,
    };
    // Enumerate at most one entry; never return names or read existing contents.
    let read =
        std::fs::read_dir(path).and_then(|mut entries| entries.next().transpose().map(|_| ()));
    if let Err(error) = read {
        result.issue = Some(issue(error));
        return result;
    }
    result.readable = true;
    let write = (|| -> io::Result<()> {
        let mut file = tempfile::Builder::new()
            .prefix(".anywhere-access-")
            .tempfile_in(path)?;
        file.write_all(b"Anywhere Host access check\n")?;
        file.flush()?;
        file.close()
    })();
    match write {
        Ok(()) => result.writable = true,
        Err(error) => result.issue = Some(issue(error)),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checks_without_changing_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("project.txt");
        std::fs::write(&file, "original").unwrap();
        let result = check(dir.path());
        assert!(result.readable && result.writable);
        assert!(result.issue.is_none());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn missing_path_is_not_created_or_reported_as_denied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing");
        let result = check(&path);
        assert_eq!(result.issue, Some("missing"));
        assert!(!result.readable && !result.writable && !path.exists());
    }
    #[test]
    fn file_is_not_a_directory() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let result = check(file.path());
        assert!(!result.readable && !result.writable);
        assert_eq!(result.issue, Some("unavailable"));
    }
    #[test]
    fn denied_read_and_write_are_separate() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let read_only = check(dir.path());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
        let denied = check(dir.path());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(read_only.readable && !read_only.writable);
        assert_eq!(read_only.issue, Some("permissionDenied"));
        assert!(!denied.readable && !denied.writable);
        assert_eq!(denied.issue, Some("permissionDenied"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
