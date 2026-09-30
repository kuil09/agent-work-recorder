//! Full-display, video-only fallback. Frame durations follow real elapsed time, not a fixed 2 fps clock.
use crate::protocol::CaptureHud;
use anyhow::{bail, ensure, Context, Result};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct CuaGrabber {
    stop: Arc<AtomicBool>,
    hud: Arc<Mutex<serde_json::Value>>,
    worker: Option<JoinHandle<Result<()>>>,
    frames_dir: PathBuf,
    raw_path: PathBuf,
    started: Instant,
    completed: bool,
}
#[derive(Clone)]
struct CuaTarget {
    bin: PathBuf,
    socket: PathBuf,
}
fn discover_cua() -> Result<CuaTarget> {
    if let (Ok(bin), Ok(sock)) = (
        std::env::var("REC_CUA_BIN"),
        std::env::var("REC_CUA_SOCKET"),
    ) {
        return Ok(CuaTarget {
            bin: bin.into(),
            socket: sock.into(),
        });
    }
    let out = Command::new("ps").args(["-ax", "-o", "args="]).output()?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if !line.contains("cua-driver") || !line.contains("serve") {
            continue;
        }
        let parts: Vec<_> = line.split_whitespace().collect();
        if let (Some(bin), Some(i)) = (parts.first(), parts.iter().position(|p| *p == "--socket")) {
            if let Some(sock) = parts.get(i + 1) {
                let t = CuaTarget {
                    bin: bin.into(),
                    socket: sock.into(),
                };
                if t.bin.exists() && t.socket.exists() {
                    return Ok(t);
                }
            }
        }
    }
    bail!("no CuaDriver socket; grant native Screen Recording permission")
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
fn screenshot(target: &CuaTarget, path: &Path) -> Result<()> {
    let output = Command::new(&target.bin)
        .arg("--socket")
        .arg(&target.socket)
        .args(["call", "get_desktop_state"])
        .arg(json!({"screenshot_out_file":path.display().to_string()}).to_string())
        .output()?;
    ensure!(
        output.status.success() && path.is_file(),
        "CuaDriver screenshot failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
fn stamp(bin: &Path, raw: &Path, output: &Path, state: &Path, run: &str) -> Result<()> {
    let status = Command::new(bin)
        .arg("stamp")
        .arg("--input")
        .arg(raw)
        .arg("--output")
        .arg(output)
        .arg("--state")
        .arg(state)
        .arg("--run-id")
        .arg(run)
        .stdout(Stdio::null())
        .status()?;
    ensure!(
        status.success() && output.is_file(),
        "frame overlay failed; refusing an unaddressable screenshot"
    );
    Ok(())
}
fn concat_text(frames: &[(PathBuf, f64)], end: f64) -> String {
    let mut out = String::from("ffconcat version 1.0\n");
    for (i, (path, start)) in frames.iter().enumerate() {
        let next = frames.get(i + 1).map(|x| x.1).unwrap_or(end);
        out.push_str(&format!(
            "file '{}'\nduration {:.6}\n",
            path.display().to_string().replace('\'', "'\\''"),
            (next - start).max(0.001)
        ));
    }
    if let Some((path, _)) = frames.last() {
        out.push_str(&format!(
            "file '{}'\n",
            path.display().to_string().replace('\'', "'\\''")
        ));
    }
    out
}
impl CuaGrabber {
    pub fn start(
        run: &str,
        tmp: &Path,
        raw_path: PathBuf,
        bin: &Path,
        initial: CaptureHud,
    ) -> Result<Self> {
        let target = discover_cua()?;
        let frames_dir = tmp.join("frames");
        fs::create_dir_all(&frames_dir)?;
        let state_path = tmp.join("hud.json");
        let mut state = serde_json::to_value(initial)?;
        state["run_id"] = json!(run);
        state["git_until_ms"] = json!(now_ms() + 4500);
        fs::write(&state_path, serde_json::to_vec(&state)?)?;
        let probe = tmp.join("probe.png");
        screenshot(&target, &probe)?;
        let started = Instant::now();
        let first = frames_dir.join("f-00000.png");
        stamp(bin, &probe, &first, &state_path, run)?;
        let _ = fs::remove_file(&probe);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let hud = Arc::new(Mutex::new(state));
        let hud2 = hud.clone();
        let dir = frames_dir.clone();
        let bin = bin.to_path_buf();
        let run = run.to_string();
        let worker = thread::spawn(move || -> Result<()> {
            let mut frames = vec![(first, 0.0)];
            while !stop2.load(Ordering::SeqCst) {
                let raw = dir.join("current.png");
                screenshot(&target, &raw)?;
                let captured = started.elapsed().as_secs_f64();
                let stamped = dir.join(format!("f-{:05}.png", frames.len()));
                fs::write(&state_path, serde_json::to_vec(&*hud2.lock().unwrap())?)?;
                stamp(&bin, &raw, &stamped, &state_path, &run)?;
                frames.push((stamped, captured));
                let _ = fs::remove_file(raw);
                thread::sleep(Duration::from_millis(450));
            }
            fs::write(
                dir.join("list.txt"),
                concat_text(&frames, started.elapsed().as_secs_f64()),
            )?;
            Ok(())
        });
        Ok(Self {
            stop,
            hud,
            worker: Some(worker),
            frames_dir,
            raw_path,
            started,
            completed: false,
        })
    }
    pub fn started(&self) -> Instant {
        self.started
    }
    pub fn send(&self, hud: &CaptureHud) -> Result<()> {
        ensure!(
            !self
                .worker
                .as_ref()
                .map(|h| h.is_finished())
                .unwrap_or(true),
            "screenshot worker is no longer running"
        );
        let incoming = serde_json::to_value(hud)?;
        let mut cur = self.hud.lock().unwrap();
        if let (Some(dst), Some(src)) = (cur.as_object_mut(), incoming.as_object()) {
            for (key, value) in src {
                if key == "verdict" {
                    if hud.cmd == "hud" {
                        dst.insert(key.clone(), value.clone());
                    }
                } else if !value.is_null() {
                    dst.insert(key.clone(), value.clone());
                }
            }
            if hud.cmd == "card" {
                dst.insert("card_until_ms".into(), json!(now_ms() + 4000));
            }
            if hud.cmd == "git" {
                dst.insert("git_until_ms".into(), json!(now_ms() + 4500));
            }
        }
        Ok(())
    }
    pub fn stop(&mut self) -> Result<()> {
        if self.completed {
            return Ok(());
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.worker.take() {
            h.join()
                .map_err(|_| anyhow::anyhow!("screenshot worker panicked"))??;
        }
        let output = Command::new("ffmpeg")
            .args([
                "-y", "-nostdin", "-v", "error", "-f", "concat", "-safe", "0", "-i",
            ])
            .arg(self.frames_dir.join("list.txt"))
            .args([
                "-vf",
                "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-fps_mode",
                "vfr",
            ])
            .arg(&self.raw_path)
            .output()
            .context("ffmpeg screenshot encoding")?;
        ensure!(
            output.status.success(),
            "screenshot encoding failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.completed = true;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_durations_use_elapsed_time_and_escape_paths() {
        let text = concat_text(
            &[
                (PathBuf::from("/tmp/a'b.png"), 0.0),
                (PathBuf::from("/tmp/c.png"), 1.75),
            ],
            4.0,
        );
        assert!(text.contains("duration 1.750000"));
        assert!(text.contains("duration 2.250000"));
        assert!(text.contains("a'\\''b.png"));
    }
}
