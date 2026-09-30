use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
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

#[derive(Debug)]
enum ProcessState {
    Running,
    Exited,
    // EPERM/EACCES and unexpected errors are not evidence of process death.
    Unknown(io::Error),
}

// Darwin's public <sys/errno.h>: ESRCH = 3 (also the Unix hosts used by our tests).
// Do not classify a generic nonzero exit status or ErrorKind::NotFound as ESRCH.
const ESRCH: i32 = 3;

fn probe_process_with(pid: u32, probe: impl FnOnce(i32) -> io::Result<()>) -> ProcessState {
    // kill(0, 0) and kill(negative, 0) address process groups, not this daemon.
    let pid = match i32::try_from(pid) {
        Ok(pid) if pid > 0 => pid,
        _ => {
            return ProcessState::Unknown(io::Error::new(
                io::ErrorKind::InvalidInput,
                "PID must be a positive signed 32-bit process ID",
            ));
        }
    };
    match probe(pid) {
        Ok(()) => ProcessState::Running,
        Err(error) if error.raw_os_error() == Some(ESRCH) => ProcessState::Exited,
        Err(error) => ProcessState::Unknown(error),
    }
}

fn probe_process(pid: u32) -> ProcessState {
    probe_process_with(pid, |pid| {
        extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // SAFETY: pid is positive and representable as Darwin pid_t; signal 0
        // performs a permission/existence check without delivering a signal.
        // Read errno immediately, on this thread, before doing any other work.
        if unsafe { kill(pid, 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    })
}

/// Read the session without unlinking or rewriting anything, even for ESRCH.
/// Only the daemon's explicit lifecycle actions may change session resources.
/// A PID check cannot prove socket ownership, and a restricted caller must not
/// partially clean up an authorized host's Run (issue #7).
pub fn load_session() -> Result<Option<SessionFile>> {
    load_session_with(&session_path()?, probe_process)
}

fn load_session_with(
    path: &Path,
    probe: impl FnOnce(u32) -> ProcessState,
) -> Result<Option<SessionFile>> {
    // exists() hides permission/I/O errors; only an actual missing file is None.
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot read session {}; nothing was removed. Use the same authorized host context as rec start",
                    path.display()
                )
            });
        }
    };
    let session: SessionFile = serde_json::from_str(&text)
        .with_context(|| format!("corrupt session file {}; left unchanged", path.display()))?;
    match probe(session.pid) {
        ProcessState::Running => Ok(Some(session)),
        // Deliberately retain stale metadata/socket for diagnosis. Starting a new
        // Run may replace the session record under the daemon's existing lock;
        // lookup never deletes a socket, even if the OS hides a PID with ESRCH.
        ProcessState::Exited => Ok(None),
        ProcessState::Unknown(error) => Err(error).with_context(|| {
            format!(
                "cannot verify recording Run {} (daemon PID {}). Session {} and socket {} were left unchanged. A sandbox/access restriction or an invalid PID is not proof that the daemon exited. Run all rec commands from the same authorized host context as rec start; do not delete the session or socket. See docs/session-recovery.md",
                session.run_id, session.pid, path.display(), session.socket
            )
        }),
    }
}

pub fn save_session(session: &SessionFile) -> Result<()> {
    crate::id::write_json_atomic(&session_path()?, session)
}

/// Used by the owning daemon after explicit successful finalization, not lookup.
pub fn clear_session() -> Result<()> {
    clear_session_at(&session_path()?)
}

fn clear_session_at(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to clear session {}; cleanup did not complete", path.display())),
    }
}

pub fn require_active_session() -> Result<SessionFile> {
    match load_session()? {
        Some(s) => Ok(s),
        None => bail!("No active recording."),
    }
}

/// Conservative compatibility predicate for the daemon's test-owner checks.
/// Unknown/denied access must not reap a still-running test. This is NOT proof
/// of ownership or identity. Invalid PIDs are never accepted as test owners.
pub fn pid_alive(pid: u32) -> bool {
    pid > 0 && pid <= i32::MAX as u32 && !matches!(probe_process(pid), ProcessState::Exited)
}

