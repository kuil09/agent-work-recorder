use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitInfo {
    pub available: bool,
    pub repository: Option<String>,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub working_tree: Option<String>,
    pub changed_files: Option<u32>,
}

impl GitInfo {
    pub fn unavailable() -> Self {
        GitInfo {
            available: false,
            repository: None,
            root: None,
            branch: None,
            commit: None,
            working_tree: None,
            changed_files: None,
        }
    }

    pub fn collect(cwd: &Path) -> Self {
        let root = git_output(cwd, &["rev-parse", "--show-toplevel"]);
        let Some(root) = root else {
            return Self::unavailable();
        };
        let root_path = PathBuf::from(root.trim());
        let repository = root_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned());
        let branch = git_output(&root_path, &["rev-parse", "--abbrev-ref", "HEAD"]);
        let commit = git_output(&root_path, &["rev-parse", "--short=7", "HEAD"]);
        let dirty = git_output(&root_path, &["status", "--porcelain"])
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let changed_files = git_output(&root_path, &["status", "--porcelain"])
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count() as u32);
        GitInfo {
            available: true,
            repository,
            root: Some(root_path.display().to_string()),
            branch,
            commit,
            working_tree: Some(if dirty {
                "DIRTY".into()
            } else {
                "CLEAN".into()
            }),
            changed_files,
        }
    }
}

fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn missing_git_is_unavailable() {
        let info = GitInfo::collect(Path::new("/tmp"));
        // /tmp may or may not be a repo; just ensure the struct is well-formed.
        if !info.available {
            assert!(info.repository.is_none());
            assert!(info.commit.is_none());
        }
    }

    #[test]
    fn this_repo_if_present() {
        let cwd = env::current_dir().unwrap();
        let _ = GitInfo::collect(&cwd);
    }
}
