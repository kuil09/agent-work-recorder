use crate::git::GitInfo;
use crate::health::{CaptureHealth, HealthTracker};
use crate::id::{format_step_id, write_json_atomic};
use crate::protocol::{
    CaptureHud, EventKind, IpcRequest, IpcResponse, ObserveStatus, RunEvent, TestResult,
};
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
    /// Capture the main display (only full is supported)
    #[arg(long, value_name = "full", conflicts_with_all = ["window", "window_id", "app"],
        long_help = "Record the entire main display. Only full is accepted; not a numeric display ID or all monitors. This is also the default when no target is supplied. May expose unrelated apps and notifications. Prefer an explicit window or app.")]
    pub screen: Option<String>,
    /// Find a window by title or application substring
    #[arg(long, value_name = "QUERY", conflicts_with_all = ["screen", "window_id", "app"],
        long_help = "Case-insensitive substring match on window title or owning app. A unique exact title can disambiguate; otherwise multiple matches fail. Use rec-capture list-windows and --window-id to select explicitly. Never falls back to full-display screenshots.")]
    pub window: Option<String>,
    /// Capture one discovered window by its current numeric ID
    #[arg(long, value_name = "ID", conflicts_with_all = ["screen", "window", "app"],
        long_help = "Current numeric window ID from rec-capture list-windows. IDs may change when a window closes or an app restarts. A missing target fails instead of widening scope. This selects video only; optional audio remains app-scoped.")]
    pub window_id: Option<u32>,
    /// Capture one application's windows on the main display
    #[arg(long, value_name = "NAME_OR_BUNDLE_ID", conflicts_with_all = ["screen", "window", "window_id"],
        long_help = "Exact running application name or bundle ID, such as com.apple.Safari. Includes that application's windows on the main display only; not all monitors or an arbitrary set of apps. Discover with rec-capture list-apps. No automatic reconnection after app restart or wider fallback.")]
    pub app: Option<String>,
    /// Opt in to system/app audio (default OFF; never microphone)
    #[arg(
        long,
        long_help = "Explicitly enable 48 kHz stereo AAC. Default OFF. Full-display video uses system audio; app/window video uses owning-app audio. A single-window recording can therefore include sound from the same app's OTHER windows. Never captures microphone input. Missing requested audio is an error, not silent success."
    )]
    pub system_audio: bool,
    /// Allow a lower-frequency CuaDriver screenshot fallback (full display, video only)
    #[arg(
        long,
        long_help = "Permit falling back to CuaDriver screenshots when native ScreenCaptureKit capture fails for a default/full-display, video-only Run. Default OFF so that a permission or initialization failure is reported instead of being masked. The fallback is recorded in the stop output and the MP4 comment metadata. Never applies to app, window or audio requests."
    )]
    #[serde(default)]
    pub allow_screenshot_fallback: bool,
}
impl CaptureOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            [
                self.screen.is_some(),
                self.window.is_some(),
                self.window_id.is_some(),
                self.app.is_some()
            ]
            .into_iter()
            .filter(|v| *v)
            .count()
                <= 1,
            "use only one of --screen, --window, --window-id, --app"
        );
        ensure!(
            self.screen.as_deref().map(|s| s == "full").unwrap_or(true),
            "--screen only supports full"
        );
        for value in [&self.app, &self.window].into_iter().flatten() {
            ensure!(!value.trim().is_empty(), "capture target cannot be empty");
        }
        Ok(())
    }
    pub fn describe(&self) -> String {
        if let Some(app) = &self.app {
            format!("app {app} (main display)")
        } else if let Some(window) = &self.window {
            format!("window {window}")
        } else if let Some(id) = self.window_id {
            format!("window ID {id}")
        } else {
            "main display".into()
        }
    }
    fn allows_screenshot_fallback(&self) -> bool {
        self.allow_screenshot_fallback
            && self.app.is_none()
            && self.window.is_none()
            && self.window_id.is_none()
            && !self.system_audio
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonArgs {
    pub run_id: String,
    pub title: Option<String>,
    pub workdir: PathBuf,
    pub output: PathBuf,
    pub capture: CaptureOptions,
    #[serde(default)]
    pub no_git_context: bool,
}
/// Margin over the client's own timeout before the daemon stops trusting a test CLI.
const TEST_DEADLINE_GRACE: Duration = Duration::from_secs(30);
/// Used when an older client does not report its timeout.
const DEFAULT_TEST_TIMEOUT: Duration = Duration::from_secs(300);
struct ActiveTest {
    step: u32,
    owner_pid: u32,
    command: String,
    deadline: Instant,
}
struct RunState {
    run_id: String,
    title: Option<String>,
    output: PathBuf,
    tmp_dir: PathBuf,
    started: Instant,
    created_at: String,
    step: u32,
    checkpoints: u32,
    tests: u32,
    events: Vec<RunEvent>,
    action: Option<String>,
    verdict: Option<String>,
    git: GitInfo,
    active_test: Option<ActiveTest>,
    hold_until: Instant,
    finalizing: bool,
    warning: Option<String>,
}
impl RunState {
    fn next_step(
        &mut self,
        kind: EventKind,
        text: String,
        status: Option<ObserveStatus>,
    ) -> RunEvent {
        self.step += 1;
        if kind == EventKind::Checkpoint {
            self.checkpoints += 1;
        }
        if kind != EventKind::Expect {
            self.action = Some(crate::runner::display_text(&text, 110));
        }
        if kind == EventKind::Observe {
            self.verdict = status.and_then(|s| s.as_verdict_label()).map(String::from);
        }
        if matches!(kind, EventKind::TestStart | EventKind::TestResult) {
            self.verdict = None;
        }
        RunEvent {
            ts_ms: now_ms(),
            media_ms: self.started.elapsed().as_millis() as u64,
            step: self.step,
            step_id: format_step_id(&self.run_id, self.step),
            kind,
            text,
            status,
            test_result: None,
        }
    }
    fn add_warning(&mut self, message: String) {
        eprintln!("warning: {message}");
        self.warning = Some(match self.warning.take() {
            Some(existing) => format!("{existing}; {message}"),
            None => message,
        });
    }
    fn response(&self) -> IpcResponse {
        let s = self.started.elapsed().as_secs();
        IpcResponse {
            ok: true,
            run_id: Some(self.run_id.clone()),
            step: Some(self.step),
            step_id: Some(format_step_id(&self.run_id, self.step)),
            output: Some(self.output.display().to_string()),
            duration: Some(format!("{:02}:{:02}", s / 60, s % 60)),
            steps: Some(self.step),
            checkpoints: Some(self.checkpoints),
            tests: Some(self.tests),
            verdict: self.verdict.clone(),
            warning: self.warning.clone(),
            ..Default::default()
        }
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
fn find_capture_bin() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("REC_CAPTURE") {
        return Ok(PathBuf::from(p));
    }
    if let Some(dir) = std::env::current_exe()?.parent() {
        let p = dir.join("rec-capture");
        if p.is_file() {
            return Ok(p);
        }
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join("rec-capture"))
        .find(|candidate| is_executable_file(candidate))
        .context("rec-capture not found on PATH")
}
fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}
#[derive(Deserialize)]
struct WindowInfo {
    id: u32,
    app: String,
    #[serde(default)]
    title: String,
}
fn resolve_window(bin: &Path, query: &str) -> Result<u32> {
    let out = Command::new(bin).arg("list-windows").output()?;
    ensure!(
        out.status.success(),
        "list-windows failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let windows: Vec<WindowInfo> =
        serde_json::from_slice(&out.stdout).context("invalid window list")?;
    let q = query.to_lowercase();
    let matches: Vec<_> = windows
        .iter()
        .filter(|w| w.title.to_lowercase().contains(&q) || w.app.to_lowercase().contains(&q))
        .collect();
    if matches.len() == 1 {
        return Ok(matches[0].id);
    }
    let exact: Vec<_> = matches
        .iter()
        .filter(|w| w.title.to_lowercase() == q)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].id);
    }
    let options = matches
        .iter()
        .map(|w| format!("{}: {} — {}", w.id, w.app, w.title))
        .collect::<Vec<_>>()
        .join("\n");
    bail!("window target is missing or ambiguous; select --window-id\n{options}")
}
#[derive(Deserialize)]
struct CaptureEvent {
    event: String,
    message: Option<String>,
    #[serde(default)]
    elapsed_ms: u64,
    health: Option<CaptureHealth>,
}
struct CaptureProc {
    child: Child,
    stdin: ChildStdin,
    failure: Arc<Mutex<Option<String>>>,
    stopped: bool,
    telemetry: Arc<Mutex<HealthTracker>>,
    started: Instant,
}
enum CaptureBackend {
    Sck(CaptureProc),
    Cua(crate::cua::CuaGrabber),
}
impl CaptureBackend {
    fn snapshot(&mut self) -> CaptureHealth {
        match self {
            Self::Sck(p) => {
                let now = Instant::now();
                let elapsed = p.started.elapsed().as_millis() as u64;
                if !p.stopped {
                    match p.child.try_wait() {
                        Ok(Some(status)) => p.telemetry.lock().unwrap().error(
                            format!("capture process exited: {status}"),
                            now,
                            elapsed,
                        ),
                        Err(error) => {
                            p.telemetry
                                .lock()
                                .unwrap()
                                .error(error.to_string(), now, elapsed)
                        }
                        _ => {}
                    }
                }
                p.telemetry.lock().unwrap().snapshot(now, elapsed)
            }
            Self::Cua(_) => CaptureHealth {
                state: "unverified".into(),
                first_frame: "unverified (screenshot fallback)".into(),
                ..Default::default()
            },
        }
    }
    fn health(&mut self) -> Result<()> {
        if let Self::Sck(p) = self {
            if let Some(error) = p.failure.lock().unwrap().clone() {
                bail!("capture failed: {error}");
            }
            if !p.stopped {
                ensure!(
                    p.child.try_wait()?.is_none(),
                    "capture process exited unexpectedly"
                );
            }
        }
        Ok(())
    }
    fn send(&mut self, hud: &CaptureHud) -> Result<()> {
        self.health()?;
        match self {
            Self::Sck(p) => {
                serde_json::to_writer(&mut p.stdin, hud)?;
                p.stdin.write_all(b"\n")?;
                p.stdin.flush()?;
                Ok(())
            }
            Self::Cua(p) => p.send(hud),
        }
    }
    fn stop(&mut self) -> Result<()> {
        match self {
            Self::Cua(p) => p.stop(),
            Self::Sck(p) => {
                if p.stopped {
                    return Ok(());
                }
                serde_json::to_writer(
                    &mut p.stdin,
                    &CaptureHud {
                        cmd: "stop".into(),
                        ..Default::default()
                    },
                )?;
                p.stdin.write_all(b"\n")?;
                p.stdin.flush()?;
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    if let Some(status) = p.child.try_wait()? {
                        ensure!(status.success(), "capture finalization failed: {status}");
                        p.stopped = true;
                        break;
                    }
                    if Instant::now() >= deadline {
                        let _ = p.child.kill();
                        let _ = p.child.wait();
                        bail!("capture finalization timed out; raw files retained");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if let Some(error) = p.failure.lock().unwrap().clone() {
                    bail!("capture failed: {error}");
                }
                Ok(())
            }
        }
    }
}
fn diagnostic_log(run_id: &str) -> String {
    session::session_dir()
        .map(|d| {
            d.join("logs")
                .join(format!("{run_id}.log"))
                .display()
                .to_string()
        })
        .unwrap_or_default()
}
fn hud_update(s: &RunState) -> CaptureHud {
    CaptureHud {
        cmd: "hud".into(),
        step: Some(s.step),
        action: s.action.clone(),
        verdict: s.verdict.clone(),
        ..Default::default()
    }
}
fn hud_git(s: &RunState) -> CaptureHud {
    CaptureHud {
        cmd: "git".into(),
        step: Some(0),
        title: s.title.clone(),
        repository: s.git.repository.clone(),
        branch: s.git.branch.clone(),
        commit: s.git.commit.clone(),
        working_tree: s.git.working_tree.clone(),
        ..Default::default()
    }
}
fn spawn_capture(bin: &Path, raw: &Path, args: &DaemonArgs) -> Result<(CaptureProc, Instant)> {
    args.capture.validate()?;
    let mut cmd = Command::new(bin);
    cmd.args(["start", "--output"])
        .arg(raw)
        .args(["--run-id", &args.run_id])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(id) = args.capture.window_id {
        cmd.arg("--window-id").arg(id.to_string());
    } else if let Some(w) = &args.capture.window {
        cmd.arg("--window-id")
            .arg(resolve_window(bin, w)?.to_string());
    } else if let Some(app) = &args.capture.app {
        cmd.arg("--app").arg(app);
    } else {
        cmd.args(["--display", "main"]);
    }
    if args.capture.system_audio {
        cmd.arg("--system-audio");
    }
    let mut child = cmd.spawn().context("spawn rec-capture")?;
    let stdin = child.stdin.take().context("capture stdin")?;
    let stdout = child.stdout.take().context("capture stdout")?;
    let failure = Arc::new(Mutex::new(None));
    let errors = failure.clone();
    let telemetry = Arc::new(Mutex::new(HealthTracker::default()));
    let updates = telemetry.clone();
    let spawned = Instant::now();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            eprintln!("capture: {line}");
            if let Ok(ev) = serde_json::from_str::<CaptureEvent>(&line) {
                let now = Instant::now();
                let elapsed = if ev.elapsed_ms > 0 {
                    ev.elapsed_ms
                } else {
                    spawned.elapsed().as_millis() as u64
                };
                if let Some(health) = ev.health {
                    updates.lock().unwrap().update(health, now, elapsed);
                }
                if ev.event == "target_lost" {
                    updates.lock().unwrap().target_lost(elapsed);
                }
                if ev.event == "ready" {
                    let _ = tx.send((ev.elapsed_ms, Instant::now()));
                }
                if ev.event == "error" || ev.event == "permission-denied" {
                    let message = ev
                        .message
                        .unwrap_or_else(|| "Screen Recording permission is required".into());
                    updates.lock().unwrap().error(message.clone(), now, elapsed);
                    *errors.lock().unwrap() = Some(message);
                }
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(18);
    loop {
        if let Ok((elapsed, received)) = rx.try_recv() {
            let start = received
                .checked_sub(Duration::from_millis(elapsed))
                .unwrap_or(received);
            return Ok((
                CaptureProc {
                    child,
                    stdin,
                    failure,
                    stopped: false,
                    telemetry,
                    started: start,
                },
                start,
            ));
        }
        if let Some(e) = failure.lock().unwrap().clone() {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{e}");
        }
        if let Some(status) = child.try_wait()? {
            bail!("rec-capture exited early: {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("rec-capture did not produce a first frame");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn record(
    state: &mut RunState,
    capture: &mut CaptureBackend,
    kind: EventKind,
    text: String,
    status: Option<ObserveStatus>,
    result: Option<TestResult>,
) -> Result<IpcResponse> {
    ensure!(
        !state.finalizing,
        "recording is finalizing; retry rec stop instead of adding events"
    );
    let mut event = state.next_step(kind, text, status);
    event.test_result = result;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.tmp_dir.join("events.jsonl"))?;
    serde_json::to_writer(&mut f, &event)?;
    f.write_all(b"\n")?;
    let delivery = (|| -> Result<()> {
        capture.send(&hud_update(state))?;
        capture.send(&CaptureHud {
            cmd: "card".into(),
            step: Some(event.step),
            kind: Some(kind.as_card_title().into()),
            body: Some(event.text.clone()),
            ..Default::default()
        })
    })();
    if let Err(error) = delivery {
        // Preserve the event and allow a failed/abandoned test to reach finalization.
        state.add_warning(format!(
            "annotation saved to diagnostics but capture delivery failed: {error:#}"
        ));
    }
    state.hold_until = Instant::now() + Duration::from_secs(4);
    state.events.push(event);
    let mut response = state.response();
    response.kind = Some(kind.as_card_title().into());
    if kind != EventKind::Observe {
        response.verdict = None;
    }
    Ok(response)
}
fn finish(
    state: &mut RunState,
    capture: &mut CaptureBackend,
    raw: &Path,
    audio: bool,
) -> Result<IpcResponse> {
    ensure!(
        state.active_test.is_none(),
        "a test is still running; finish it before rec stop"
    );
    if !state.finalizing {
        if let Some(wait) = state.hold_until.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        state.finalizing = true;
    }
    let capture_result = capture.stop();
    let mut health = capture.snapshot();
    if let Err(error) = &capture_result {
        state.add_warning(format!(
            "capture finalization reported {error:#}; attempting to preserve validated MP4"
        ));
    }
    let info = crate::media::probe(raw)?;
    crate::media::validate(&info, audio)?;
    let duration = crate::media::duration_ms(&info)?;
    for interval in &mut health.intervals {
        if interval.end_ms.is_none() {
            interval.end_ms = Some(duration.max(interval.start_ms));
        }
    }
    let chapters = crate::media::chapters(&state.events, duration);
    let meta = state.tmp_dir.join("chapters.ffmetadata");
    fs::write(
        &meta,
        crate::media::metadata(
            state.title.as_deref(),
            &state.run_id,
            state.git.commit.as_deref(),
            &state.created_at,
            &chapters,
        ),
    )?;
    if let Some(p) = state.output.parent() {
        fs::create_dir_all(p)?;
    }
    // A file created at the requested path while recording must not cost the whole Run.
    let published = available_output(&state.output, &state.run_id);
    if published != state.output {
        state.add_warning(format!(
            "requested output {} already exists; published {} instead",
            state.output.display(),
            published.display()
        ));
        state.output = published;
    }
    crate::media::remux(raw, &state.output, &meta, &chapters, audio, &state.run_id)?;
    let impaired =
        !health.intervals.is_empty() || health.capture_error.is_some() || capture_result.is_err();
    if health.state == "unverified" {
        state.add_warning(
            "capture telemetry unavailable or first frame unverified; inspect the MP4".into(),
        );
    }
    if impaired {
        state.add_warning(format!(
            "capture was impaired; inspect intervals and retained diagnostics: {}",
            state.tmp_dir.display()
        ));
        if let Err(error) = write_json_atomic(&state.tmp_dir.join("capture-health.json"), &health) {
            state.add_warning(format!("could not save capture health summary: {error}"));
        }
    }
    // The MP4 is published from here on: cleanup problems are warnings, never a failed stop,
    // because a retry could not find the raw capture any more.
    if let Err(error) = if impaired {
        Ok(())
    } else {
        fs::remove_dir_all(&state.tmp_dir)
    } {
        state.add_warning(format!(
            "MP4 published, but temporary files were not fully removed ({error}): {}",
            state.tmp_dir.display()
        ));
    }
    let mut reply = state.response();
    reply.capture_health = Some(health);
    reply.diagnostic_log = Some(diagnostic_log(&state.run_id));
    reply.duration = Some(format!(
        "{:02}:{:02}",
        duration / 60000,
        duration / 1000 % 60
    ));
    Ok(reply)
}
/// Returns `output`, or a sibling `<stem>-<RUNID>[-n].<ext>` when `output` is taken.
fn available_output(output: &Path, run_id: &str) -> PathBuf {
    if !output.exists() {
        return output.to_path_buf();
    }
    let stem = output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".into());
    let ext = output
        .extension()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mp4".into());
    for n in 0..100 {
        let name = if n == 0 {
            format!("{stem}-{run_id}.{ext}")
        } else {
            format!("{stem}-{run_id}-{n}.{ext}")
        };
        let candidate = output.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    output.to_path_buf()
}
fn reap_test(state: &mut RunState, capture: &mut CaptureBackend) -> Result<()> {
    let Some(test) = state.active_test.as_ref() else {
        return Ok(());
    };
    // The deadline covers a PID that cannot be probed (EPERM) or that was reused: without
    // it such a test would block `rec stop` forever.
    let reason = if Instant::now() >= test.deadline {
        "test did not report a result before its deadline; the test CLI is presumed lost, exit code unknown"
    } else if !session::pid_alive(test.owner_pid) {
        "test CLI exited before reporting a result; exit code unknown"
    } else {
        return Ok(());
    };
    end_test_without_result(state, capture, reason)
}
fn end_test_without_result(
    state: &mut RunState,
    capture: &mut CaptureBackend,
    reason: &str,
) -> Result<()> {
    let Some(test) = state.active_test.take() else {
        return Ok(());
    };
    let result = TestResult {
        error: Some(reason.into()),
        ..Default::default()
    };
    record(
        state,
        capture,
        EventKind::TestResult,
        result.card_body(&test.command),
        None,
        Some(result),
    )?;
    Ok(())
}
fn handle(
    req: IpcRequest,
    state: &mut RunState,
    capture: &mut CaptureBackend,
) -> Result<IpcResponse> {
    match req {
        IpcRequest::Note { text } => record(state, capture, EventKind::Note, text, None, None),
        IpcRequest::Expect { text } => record(state, capture, EventKind::Expect, text, None, None),
        IpcRequest::Observe { text, status } => record(
            state,
            capture,
            EventKind::Observe,
            text,
            status.or(Some(ObserveStatus::Info)),
            None,
        ),
        IpcRequest::Checkpoint { text } => {
            record(state, capture, EventKind::Checkpoint, text, None, None)
        }
        IpcRequest::Status => {
            let mut reply = state.response();
            reply.capture_health = Some(capture.snapshot());
            reply.diagnostic_log = Some(diagnostic_log(&state.run_id));
            reply.active_test = state.active_test.as_ref().map(|t| t.command.clone());
            Ok(reply)
        }
        IpcRequest::TestBegin {
            command,
            cwd,
            owner_pid,
            timeout_secs,
        } => {
            ensure!(state.active_test.is_none(), "a test is already running");
            ensure!(
                !command.is_empty() && !command[0].is_empty(),
                "test command is required"
            );
            ensure!(
                session::pid_alive(owner_pid),
                "test CLI is no longer running"
            );
            let label = crate::runner::command_label(&command);
            let reply = record(
                state,
                capture,
                EventKind::TestStart,
                format!("Command: {label}\nCwd: {cwd}\nRunning (no verdict)"),
                None,
                None,
            )?;
            state.tests += 1;
            state.active_test = Some(ActiveTest {
                step: state.step,
                owner_pid,
                command: label,
                deadline: Instant::now()
                    + if timeout_secs > 0 {
                        Duration::from_secs(timeout_secs)
                    } else {
                        DEFAULT_TEST_TIMEOUT
                    }
                    + TEST_DEADLINE_GRACE,
            });
            Ok(reply)
        }
        IpcRequest::TestEnd {
            run_id,
            test_step,
            result,
        } => {
            let active = state
                .active_test
                .as_ref()
                .context("no matching active test")?;
            ensure!(
                run_id == state.run_id && test_step == active.step,
                "test result belongs to a different run/test"
            );
            let body = result.card_body(&active.command);
            let reply = record(
                state,
                capture,
                EventKind::TestResult,
                body,
                None,
                Some(result),
            )?;
            state.active_test = None;
            Ok(reply)
        }
        IpcRequest::Stop { .. } => bail!("internal stop dispatch error"),
    }
}

pub fn run(args: DaemonArgs) -> Result<()> {
    extern "C" {
        fn setsid() -> i32;
        fn flock(fd: i32, operation: i32) -> i32;
    }
    unsafe {
        setsid();
    }
    args.capture.validate()?;
    fs::create_dir_all(session::session_dir()?)?;
    // Kernel-held lock prevents two simultaneous rec start processes from creating two runs.
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(session::session_dir()?.join("run.lock"))?;
    ensure!(
        unsafe { flock(lock.as_raw_fd(), 2 | 4) } == 0,
        "An active recording already exists."
    );
    let tmp_dir = session::run_tmp_dir(&args.run_id);
    fs::create_dir_all(&tmp_dir)?;
    let raw = tmp_dir.join("raw.mp4");
    let sock = session::socket_path(&args.run_id);
    let created = utc_timestamp(SystemTime::now());
    let mut state = RunState {
        run_id: args.run_id.clone(),
        title: args.title.clone(),
        output: args.output.clone(),
        tmp_dir,
        started: Instant::now(),
        created_at: created,
        step: 0,
        checkpoints: 0,
        tests: 0,
        events: Vec::new(),
        action: args.title.clone(),
        verdict: None,
        git: if args.no_git_context {
            GitInfo::unavailable()
        } else {
            GitInfo::collect(&args.workdir)
        },
        active_test: None,
        hold_until: Instant::now() + Duration::from_millis(4500),
        finalizing: false,
        warning: None,
    };
    write_json_atomic(
        &state.tmp_dir.join("session.json"),
        &serde_json::json!({"runId":state.run_id,"title":state.title,"git":state.git,"capture":args.capture,"created_at":state.created_at}),
    )?;
    let bin = find_capture_bin()?;
    let mut capture = match spawn_capture(&bin, &raw, &args) {
        Ok((p, start)) => {
            state.started = start;
            CaptureBackend::Sck(p)
        }
        Err(e) => {
            ensure!(
                args.capture.allows_screenshot_fallback(),
                "native capture failed; refusing to expand scope or drop requested audio \
                 (screenshot fallback is opt-in via --allow-screenshot-fallback): {e:#}"
            );
            eprintln!(
                "SCK unavailable: {e:#}; falling back to full-display screenshots (no audio)"
            );
            state.add_warning(format!(
                "native capture failed ({e:#}); recorded with the CuaDriver screenshot fallback (full display, no audio)"
            ));
            let p = crate::cua::CuaGrabber::start(
                &state.run_id,
                &state.tmp_dir,
                raw.clone(),
                &bin,
                hud_git(&state),
            )?;
            state.started = p.started();
            CaptureBackend::Cua(p)
        }
    };
    state.hold_until = Instant::now() + Duration::from_millis(4500);
    capture.send(&hud_git(&state))?;
    capture.send(&hud_update(&state))?;
    let _ = fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock)?;
    fs::set_permissions(&sock, fs::Permissions::from_mode(0o600))?;
    session::save_session(&SessionFile {
        run_id: state.run_id.clone(),
        pid: std::process::id(),
        socket: sock.display().to_string(),
        workdir: args.workdir.display().to_string(),
        output: state.output.display().to_string(),
        title: state.title.clone(),
    })?;
    // Each connection is read on its own thread so that a stalled client cannot block the
    // recorder; requests are still executed one at a time on this thread.
    let (tx, rx) = mpsc::channel::<(Result<IpcRequest, String>, std::os::unix::net::UnixStream)>();
    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(stream) = incoming else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || {
                if stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .is_err()
                    || stream
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .is_err()
                {
                    return;
                }
                let Ok(output) = stream.try_clone() else {
                    return;
                };
                let mut line = String::new();
                if BufReader::new(stream)
                    .take(1_048_577)
                    .read_line(&mut line)
                    .is_err()
                    || line.len() > 1_048_576
                {
                    return;
                }
                let request =
                    serde_json::from_str::<IpcRequest>(line.trim()).map_err(|e| e.to_string());
                let _ = tx.send((request, output));
            });
        }
    });
    loop {
        let (request, mut output) = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = capture.snapshot();
                // Also resolve an overdue test while nobody is sending commands.
                if let Err(error) = reap_test(&mut state, &mut capture) {
                    eprintln!("could not close an overdue test: {error:#}");
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let mut stopped = false;
        let response = match request {
            Ok(req) => (|| -> Result<IpcResponse> {
                if !matches!(req, IpcRequest::Status) {
                    reap_test(&mut state, &mut capture)?;
                }
                if let IpcRequest::Stop { abandon_test } = req {
                    if abandon_test {
                        end_test_without_result(
                            &mut state,
                            &mut capture,
                            "abandoned by rec stop --abandon-test; exit code unknown",
                        )?;
                    }
                    let r = finish(&mut state, &mut capture, &raw, args.capture.system_audio)?;
                    stopped = true;
                    Ok(r)
                } else {
                    handle(req, &mut state, &mut capture)
                }
            })(),
            Err(e) => Err(anyhow::anyhow!(e)),
        };
        let response = response.unwrap_or_else(|e| IpcResponse::err(format!("{e:#}")));
        let mut response = response;
        response.capture_target = Some(args.capture.describe());
        response.diagnostic_log = Some(diagnostic_log(&state.run_id));
        let _ = writeln!(output, "{}", serde_json::to_string(&response)?);
        if stopped {
            if let Err(error) = session::clear_session() {
                eprintln!("warning: {error:#}");
            }
            let _ = fs::remove_file(&sock);
            return Ok(());
        }
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` without spawning `date` (civil-from-days, proleptic Gregorian).
pub fn utc_timestamp(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> RunState {
        RunState {
            run_id: "7F32".into(),
            title: None,
            output: PathBuf::new(),
            tmp_dir: PathBuf::new(),
            started: Instant::now(),
            created_at: String::new(),
            step: 0,
            checkpoints: 0,
            tests: 0,
            events: vec![],
            action: Some("Original action".into()),
            verdict: Some("Agent verdict: PASS".into()),
            git: GitInfo::collect(Path::new("/tmp")),
            active_test: None,
            hold_until: Instant::now(),
            finalizing: false,
            warning: None,
        }
    }
    #[test]
    fn expect_preserves_action_but_test_clears_old_verdict() {
        let mut s = state();
        s.next_step(EventKind::Expect, "Expected result".into(), None);
        assert_eq!(s.action.as_deref(), Some("Original action"));
        s.next_step(EventKind::TestStart, "Command: true".into(), None);
        assert!(s.verdict.is_none());
        s.next_step(EventKind::TestResult, "Exit: 0".into(), None);
        assert!(s.verdict.is_none());
        assert_eq!(s.step, 3);
    }
    #[test]
    fn requested_audio_or_app_never_uses_screenshot_fallback() {
        // Fallback is opt-in: a default Run reports native failures instead of masking them.
        assert!(!CaptureOptions::default().allows_screenshot_fallback());
        let opt_in = CaptureOptions {
            allow_screenshot_fallback: true,
            ..Default::default()
        };
        assert!(opt_in.allows_screenshot_fallback());
        assert!(!CaptureOptions {
            system_audio: true,
            ..opt_in.clone()
        }
        .allows_screenshot_fallback());
        assert!(!CaptureOptions {
            app: Some("Safari".into()),
            ..opt_in
        }
        .allows_screenshot_fallback());
        assert!(!CaptureOptions {
            system_audio: true,
            ..Default::default()
        }
        .allows_screenshot_fallback());
        assert!(!CaptureOptions {
            app: Some("Safari".into()),
            ..Default::default()
        }
        .allows_screenshot_fallback());
        assert!(CaptureOptions {
            screen: Some("typo".into()),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    #[test]
    fn utc_timestamp_matches_known_instants() {
        let at = |secs| UNIX_EPOCH + Duration::from_secs(secs);
        assert_eq!(utc_timestamp(at(0)), "1970-01-01T00:00:00Z");
        assert_eq!(
            utc_timestamp(at(951_782_400 + 86_399)),
            "2000-02-29T23:59:59Z"
        );
        assert_eq!(
            utc_timestamp(at(1_782_864_000 + 3_723)),
            "2026-07-01T01:02:03Z"
        );
    }
    #[test]
    fn conflicting_output_gets_a_run_scoped_sibling() {
        let dir = std::env::temp_dir().join(format!("rec-out-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let wanted = dir.join("review.mp4");
        assert_eq!(available_output(&wanted, "7F32"), wanted);
        fs::write(&wanted, b"x").unwrap();
        let first = available_output(&wanted, "7F32");
        assert_eq!(first, dir.join("review-7F32.mp4"));
        fs::write(&first, b"x").unwrap();
        assert_eq!(
            available_output(&wanted, "7F32"),
            dir.join("review-7F32-1.mp4")
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
