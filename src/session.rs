use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const SESSION_DIR_NAME: &str = ".agent-recorder";
pub const SESSION_FILE_NAME: &str = "session";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFile {
    pub run_id: String,
    pub pid: u32,
    pub socket: String,
    pub workdir: String,
    pub output: String,
    pub title: Option<String>,
}

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

pub fn session_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(SESSION_DIR_NAME))
}

pub fn session_path() -> Result<PathBuf> {
    Ok(session_dir()?.join(SESSION_FILE_NAME))
}

pub fn run_tmp_dir(run_id: &str) -> PathBuf {
    std::env::temp_dir().join("agent-recorder").join(run_id)
}

pub fn socket_path(run_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("agent-recorder-{run_id}.sock"))
}

pub fn load_session() -> Result<Option<SessionFile>> {
    let path = session_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)?;
    let session: SessionFile = serde_json::from_str(&text)
        .with_context(|| format!("corrupt session file {}", path.display()))?;
    if !pid_alive(session.pid) {
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&session.socket);
        return Ok(None);
    }
    Ok(Some(session))
}

pub fn save_session(session: &SessionFile) -> Result<()> {
    crate::id::write_json_atomic(&session_path()?, session)
}

pub fn clear_session() -> Result<()> {
    let path = session_path()?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn require_active_session() -> Result<SessionFile> {
    match load_session()? {
        Some(s) => Ok(s),
        None => bail!("No active recording."),
    }
}

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let status = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status();
    matches!(status, Ok(s) if s.success())
}

pub fn recordings_dir(workdir: &Path) -> PathBuf {
    workdir.join("recordings")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_tmp_contains_id() {
        let p = run_tmp_dir("7F32");
        assert!(p.ends_with("7F32"));
        assert!(p.to_string_lossy().contains("agent-recorder"));
    }
}
