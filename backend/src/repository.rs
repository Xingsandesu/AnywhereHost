//! Read-only Git projection. libgit2 has no registered external filters or hooks.
//! Invoked as a bounded child of the Host; never run the Git command-line client
//! against remote-selected repositories (status/diff can execute clean/fsmonitor).
use anyhow::{Context, Result, ensure};
use git2::{
    Diff, DiffFormat, DiffOptions, ErrorCode, Repository, RepositoryOpenFlags, Status,
    StatusOptions,
};
use serde_json::{Value, json};
use std::path::Path;

pub const MAX_OUTPUT: usize = 4 * 1024 * 1024;
const MAX_FILES: usize = 50_000;

pub fn limit_reader() -> Result<()> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    // macOS has no reliable address-space limit; also bound output and wall-clock time.
    let limits = [(Resource::Cpu, 8), (Resource::Core, 0)];
    for (resource, bound) in limits {
        let old = getrlimit(resource);
        let bound = old.maximum.map_or(bound, |max| max.min(bound));
        setrlimit(
            resource,
            Rlimit {
                current: Some(bound),
                maximum: Some(bound),
            },
        )?;
    }
    Ok(())
}
fn options() -> DiffOptions {
    let mut options = DiffOptions::new();
    options.ignore_submodules(true).max_size(2 * 1024 * 1024);
    options
}
fn patch(diff: Diff<'_>) -> Result<String> {
    ensure!(diff.deltas().len() <= MAX_FILES, "Too many changed files");
    let mut bytes = Vec::new();
    diff.print(DiffFormat::Patch, |_, _, line| {
        let prefix = matches!(line.origin(), '+' | '-' | ' ');
        if bytes.len() + line.content().len() + usize::from(prefix) > MAX_OUTPUT / 3 {
            return false;
        }
        if prefix {
            bytes.push(line.origin() as u8);
        }
        bytes.extend_from_slice(line.content());
        true
    })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
fn code(status: Status, index: bool) -> char {
    let flags = if index {
        [
            (Status::INDEX_NEW, 'A'),
            (Status::INDEX_DELETED, 'D'),
            (Status::INDEX_MODIFIED, 'M'),
            (Status::INDEX_RENAMED, 'R'),
            (Status::INDEX_TYPECHANGE, 'T'),
        ]
    } else {
        [
            (Status::WT_NEW, '?'),
            (Status::WT_DELETED, 'D'),
            (Status::WT_MODIFIED, 'M'),
            (Status::WT_RENAMED, 'R'),
            (Status::WT_TYPECHANGE, 'T'),
        ]
    };
    flags
        .into_iter()
        .find(|(flag, _)| status.contains(*flag))
        .map_or(' ', |(_, code)| code)
}
pub fn read(root: &Path) -> Result<Value> {
    ensure!(root.is_absolute(), "Project root must be absolute");
    // Do not discover a parent repository outside the requested project root.
    let repo = Repository::open_ext(
        root,
        RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&Path>(),
    )?;
    ensure!(!repo.is_bare(), "Bare repositories are unsupported");
    ensure!(
        repo.workdir().context("Missing worktree")?.canonicalize()? == root.canonicalize()?,
        "Project must name the repository worktree root"
    );
    let index = repo.index()?;
    ensure!(
        index.len() <= MAX_FILES,
        "Repository index exceeds file limit"
    );
    // libgit2 cannot reproduce user-defined clean/process filters. Report that
    // limitation explicitly instead of presenting an unfiltered diff as truth.
    for entry in index.iter() {
        let path = std::str::from_utf8(&entry.path).context("Non-UTF-8 Git path is unsupported")?;
        let attr = repo.get_attr(
            Path::new(path),
            "filter",
            git2::AttrCheckFlags::FILE_THEN_INDEX,
        )?;
        ensure!(
            matches!(
                git2::AttrValue::from_string(attr),
                git2::AttrValue::Unspecified | git2::AttrValue::False
            ),
            "Repositories using custom Git filters are unsupported"
        );
    }
    let head = match repo.head() {
        Ok(head) => Some(head),
        Err(error) if error.code() == ErrorCode::UnbornBranch => None,
        Err(error) => return Err(error.into()),
    };
    let mut status = match &head {
        Some(head) if head.is_branch() => {
            format!("## {}\n", head.shorthand().context("Invalid branch name")?)
        }
        Some(_) => "## HEAD (detached)\n".into(),
        None => {
            let head = repo.find_reference("HEAD")?;
            format!(
                "## No commits yet on {}\n",
                head.symbolic_target()?
                    .context("Invalid unborn HEAD")?
                    .strip_prefix("refs/heads/")
                    .context("Invalid branch reference")?
            )
        }
    };
    let mut status_options = StatusOptions::new();
    status_options
        .include_untracked(true)
        .include_ignored(false)
        .exclude_submodules(true)
        .renames_head_to_index(false)
        .renames_index_to_workdir(false)
        .update_index(false);
    let statuses = repo.statuses(Some(&mut status_options))?;
    ensure!(statuses.len() <= MAX_FILES, "Too many changed files");
    for entry in statuses.iter() {
        let flags = entry.status();
        // An unmerged index needs a full conflict-stage representation. Fail
        // explicitly instead of inventing a porcelain XY code from partial flags.
        ensure!(
            !flags.is_conflicted(),
            "Conflicted repositories are unsupported"
        );
        let path = entry.path().context("Non-UTF-8 Git path is unsupported")?;
        let (x, y) = if flags.is_wt_new() {
            ('?', '?')
        } else {
            (code(flags, true), code(flags, false))
        };
        // JSON quoting escapes line breaks/control characters in filenames.
        status.push_str(&format!("{x}{y} {}\n", json!(path)));
        ensure!(status.len() <= MAX_OUTPUT / 3, "Git status exceeds limit");
    }
    let tree = head.as_ref().map(|head| head.peel_to_tree()).transpose()?;
    let unstaged = patch(repo.diff_index_to_workdir(Some(&index), Some(&mut options()))?)?;
    let staged =
        patch(repo.diff_tree_to_index(tree.as_ref(), Some(&index), Some(&mut options()))?)?;
    let result = json!({"status":status,"unstagedDiff":unstaged,"stagedDiff":staged,"untrackedContentsIncluded":false,"submodulesIncluded":false});
    ensure!(
        serde_json::to_vec(&result)?.len() < MAX_OUTPUT,
        "Git output exceeds limit"
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_actual_staged_and_unstaged_changes_without_running_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("tracked.txt"), "staged\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("tracked.txt")).unwrap();
        index.write().unwrap();
        std::fs::write(dir.path().join("tracked.txt"), "unstaged\n").unwrap();
        let script = dir.path().join("should-not-run.sh");
        std::fs::write(&script, "#!/bin/sh\ntouch \"$0.ran\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut config = repo.config().unwrap();
        for key in [
            "core.fsmonitor",
            "diff.external",
            "diff.evil.command",
            "diff.evil.textconv",
            "filter.evil.clean",
            "filter.evil.process",
        ] {
            config.set_str(key, script.to_str().unwrap()).unwrap();
        }
        std::fs::write(dir.path().join(".gitattributes"), "*.txt diff=evil\n").unwrap();
        let result = read(dir.path()).unwrap();
        assert!(result["stagedDiff"].as_str().unwrap().contains("+staged"));
        assert!(
            result["unstagedDiff"]
                .as_str()
                .unwrap()
                .contains("+unstaged")
        );
        std::fs::write(dir.path().join(".gitattributes"), "*.txt filter=evil\n").unwrap();
        assert!(read(dir.path()).is_err());
        assert!(!script.with_extension("sh.ran").exists());
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        assert!(read(&subdir).is_err());
    }
}
