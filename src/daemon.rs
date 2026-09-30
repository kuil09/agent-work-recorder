use crate::git::GitInfo;
use crate::id::{format_step_id, write_json_atomic};
use crate::protocol::{CaptureHud, EventKind, IpcRequest, IpcResponse, ObserveStatus, RunEvent, TestResult};
use crate::session::{self, SessionFile};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, clap::Args, Serialize, Deserialize)]
pub struct CaptureOptions {
    /// Capture the main display (only `full` is supported)
    #[arg(long, conflicts_with_all = ["window", "window_id", "app"]) ]
    pub screen: Option<String>,
    /// Window title/application substring; prefer --window-id when ambiguous
    #[arg(long, conflicts_with_all = ["screen", "window_id", "app"]) ]
    pub window: Option<String>,
    #[arg(long, conflicts_with_all = ["screen", "window", "app"]) ]
    pub window_id: Option<u32>,
    /// Exact application name or bundle ID; its windows on the main display
    #[arg(long, conflicts_with_all = ["screen", "window", "window_id"]) ]
    pub app: Option<String>,
    /// Opt in to system/app audio; NEVER records the microphone
    #[arg(long)]
    pub system_audio: bool,
}
impl CaptureOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!([self.screen.is_some(), self.window.is_some(), self.window_id.is_some(), self.app.is_some()].into_iter().filter(|v| *v).count() <= 1,
            "use only one of --screen, --window, --window-id, --app");
        ensure!(self.screen.as_deref().map(|s| s == "full").unwrap_or(true), "--screen only supports full");
        for value in [&self.app, &self.window].into_iter().flatten() { ensure!(!value.trim().is_empty(), "capture target cannot be empty"); }
        Ok(())
    }
    pub fn describe(&self) -> String {
        if let Some(app) = &self.app { format!("app {app} (main display)") }
        else if let Some(window) = &self.window { format!("window {window}") }
        else if let Some(id) = self.window_id { format!("window ID {id}") }
        else { "main display".into() }
    }
    fn allows_screenshot_fallback(&self) -> bool {
        self.app.is_none() && self.window.is_none() && self.window_id.is_none() && !self.system_audio
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonArgs {
    pub run_id: String, pub title: Option<String>, pub workdir: PathBuf, pub output: PathBuf, pub capture: CaptureOptions,
}
struct ActiveTest { step: u32, owner_pid: u32, command: String }
struct RunState {
    run_id: String, title: Option<String>, output: PathBuf, tmp_dir: PathBuf,
    started: Instant, created_at: String, step: u32, checkpoints: u32, tests: u32,
    events: Vec<RunEvent>, action: Option<String>, verdict: Option<String>, git: GitInfo,
    active_test: Option<ActiveTest>, hold_until: Instant, finalizing: bool,
}
impl RunState {
    fn next_step(&mut self, kind: EventKind, text: String, status: Option<ObserveStatus>) -> RunEvent {
        self.step += 1;
        if kind == EventKind::Checkpoint { self.checkpoints += 1; }
        if kind != EventKind::Expect { self.action = Some(crate::runner::display_text(&text, 110)); }
        if kind == EventKind::Observe { self.verdict = status.and_then(|s| s.as_verdict_label()).map(String::from); }
        if matches!(kind, EventKind::TestStart | EventKind::TestResult) { self.verdict = None; }
        RunEvent { ts_ms: now_ms(), media_ms: self.started.elapsed().as_millis() as u64, step: self.step,
            step_id: format_step_id(&self.run_id, self.step), kind, text, status, test_result: None }
    }
    fn response(&self) -> IpcResponse {
        let s = self.started.elapsed().as_secs();
        IpcResponse { ok: true, run_id: Some(self.run_id.clone()), step: Some(self.step),
            step_id: Some(format_step_id(&self.run_id, self.step)), output: Some(self.output.display().to_string()),
            duration: Some(format!("{:02}:{:02}", s / 60, s % 60)), steps: Some(self.step),
            checkpoints: Some(self.checkpoints), tests: Some(self.tests), verdict: self.verdict.clone(), ..Default::default() }
    }
}
fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }
fn find_capture_bin() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("REC_CAPTURE") { return Ok(PathBuf::from(p)); }
    if let Some(dir) = std::env::current_exe()?.parent() {
        let p = dir.join("rec-capture"); if p.is_file() { return Ok(p); }
    }
    let out = Command::new("which").arg("rec-capture").output()?;
    ensure!(out.status.success(), "rec-capture not found on PATH");
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}
#[derive(Deserialize)]
struct WindowInfo { id: u32, app: String, #[serde(default)] title: String }
fn resolve_window(bin: &Path, query: &str) -> Result<u32> {
    let out = Command::new(bin).arg("list-windows").output()?;
    ensure!(out.status.success(), "list-windows failed: {}", String::from_utf8_lossy(&out.stderr));
    let windows: Vec<WindowInfo> = serde_json::from_slice(&out.stdout).context("invalid window list")?;
    let q = query.to_lowercase();
    let matches: Vec<_> = windows.iter().filter(|w| w.title.to_lowercase().contains(&q) || w.app.to_lowercase().contains(&q)).collect();
    if matches.len() == 1 { return Ok(matches[0].id); }
    let exact: Vec<_> = matches.iter().filter(|w| w.title.to_lowercase() == q).collect();
    if exact.len() == 1 { return Ok(exact[0].id); }
    let options = matches.iter().map(|w| format!("{}: {} — {}", w.id, w.app, w.title)).collect::<Vec<_>>().join("\n");
    bail!("window target is missing or ambiguous; select --window-id\n{options}")
}
#[derive(Deserialize)]
struct CaptureEvent { event: String, message: Option<String>, #[serde(default)] elapsed_ms: u64 }
struct CaptureProc { child: Child, stdin: ChildStdin, failure: Arc<Mutex<Option<String>>>, stopped: bool }
enum CaptureBackend { Sck(CaptureProc), Cua(crate::cua::CuaGrabber) }
impl CaptureBackend {
    fn health(&mut self) -> Result<()> {
        if let Self::Sck(p) = self {
            if let Some(error) = p.failure.lock().unwrap().clone() { bail!("capture failed: {error}"); }
            if !p.stopped { ensure!(p.child.try_wait()?.is_none(), "capture process exited unexpectedly"); }
        }
        Ok(())
    }
    fn send(&mut self, hud: &CaptureHud) -> Result<()> {
        self.health()?;
        match self {
            Self::Sck(p) => { serde_json::to_writer(&mut p.stdin, hud)?; p.stdin.write_all(b"\n")?; p.stdin.flush()?; Ok(()) }
            Self::Cua(p) => p.send(hud),
        }
    }
    fn stop(&mut self) -> Result<()> {
        match self {
            Self::Cua(p) => p.stop(),
            Self::Sck(p) => {
                if p.stopped { return Ok(()); }
                serde_json::to_writer(&mut p.stdin, &CaptureHud { cmd: "stop".into(), ..Default::default() })?;
                p.stdin.write_all(b"\n")?; p.stdin.flush()?;
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    if let Some(status) = p.child.try_wait()? { ensure!(status.success(), "capture finalization failed: {status}"); p.stopped = true; break; }
                    if Instant::now() >= deadline { let _ = p.child.kill(); let _ = p.child.wait(); bail!("capture finalization timed out; raw files retained"); }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if let Some(error) = p.failure.lock().unwrap().clone() { bail!("capture failed: {error}"); }
                Ok(())
            }
        }
    }
}
fn hud_update(s: &RunState) -> CaptureHud {
    CaptureHud { cmd: "hud".into(), step: Some(s.step), action: s.action.clone(), verdict: s.verdict.clone(), ..Default::default() }
}
fn hud_git(s: &RunState) -> CaptureHud {
    CaptureHud { cmd: "git".into(), step: Some(0), title: s.title.clone(), repository: s.git.repository.clone(),
        branch: s.git.branch.clone(), commit: s.git.commit.clone(), working_tree: s.git.working_tree.clone(), ..Default::default() }
}
fn spawn_capture(bin: &Path, raw: &Path, args: &DaemonArgs) -> Result<(CaptureProc, Instant)> {
    args.capture.validate()?;
    let mut cmd = Command::new(bin);
    cmd.args(["start", "--output"]).arg(raw).args(["--run-id", &args.run_id]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    if let Some(id) = args.capture.window_id { cmd.arg("--window-id").arg(id.to_string()); }
    else if let Some(w) = &args.capture.window { cmd.arg("--window-id").arg(resolve_window(bin, w)?.to_string()); }
    else if let Some(app) = &args.capture.app { cmd.arg("--app").arg(app); }
    else { cmd.args(["--display", "main"]); }
    if args.capture.system_audio { cmd.arg("--system-audio"); }
    let mut child = cmd.spawn().context("spawn rec-capture")?;
    let stdin = child.stdin.take().context("capture stdin")?;
    let stdout = child.stdout.take().context("capture stdout")?;
    let failure = Arc::new(Mutex::new(None)); let errors = failure.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            eprintln!("capture: {line}");
            if let Ok(ev) = serde_json::from_str::<CaptureEvent>(&line) {
                if ev.event == "ready" { let _ = tx.send((ev.elapsed_ms, Instant::now())); }
                if ev.event == "error" || ev.event == "permission-denied" {
                    *errors.lock().unwrap() = Some(ev.message.unwrap_or_else(|| "Screen Recording permission is required".into()));
                }
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(18);
    loop {
        if let Ok((elapsed, received)) = rx.try_recv() {
            let start = received.checked_sub(Duration::from_millis(elapsed)).unwrap_or(received);
            return Ok((CaptureProc { child, stdin, failure, stopped: false }, start));
        }
        if let Some(e) = failure.lock().unwrap().clone() { let _ = child.kill(); let _ = child.wait(); bail!("{e}"); }
        if let Some(status) = child.try_wait()? { bail!("rec-capture exited early: {status}"); }
        if Instant::now() >= deadline { let _ = child.kill(); let _ = child.wait(); bail!("rec-capture did not produce a first frame"); }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn record(state: &mut RunState, capture: &mut CaptureBackend, kind: EventKind, text: String, status: Option<ObserveStatus>, result: Option<TestResult>) -> Result<IpcResponse> {
    ensure!(!state.finalizing, "recording is finalizing; retry rec stop instead of adding events");
    capture.health()?;
    let mut event = state.next_step(kind, text, status); event.test_result = result;
    let mut f = OpenOptions::new().create(true).append(true).open(state.tmp_dir.join("events.jsonl"))?;
    serde_json::to_writer(&mut f, &event)?; f.write_all(b"\n")?;
    capture.send(&hud_update(state))?;
    capture.send(&CaptureHud { cmd: "card".into(), step: Some(event.step), kind: Some(kind.as_card_title().into()), body: Some(event.text.clone()), ..Default::default() })?;
    state.hold_until = Instant::now() + Duration::from_secs(4);
    state.events.push(event);
    let mut response = state.response(); response.kind = Some(kind.as_card_title().into());
    if kind != EventKind::Observe { response.verdict = None; }
    Ok(response)
}
fn finish(state: &mut RunState, capture: &mut CaptureBackend, raw: &Path, audio: bool) -> Result<IpcResponse> {
    ensure!(state.active_test.is_none(), "a test is still running; finish it before rec stop");
    if !state.finalizing {
        if let Some(wait) = state.hold_until.checked_duration_since(Instant::now()) { std::thread::sleep(wait); }
        state.finalizing = true;
    }
    capture.stop()?;
    let info = crate::media::probe(raw)?; crate::media::validate(&info, audio)?;
    let duration = crate::media::duration_ms(&info)?;
    let chapters = crate::media::chapters(&state.events, duration);
    let meta = state.tmp_dir.join("chapters.ffmetadata");
    fs::write(&meta, crate::media::metadata(state.title.as_deref(), &state.run_id, state.git.commit.as_deref(), &state.created_at, &chapters))?;
    if let Some(p) = state.output.parent() { fs::create_dir_all(p)?; }
    crate::media::remux(raw, &state.output, &meta, &chapters, audio, &state.run_id)?;
    let mut reply = state.response(); reply.duration = Some(format!("{:02}:{:02}", duration / 60000, duration / 1000 % 60));
    fs::remove_dir_all(&state.tmp_dir)?;
    Ok(reply)
}
fn reap_test(state: &mut RunState, capture: &mut CaptureBackend) -> Result<()> {
    if state.active_test.as_ref().map(|t| !session::pid_alive(t.owner_pid)).unwrap_or(false) {
        let t = state.active_test.take().unwrap();
        let result = TestResult { error: Some("test CLI exited before reporting a result; exit code unknown".into()), ..Default::default() };
        record(state, capture, EventKind::TestResult, result.card_body(&t.command), None, Some(result))?;
    }
    Ok(())
}
fn handle(req: IpcRequest, state: &mut RunState, capture: &mut CaptureBackend) -> Result<IpcResponse> {
    match req {
        IpcRequest::Note { text } => record(state, capture, EventKind::Note, text, None, None),
        IpcRequest::Expect { text } => record(state, capture, EventKind::Expect, text, None, None),
        IpcRequest::Observe { text, status } => record(state, capture, EventKind::Observe, text, status.or(Some(ObserveStatus::Info)), None),
        IpcRequest::Checkpoint { text } => record(state, capture, EventKind::Checkpoint, text, None, None),
        IpcRequest::Status => { capture.health()?; Ok(state.response()) }
        IpcRequest::TestBegin { command, cwd, owner_pid } => {
            ensure!(state.active_test.is_none(), "a test is already running");
            ensure!(!command.is_empty() && !command[0].is_empty(), "test command is required");
            ensure!(session::pid_alive(owner_pid), "test CLI is no longer running");
            let label = crate::runner::command_label(&command);
            let reply = record(state, capture, EventKind::TestStart, format!("Command: {label}\nCwd: {cwd}\nRunning (no verdict)"), None, None)?;
            state.tests += 1;
            state.active_test = Some(ActiveTest { step: state.step, owner_pid, command: label });
            Ok(reply)
        }
        IpcRequest::TestEnd { run_id, test_step, result } => {
            let active = state.active_test.as_ref().context("no matching active test")?;
            ensure!(run_id == state.run_id && test_step == active.step, "test result belongs to a different run/test");
            let body = result.card_body(&active.command);
            let reply = record(state, capture, EventKind::TestResult, body, None, Some(result))?;
            state.active_test = None; Ok(reply)
        }
        IpcRequest::Stop => bail!("internal stop dispatch error"),
    }
}

pub fn run(args: DaemonArgs) -> Result<()> {
    extern "C" { fn setsid() -> i32; fn flock(fd: i32, operation: i32) -> i32; }
    unsafe { setsid(); }
    args.capture.validate()?;
    fs::create_dir_all(session::session_dir()?)?;
    // Kernel-held lock prevents two simultaneous rec start processes from creating two runs.
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(session::session_dir()?.join("run.lock"))?;
    ensure!(unsafe { flock(lock.as_raw_fd(), 2 | 4) } == 0, "An active recording already exists.");
    let tmp_dir = session::run_tmp_dir(&args.run_id); fs::create_dir_all(&tmp_dir)?;
    let raw = tmp_dir.join("raw.mp4"); let sock = session::socket_path(&args.run_id);
    let created = Command::new("date").args(["-u", "+%Y-%m-%dT%H:%M:%SZ"]).output().ok().map(|r| String::from_utf8_lossy(&r.stdout).trim().to_string()).unwrap_or_default();
    let mut state = RunState { run_id: args.run_id.clone(), title: args.title.clone(), output: args.output.clone(), tmp_dir,
        started: Instant::now(), created_at: created, step: 0, checkpoints: 0, tests: 0, events: Vec::new(),
        action: args.title.clone(), verdict: None, git: GitInfo::collect(&args.workdir), active_test: None,
        hold_until: Instant::now() + Duration::from_millis(4500), finalizing: false };
    write_json_atomic(&state.tmp_dir.join("session.json"), &serde_json::json!({"runId":state.run_id,"title":state.title,"git":state.git,"capture":args.capture,"created_at":state.created_at}))?;
    let bin = find_capture_bin()?;
    let mut capture = match spawn_capture(&bin, &raw, &args) {
        Ok((p, start)) => { state.started = start; CaptureBackend::Sck(p) }
        Err(e) => {
            ensure!(args.capture.allows_screenshot_fallback(), "native capture failed; refusing to expand scope or drop requested audio: {e:#}");
            eprintln!("SCK unavailable: {e:#}; falling back to full-display screenshots (no audio)");
            let p = crate::cua::CuaGrabber::start(&state.run_id, &state.tmp_dir, raw.clone(), &bin, hud_git(&state))?;
            state.started = p.started(); CaptureBackend::Cua(p)
        }
    };
    state.hold_until = Instant::now() + Duration::from_millis(4500);
    capture.send(&hud_git(&state))?; capture.send(&hud_update(&state))?;
    let _ = fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock)?; fs::set_permissions(&sock, fs::Permissions::from_mode(0o600))?;
    session::save_session(&SessionFile { run_id: state.run_id.clone(), pid: std::process::id(), socket: sock.display().to_string(), workdir: args.workdir.display().to_string(), output: state.output.display().to_string(), title: state.title.clone() })?;
    for incoming in listener.incoming() {
        let stream = match incoming { Ok(s) => s, Err(_) => continue };
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut output = stream.try_clone()?;
        let mut line = String::new();
        if BufReader::new(stream).take(1_048_577).read_line(&mut line).is_err() || line.len() > 1_048_576 { continue; }
        let request = serde_json::from_str::<IpcRequest>(line.trim());
        let mut stopped = false;
        let response = match request {
            Ok(req) => (|| -> Result<IpcResponse> {
                reap_test(&mut state, &mut capture)?;
                if matches!(req, IpcRequest::Stop) {
                    let r = finish(&mut state, &mut capture, &raw, args.capture.system_audio)?; stopped = true; Ok(r)
                } else { handle(req, &mut state, &mut capture) }
            })(),
            Err(e) => Err(e.into()),
        };
        let response = response.unwrap_or_else(|e| IpcResponse::err(format!("{e:#}")));
        let _ = writeln!(output, "{}", serde_json::to_string(&response)?);
        if stopped { session::clear_session()?; let _ = fs::remove_file(&sock); return Ok(()); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> RunState {
        RunState { run_id: "7F32".into(), title: None, output: PathBuf::new(), tmp_dir: PathBuf::new(), started: Instant::now(), created_at: String::new(), step: 0, checkpoints: 0, tests: 0, events: vec![], action: Some("Original action".into()), verdict: Some("Agent verdict: PASS".into()), git: GitInfo::collect(Path::new("/tmp")), active_test: None, hold_until: Instant::now(), finalizing: false }
    }
    #[test]
    fn expect_preserves_action_but_test_clears_old_verdict() {
        let mut s = state();
        s.next_step(EventKind::Expect, "Expected result".into(), None);
        assert_eq!(s.action.as_deref(), Some("Original action"));
        s.next_step(EventKind::TestStart, "Command: true".into(), None);
        assert!(s.verdict.is_none());
        s.next_step(EventKind::TestResult, "Exit: 0".into(), None);
        assert!(s.verdict.is_none()); assert_eq!(s.step, 3);
    }
    #[test]
    fn requested_audio_or_app_never_uses_screenshot_fallback() {
        assert!(CaptureOptions::default().allows_screenshot_fallback());
        assert!(!CaptureOptions { system_audio: true, ..Default::default() }.allows_screenshot_fallback());
        assert!(!CaptureOptions { app: Some("Safari".into()), ..Default::default() }.allows_screenshot_fallback());
        assert!(CaptureOptions { screen: Some("typo".into()), ..Default::default() }.validate().is_err());
    }
}