pub fn recordings_dir(workdir: &Path) -> PathBuf {
    workdir.join("recordings")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        session: PathBuf,
        socket: PathBuf,
        listener: UnixListener,
        bytes: Vec<u8>,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "rec-s7-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let session = root.join("session");
            let socket = root.join("run.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            let value = SessionFile {
                run_id: "7F32".into(), pid: std::process::id(),
                socket: socket.display().to_string(), workdir: root.display().to_string(),
                output: root.join("review.mp4").display().to_string(), title: None,
            };
            let bytes = serde_json::to_vec(&value).unwrap();
            fs::write(&session, &bytes).unwrap();
            Self { root, session, socket, listener, bytes }
        }

        fn assert_unchanged(&self, session_inode: u64, socket_inode: u64) {
            assert_eq!(fs::read(&self.session).unwrap(), self.bytes);
            assert_eq!(fs::metadata(&self.session).unwrap().ino(), session_inode);
            assert_eq!(fs::symlink_metadata(&self.socket).unwrap().ino(), socket_inode);
            // The original listening endpoint remains reachable from the host.
            let _client = UnixStream::connect(&self.socket).unwrap();
            let _server = self.listener.accept().unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
    }

    #[test]
    fn run_tmp_contains_id() {
        let p = run_tmp_dir("7F32");
        assert!(p.ends_with("7F32"));
        assert!(p.to_string_lossy().contains("agent-recorder"));
    }

    #[test]
    fn only_esrch_is_confirmed_exit() {
        assert!(matches!(probe_process_with(1, |_| Ok(())), ProcessState::Running));
        assert!(matches!(probe_process_with(1, |_| Err(io::Error::from_raw_os_error(3))), ProcessState::Exited));
        for errno in [1, 2, 4, 5, 13, 22] {
            assert!(matches!(probe_process_with(1, |_| Err(io::Error::from_raw_os_error(errno))), ProcessState::Unknown(_)));
        }
        assert!(matches!(probe_process_with(1, |_| Err(io::Error::new(io::ErrorKind::NotFound, "not ESRCH"))), ProcessState::Unknown(_)));
    }

    #[test]
    fn invalid_pids_never_probe_a_process_group() {
        for pid in [0, i32::MAX as u32 + 1, u32::MAX] {
            assert!(matches!(probe_process_with(pid, |_| panic!("must not call kill")), ProcessState::Unknown(_)));
            assert!(!pid_alive(pid));
        }
    }

    #[test]
    fn real_current_process_is_running_and_reaped_child_is_exited() {
        assert!(pid_alive(std::process::id()));
        let mut child = std::process::Command::new("/bin/sh").args(["-c", "exit 0"]).spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(matches!(probe_process(pid), ProcessState::Exited));
    }

    #[test]
    fn eperm_lookup_cannot_unlink_a_live_socket_or_readonly_session() {
        let f = Fixture::new();
        fs::set_permissions(&f.session, fs::Permissions::from_mode(0o400)).unwrap();
        let a = fs::metadata(&f.session).unwrap().ino();
        let b = fs::symlink_metadata(&f.socket).unwrap().ino();
        let error = load_session_with(&f.session, |pid| {
            probe_process_with(pid, |_| Err(io::Error::from_raw_os_error(1)))
        }).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("left unchanged"));
        assert!(message.contains("authorized host context"));
        assert!(!message.contains("No active recording"));
        f.assert_unchanged(a, b);
        assert!(load_session_with(&f.session, probe_process).unwrap().is_some());
    }

    #[test]
    fn all_uncertain_probes_preserve_the_session_and_endpoint() {
        for errno in [2, 4, 5, 13, 22] {
            let f = Fixture::new();
            let a = fs::metadata(&f.session).unwrap().ino();
            let b = fs::symlink_metadata(&f.socket).unwrap().ino();
            assert!(load_session_with(&f.session, |pid| probe_process_with(pid, |_| Err(io::Error::from_raw_os_error(errno)))).is_err());
            f.assert_unchanged(a, b);
        }
    }

    #[test]
    fn even_esrch_lookup_is_readonly() {
        let f = Fixture::new();
        let a = fs::metadata(&f.session).unwrap().ino();
        let b = fs::symlink_metadata(&f.socket).unwrap().ino();
        assert!(load_session_with(&f.session, |pid| probe_process_with(pid, |_| Err(io::Error::from_raw_os_error(3)))).unwrap().is_none());
        f.assert_unchanged(a, b);
    }

    #[test]
    fn missing_corrupt_and_unreadable_sessions_are_distinct() {
        let f = Fixture::new();
        assert!(load_session_with(&f.root.join("missing"), |_| panic!("no PID to probe")).unwrap().is_none());
        fs::write(&f.session, "not JSON").unwrap();
        assert!(load_session_with(&f.session, |_| panic!("corrupt session")).unwrap_err().to_string().contains("corrupt session"));
        assert_eq!(fs::read_to_string(&f.session).unwrap(), "not JSON");
        // Reading a directory deterministically fails even when tests run as root.
        assert!(load_session_with(&f.root, |_| panic!("unreadable session")).is_err());
        assert!(f.socket.exists());
    }

    #[test]
    fn cleanup_errors_are_reported_and_only_notfound_is_ignored() {
        let f = Fixture::new();
        let error = clear_session_at(&f.root).unwrap_err();
        assert!(error.to_string().contains("cleanup did not complete"));
        assert!(f.root.is_dir());
        clear_session_at(&f.root.join("missing")).unwrap();
        clear_session_at(&f.session).unwrap();
        assert!(!f.session.exists());
        // Explicit session removal must not implicitly remove any socket.
        assert!(f.socket.exists());
    }
}
