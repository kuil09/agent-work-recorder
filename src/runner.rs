//! Runs commands in the invoking CLI's cwd/environment; the recorder daemon never blocks on tests.
use crate::protocol::TestResult;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

const TAIL_BYTES: usize = 4096;
static INTERRUPTED: AtomicI32 = AtomicI32::new(0);
extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
    fn kill(pid: i32, sig: i32) -> i32;
}
extern "C" fn interrupted(sig: i32) { INTERRUPTED.store(sig, Ordering::SeqCst); }
/// Only called by the test CLI, not by unit tests or the daemon.
pub fn install_signal_handlers() {
    unsafe { signal(2, interrupted as *const () as usize); signal(15, interrupted as *const () as usize); }
}

pub fn display_text(text: &str, limit: usize) -> String {
    // Discard CSI/OSC escapes and controls in video summaries, retaining printable Unicode.
    let mut clean = String::new();
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            match it.next() {
                Some('[') => { for v in it.by_ref() { if ('@'..='~').contains(&v) { break; } } }
                Some(']') => { while let Some(v) = it.next() { if v == '\u{7}' || (v == '\u{1b}' && it.next() == Some('\\')) { break; } } }
                _ => {}
            }
        } else if c.is_whitespace() { clean.push(' '); }
        else if !c.is_control() { clean.push(c); }
    }
    let flat = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > limit { format!("{}…", flat.chars().take(limit).collect::<String>()) } else { flat }
}

pub fn command_label(argv: &[String]) -> String {
    argv.iter().map(|s| {
        if !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "_./:-".contains(c)) { s.clone() }
        else { format!("'{}'", s.replace('\'', "'\\''")) }
    }).collect::<Vec<_>>().join(" ")
}

fn drain(mut input: impl Read, stderr: bool, echo: bool) -> String {
    let mut tail = VecDeque::with_capacity(TAIL_BYTES);
    let mut chunk = [0u8; 8192];
    loop {
        match input.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if echo {
                    if stderr { let _ = std::io::stderr().write_all(&chunk[..n]); }
                    else { let _ = std::io::stdout().write_all(&chunk[..n]); }
                }
                for &b in &chunk[..n] {
                    if tail.len() == TAIL_BYTES { tail.pop_front(); }
                    tail.push_back(b);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&tail.into_iter().collect::<Vec<_>>()).into_owned()
}

pub fn execute(argv: &[String], timeout: Duration, echo: bool) -> TestResult {
    let started = Instant::now();
    let Some(program) = argv.first() else { return TestResult { error: Some("command is required".into()), ..Default::default() }; };
    let mut child = match Command::new(program).args(&argv[1..]).stdin(Stdio::null())
        .stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn() {
        Ok(c) => c,
        Err(e) => return TestResult { error: Some(e.to_string()), duration_ms: started.elapsed().as_millis() as u64, ..Default::default() },
    };
    let group = child.id() as i32;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let stdout = std::thread::spawn(move || drain(out, false, echo));
    let stderr = std::thread::spawn(move || drain(err, true, echo));
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Err(e) => { unsafe { kill(-group, 9); } let _ = child.wait(); break Err(e); }
            Ok(None) => {}
        }
        let sig = INTERRUPTED.load(Ordering::SeqCst);
        if started.elapsed() >= timeout || sig != 0 {
            timed_out = sig == 0;
            // Kill the entire test process group; otherwise grandchildren may keep pipes open.
            unsafe { kill(-group, 9); }
            break child.wait();
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // A completed shell may have left background children inheriting stdout/stderr.
    unsafe { kill(-group, 9); }
    let mut result = TestResult {
        duration_ms: started.elapsed().as_millis() as u64,
        timed_out,
        stdout_summary: stdout.join().unwrap_or_default(),
        stderr_summary: stderr.join().unwrap_or_default(),
        ..Default::default()
    };
    match status {
        Ok(s) => { result.exit_code = s.code(); result.signal = s.signal(); }
        Err(e) => result.error = Some(e.to_string()),
    }
    let interrupted = INTERRUPTED.load(Ordering::SeqCst);
    if interrupted != 0 { result.exit_code = None; result.signal = Some(interrupted); }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captures_both_streams_and_exit_without_verdict() {
        let r = execute(&["sh".into(), "-c".into(), "printf hello; printf error >&2; exit 7".into()], Duration::from_secs(3), false);
        assert_eq!(r.exit_code, Some(7)); assert_eq!(r.cli_exit_code(), 7);
        assert_eq!(r.stdout_summary, "hello"); assert_eq!(r.stderr_summary, "error");
        assert!(!r.card_body("sh").contains("PASS"));
    }
    #[test]
    fn timeout_and_missing_command_are_explicit() {
        let r = execute(&["sh".into(), "-c".into(), "sleep 30".into()], Duration::from_millis(60), false);
        assert!(r.timed_out); assert_eq!(r.cli_exit_code(), 124);
        assert_eq!(execute(&["/nonexistent/rec-test".into()], Duration::from_secs(1), false).cli_exit_code(), 127);
    }
    #[test]
    fn output_is_bounded_and_unicode_safe() {
        let data = "한글".repeat(10000);
        let summary = drain(data.as_bytes(), false, false);
        assert!(summary.len() <= TAIL_BYTES + 3);
        assert!(display_text("\u{1b}[31m한글\u{1b}[0m\n result", 50).starts_with("한글 result"));
    }
    #[test]
    fn shell_metacharacters_are_not_executed() {
        let r = execute(&["printf".into(), "%s".into(), "$(exit 7); literal".into()], Duration::from_secs(2), false);
        assert_eq!(r.exit_code, Some(0)); assert_eq!(r.stdout_summary, "$(exit 7); literal");
    }
}
