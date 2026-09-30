use crate::git::GitInfo;
use crate::id::{format_step_id, write_json_atomic};
use crate::protocol::{CaptureHud, EventKind, IpcRequest, IpcResponse, ObserveStatus, RunEvent};
use crate::session::{self, SessionFile};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct DaemonArgs {
    pub run_id: String,
    pub title: Option<String>,
    pub workdir: PathBuf,
    pub output: PathBuf,
    pub screen: Option<String>,
    pub window: Option<String>,
    pub window_id: Option<u32>,
}

struct RunState {
    run_id: String,
    title: Option<String>,
    output: PathBuf,
    tmp_dir: PathBuf,
    started: Instant,
    step: u32,
    checkpoints: u32,
    events: Vec<RunEvent>,
    action: Option<String>,
    verdict: Option<String>,
    git: GitInfo,
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
        if kind == EventKind::Note || kind == EventKind::Checkpoint {
            self.action = Some(text.clone());
        }
        if kind == EventKind::Observe {
            if let Some(label) = status.and_then(|s| s.as_verdict_label()) {
                self.verdict = Some(label.to_string());
            } else if status == Some(ObserveStatus::Info) {
                self.verdict = None;
            }
            self.action = Some(text.clone());
        }
        let event = RunEvent {
            ts_ms: now_ms(),
            step: self.step,
            step_id: format_step_id(&self.run_id, self.step),
            kind,
            text,
            status,
        };
        self.events.push(event.clone());
        event
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn find_capture_bin() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("REC_CAPTURE") {
        return Ok(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("rec-capture");
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    which("rec-capture").context("rec-capture not found on PATH")
}

fn which(name: &str) -> Result<PathBuf> {
    let out = Command::new("which").arg(name).output()?;
    if !out.status.success() {
        bail!("{name} not found");
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    ))
}

#[derive(Debug, Deserialize)]
struct CaptureEvent {
    event: String,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WindowInfo {
    id: u32,
    app: String,
    #[serde(default)]
    title: String,
}

fn capture_list_windows(bin: &Path) -> Result<Vec<WindowInfo>> {
    let out = Command::new(bin)
        .arg("list-windows")
        .output()
        .context("rec-capture list-windows")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("Screen Recording permission") || out.status.code() == Some(2) {
            eprint!("{}", err);
            std::process::exit(1);
        }
        bail!("list-windows failed: {err}");
    }
    Ok(serde_json::from_slice(&out.stdout).unwrap_or_default())
}

fn resolve_window(bin: &Path, query: &str) -> Result<u32> {
    let windows = capture_list_windows(bin)?;
    let q = query.to_lowercase();
    let matches: Vec<&WindowInfo> = windows
        .iter()
        .filter(|w| w.title.to_lowercase().contains(&q) || w.app.to_lowercase().contains(&q))
        .collect();
    match matches.as_slice() {
        [] => bail!("no window matching `{query}`"),
        [one] => Ok(one.id),
        many => {
            let exact_title: Vec<&WindowInfo> = many
                .iter()
                .copied()
                .filter(|w| w.title.to_lowercase() == q)
                .collect();
            if exact_title.len() == 1 {
                return Ok(exact_title[0].id);
            }
            let exact_app: Vec<&WindowInfo> = many
                .iter()
                .copied()
                .filter(|w| w.app.to_lowercase() == q)
                .collect();
            if !exact_app.is_empty() {
                return Ok(exact_app[0].id);
            }
            eprintln!("multiple windows match `{query}`, using first:");
            for w in many {
                eprintln!("  {}  {} — {}", w.id, w.app, w.title);
            }
            Ok(many[0].id)
        }
    }
}

struct CaptureProc {
    child: Child,
    stdin: ChildStdin,
}

enum CaptureBackend {
    Sck(CaptureProc),
    Cua(crate::cua::CuaGrabber),
}

impl CaptureBackend {
    fn send(&mut self, hud: &CaptureHud) -> Result<()> {
        match self {
            CaptureBackend::Sck(p) => p.send(hud),
            CaptureBackend::Cua(p) => p.send(hud),
        }
    }

    fn stop(&mut self) -> Result<()> {
        match self {
            CaptureBackend::Sck(p) => p.stop(),
            CaptureBackend::Cua(p) => p.stop(),
        }
    }
}

impl CaptureProc {
    fn send(&mut self, hud: &CaptureHud) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, hud)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        let stop = CaptureHud {
            cmd: "stop".into(),
            step: None,
            action: None,
            verdict: None,
            kind: None,
            body: None,
            title: None,
            repository: None,
            branch: None,
            commit: None,
            working_tree: None,
        };
        let _ = self.send(&stop);
        Ok(())
    }
}

fn hud_update(state: &RunState) -> CaptureHud {
    CaptureHud {
        cmd: "hud".into(),
        step: Some(state.step),
        action: state.action.clone(),
        verdict: state.verdict.clone(),
        kind: None,
        body: None,
        title: None,
        repository: None,
        branch: None,
        commit: None,
        working_tree: None,
    }
}

fn hud_card(kind: EventKind, body: &str, step: u32) -> CaptureHud {
    CaptureHud {
        cmd: "card".into(),
        step: Some(step),
        action: None,
        verdict: None,
        kind: Some(kind.as_card_title().into()),
        body: Some(body.to_string()),
        title: None,
        repository: None,
        branch: None,
        commit: None,
        working_tree: None,
    }
}

fn hud_git(state: &RunState) -> CaptureHud {
    CaptureHud {
        cmd: "git".into(),
        step: Some(0),
        action: None,
        verdict: None,
        kind: None,
        body: None,
        title: state.title.clone(),
        repository: state.git.repository.clone(),
        branch: state.git.branch.clone(),
        commit: state.git.commit.clone(),
        working_tree: state.git.working_tree.clone(),
    }
}

fn spawn_capture(
    bin: &Path,
    raw_path: &Path,
    run_id: &str,
    screen: &Option<String>,
    window: &Option<String>,
    window_id: Option<u32>,
) -> Result<CaptureProc> {
    let mut cmd = Command::new(bin);
    cmd.arg("start")
        .arg("--output")
        .arg(raw_path)
        .arg("--run-id")
        .arg(run_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    if let Some(id) = window_id {
        cmd.arg("--window-id").arg(id.to_string());
    } else if let Some(w) = window {
        let id = resolve_window(bin, w)?;
        cmd.arg("--window-id").arg(id.to_string());
    } else {
        let screen = screen.as_deref().unwrap_or("full");
        if screen != "full" {
            bail!("--screen only supports `full` in phase 1");
        }
        cmd.arg("--display").arg("main");
    }

    let mut child = cmd.spawn().context("spawn rec-capture")?;
    let stdin = child.stdin.take().context("capture stdin")?;
    let stdout = child.stdout.take().context("capture stdout")?;

    let ready = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(Mutex::new(None::<String>));
    let ready2 = ready.clone();
    let failed2 = failed.clone();

    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            eprintln!("capture: {line}");
            if let Ok(ev) = serde_json::from_str::<CaptureEvent>(&line) {
                match ev.event.as_str() {
                    "ready" | "started" => ready2.store(true, Ordering::SeqCst),
                    "error" => {
                        *failed2.lock().unwrap() =
                            Some(ev.message.unwrap_or_else(|| "capture error".into()));
                    }
                    "permission-denied" => {
                        *failed2.lock().unwrap() = Some(
                            "Screen Recording permission is required.\n\nSystem Settings >\nPrivacy & Security >\nScreen & System Audio Recording"
                                .into(),
                        );
                    }
                    _ => {}
                }
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(msg) = failed.lock().unwrap().clone() {
            let _ = child.kill();
            bail!("{msg}");
        }
        if ready.load(Ordering::SeqCst) {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            bail!("rec-capture did not become ready");
        }
        match child.try_wait()? {
            Some(status) => bail!("rec-capture exited early: {status}"),
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    Ok(CaptureProc { child, stdin })
}

fn append_event(path: &Path, event: &RunEvent) -> Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut f, event)?;
    f.write_all(b"\n")?;
    Ok(())
}

fn handle_event(
    state: &mut RunState,
    capture: &mut CaptureBackend,
    events_path: &Path,
    kind: EventKind,
    text: String,
    status: Option<ObserveStatus>,
) -> Result<IpcResponse> {
    let event = state.next_step(kind, text, status);
    append_event(events_path, &event)?;
    let _ = capture.send(&hud_update(state));
    let _ = capture.send(&hud_card(kind, &event.text, event.step));
    let mut resp = IpcResponse {
        ok: true,
        error: None,
        run_id: Some(state.run_id.clone()),
        step: Some(event.step),
        step_id: Some(event.step_id.clone()),
        kind: Some(kind.as_card_title().into()),
        output: None,
        duration: None,
        steps: None,
        checkpoints: None,
        verdict: None,
    };
    if kind == EventKind::Observe {
        resp.verdict = status
            .and_then(|s| s.as_verdict_label())
            .map(|s| s.to_string());
    }
    Ok(resp)
}

fn finalize(
    state: &RunState,
    capture: &mut CaptureBackend,
    raw_path: &Path,
) -> Result<IpcResponse> {
    capture.stop()?;
    if let CaptureBackend::Sck(p) = capture {
        let timeout = Instant::now() + Duration::from_secs(30);
        loop {
            match p.child.try_wait()? {
                Some(_) => break,
                None if Instant::now() > timeout => {
                    let _ = p.child.kill();
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    if let Some(parent) = state.output.parent() {
        fs::create_dir_all(parent)?;
    }

    if raw_path.exists() {
        remux_mp4(raw_path, &state.output, state)?;
    } else {
        bail!("capture file missing: {}", raw_path.display());
    }

    let _ = fs::remove_dir_all(&state.tmp_dir);

    Ok(IpcResponse {
        ok: true,
        error: None,
        run_id: Some(state.run_id.clone()),
        step: Some(state.step),
        step_id: None,
        kind: None,
        output: Some(state.output.display().to_string()),
        duration: Some(format_duration(state.started.elapsed())),
        steps: Some(state.step),
        checkpoints: Some(state.checkpoints),
        verdict: state.verdict.clone(),
    })
}

fn remux_mp4(raw: &Path, output: &Path, state: &RunState) -> Result<()> {
    let tmp_out = output.with_extension("tmp.mp4");
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(raw)
        .args(["-c", "copy", "-movflags", "+faststart"]);
    if let Some(title) = &state.title {
        cmd.arg("-metadata").arg(format!("title={title}"));
    }
    cmd.arg("-metadata")
        .arg(format!("comment=runId={}", state.run_id));
    if let Some(commit) = &state.git.commit {
        cmd.arg("-metadata").arg(format!("synopsis=git {commit}"));
    }
    cmd.arg(&tmp_out);
    let status = cmd.status().context("ffmpeg remux")?;
    if status.success() && tmp_out.exists() {
        fs::rename(&tmp_out, output)?;
        return Ok(());
    }
    fs::copy(raw, output).context("copy raw capture")?;
    Ok(())
}

fn detach() {
    extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        let _ = setsid();
    }
}

pub fn run(args: DaemonArgs) -> Result<()> {
    detach();

    let tmp_dir = session::run_tmp_dir(&args.run_id);
    fs::create_dir_all(&tmp_dir)?;
    let raw_path = tmp_dir.join("raw.mp4");
    let events_path = tmp_dir.join("events.jsonl");
    let sock_path = session::socket_path(&args.run_id);
    let _ = fs::remove_file(&sock_path);

    let git = GitInfo::collect(&args.workdir);
    let mut state = RunState {
        run_id: args.run_id.clone(),
        title: args.title.clone(),
        output: args.output.clone(),
        tmp_dir: tmp_dir.clone(),
        started: Instant::now(),
        step: 0,
        checkpoints: 0,
        events: Vec::new(),
        action: args.title.clone(),
        verdict: None,
        git,
    };

    write_json_atomic(
        &tmp_dir.join("session.json"),
        &serde_json::json!({
            "runId": state.run_id,
            "title": state.title,
            "git": state.git,
        }),
    )?;

    let capture_bin = find_capture_bin()?;
    let mut capture = match spawn_capture(
        &capture_bin,
        &raw_path,
        &state.run_id,
        &args.screen,
        &args.window,
        args.window_id,
    ) {
        Ok(p) => CaptureBackend::Sck(p),
        Err(e) => {
            eprintln!("SCK capture unavailable: {e:#}");
            if args.window.is_some() || args.window_id.is_some() {
                let _ = fs::remove_dir_all(&tmp_dir);
                bail!(
                    "specific window capture failed; refusing full-screen screenshot fallback: {e:#}"
                );
            }
            eprintln!("falling back to CuaDriver screenshots");
            CaptureBackend::Cua(crate::cua::CuaGrabber::start(
                &state.run_id,
                &tmp_dir,
                raw_path.clone(),
                &capture_bin,
                hud_git(&state),
            )?)
        }
    };

    let _ = capture.send(&hud_git(&state));
    let _ = capture.send(&hud_update(&state));

    let listener =
        UnixListener::bind(&sock_path).with_context(|| format!("bind {}", sock_path.display()))?;

    let session = SessionFile {
        run_id: state.run_id.clone(),
        pid: std::process::id(),
        socket: sock_path.display().to_string(),
        workdir: args.workdir.display().to_string(),
        output: args.output.display().to_string(),
        title: args.title.clone(),
    };
    session::save_session(&session)?;

    eprintln!("daemon ready run={}", state.run_id);

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut stream_out = match stream.try_clone() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut buf = String::new();
        let mut reader = BufReader::new(stream);
        if reader.read_line(&mut buf).is_err() {
            continue;
        }
        let req: IpcRequest = match serde_json::from_str(buf.trim()) {
            Ok(r) => r,
            Err(e) => {
                let _ = writeln!(
                    stream_out,
                    "{}",
                    serde_json::to_string(&IpcResponse::err(e.to_string())).unwrap()
                );
                continue;
            }
        };
        let resp = match req {
            IpcRequest::Note { text } => handle_event(
                &mut state,
                &mut capture,
                &events_path,
                EventKind::Note,
                text,
                None,
            ),
            IpcRequest::Expect { text } => handle_event(
                &mut state,
                &mut capture,
                &events_path,
                EventKind::Expect,
                text,
                None,
            ),
            IpcRequest::Observe { text, status } => handle_event(
                &mut state,
                &mut capture,
                &events_path,
                EventKind::Observe,
                text,
                status.or(Some(ObserveStatus::Info)),
            ),
            IpcRequest::Checkpoint { text } => handle_event(
                &mut state,
                &mut capture,
                &events_path,
                EventKind::Checkpoint,
                text,
                None,
            ),
            IpcRequest::Status => Ok(IpcResponse {
                ok: true,
                error: None,
                run_id: Some(state.run_id.clone()),
                step: Some(state.step),
                step_id: Some(format_step_id(&state.run_id, state.step)),
                kind: None,
                output: Some(state.output.display().to_string()),
                duration: Some(format_duration(state.started.elapsed())),
                steps: Some(state.step),
                checkpoints: Some(state.checkpoints),
                verdict: state.verdict.clone(),
            }),
            IpcRequest::Stop => match finalize(&state, &mut capture, &raw_path) {
                Ok(r) => {
                    let json = serde_json::to_string(&r)?;
                    let _ = writeln!(stream_out, "{json}");
                    let _ = session::clear_session();
                    let _ = fs::remove_file(&sock_path);
                    return Ok(());
                }
                Err(e) => Ok(IpcResponse::err(e.to_string())),
            },
        };
        match resp {
            Ok(r) => {
                let _ = writeln!(stream_out, "{}", serde_json::to_string(&r).unwrap());
            }
            Err(e) => {
                let _ = writeln!(
                    stream_out,
                    "{}",
                    serde_json::to_string(&IpcResponse::err(e.to_string())).unwrap()
                );
            }
        }
    }
    Ok(())
}


#[cfg(test)]
mod phase1_contract_tests {
    use super::*;

    fn no_git() -> GitInfo {
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

    fn state_with_action(action: &str) -> RunState {
        RunState {
            run_id: "7F32".into(),
            title: None,
            output: PathBuf::from("/tmp/out.mp4"),
            tmp_dir: PathBuf::from("/tmp/agent-recorder-test"),
            started: Instant::now(),
            step: 0,
            checkpoints: 0,
            events: Vec::new(),
            action: Some(action.into()),
            verdict: None,
            git: no_git(),
        }
    }

    #[test]
    fn expect_is_temporary_context_not_persistent_action() {
        let mut state = state_with_action("Verify login error layout");
        let event = state.next_step(
            EventKind::Expect,
            "Error should appear below the button".into(),
            None,
        );

        assert_eq!(event.step_id, "7F32:001");
        assert_eq!(state.action.as_deref(), Some("Verify login error layout"));
    }

    #[test]
    fn note_updates_persistent_action() {
        let mut state = state_with_action("Initial task");
        state.next_step(EventKind::Note, "Adjust spacing".into(), None);

        assert_eq!(state.action.as_deref(), Some("Adjust spacing"));
    }
}
